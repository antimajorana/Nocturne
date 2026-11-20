//! Native iPadOS / iOS `UITextInput`, `UIKeyInput`, and Scribble responder bridge.
//!
//! Splits platform-neutral text window modeling, newline splitting, caret sanitization,
//! and Scribble target-rect gating from the `#[cfg(target_os = "ios")]` UIKit classes so
//! core invariants compile and run in `cargo test -p nocturne-ios` on all hosts.

use nocturne_ui_egui::{NocturneApp, PlatformEditAction};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ops::Range;

/// Maximum UTF-16 code units kept in the local `TextDocumentModel` window.
const MAX_WINDOW_UTF16: usize = 4096;

/// Minimum caret width in logical UIKit points.
const MIN_CARET_WIDTH_PTS: f32 = 2.0;

/// Minimum caret height in logical UIKit points.
const MIN_CARET_HEIGHT_PTS: f32 = 18.0;

/// Horizontal padding around the active caret for `UIIndirectScribbleInteraction`.
const SCRIBBLE_PAD_X_PTS: f32 = 160.0;

/// Vertical padding around the active caret for `UIIndirectScribbleInteraction`.
const SCRIBBLE_PAD_Y_PTS: f32 = 48.0;

/// Standard UIKit edit-menu / responder actions dispatched from `NocturneTextInputView`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextEditAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Delete,
}

/// Platform-neutral text editing commands emitted by UIKit `UIKeyInput`/`UITextInput` callbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEditCommand {
    InsertText(String),
    CommitText(String),
    PressEnter,
    MoveCursor(isize),
    DeleteBackward,
    EditAction(TextEditAction),
    SetMarkedText { text: String, selected_utf16: (usize, usize) },
    UnmarkText,
}

fn is_whole_page_fallback(cr: egui::Rect, page_rect: Option<egui::Rect>) -> bool {
    page_rect.is_some_and(|pr| {
        (cr.min.x - pr.min.x).abs() < 0.5
            && (cr.min.y - pr.min.y).abs() < 0.5
            && (cr.width() - pr.width()).abs() < 0.5
            && (cr.height() - pr.height()).abs() < 0.5
    })
}

/// Returns `true` when `PlatformOutput::ime` belongs to an external `egui::TextEdit`
/// (dialogs, popups, modals, source view, or top-bar search / ribbon inputs)
/// rather than `NocturneApp`'s own page block or page title caret.
#[must_use]
pub fn should_route_ime_to_egui(
    ime_cursor_rect: Option<egui::Rect>,
    caret_rect: Option<egui::Rect>,
    page_rect: Option<egui::Rect>,
    ctx: &egui::Context,
) -> bool {
    let Some(ime_rect) = ime_cursor_rect else {
        return false;
    };
    let Some(cr) = caret_rect else {
        return true;
    };
    if ctx.memory(|m| m.top_modal_layer().is_some()) {
        return true;
    }
    let ime_center = ime_rect.center();
    if ime_center.is_finite() && ctx.layer_id_at(ime_center).is_some_and(|layer| layer.order >= egui::Order::Middle) {
        return true;
    }
    if !is_whole_page_fallback(cr, page_rect) {
        // `caret_rect` is a localized caret (not the whole-page `page_rect` fallback).
        // Never fall back to `page_rect` containment here: scrolling a long page down
        // translates `page_rect` upward behind the top bar and ribbon. Checked before
        // `ctx.any_popup_open()` so an unrelated non-text popup (e.g. Theme menu) does
        // not hijack a localized block/title caret, while popups containing a `TextEdit`
        // on `Order::Foreground` are already matched by `layer.order >= Order::Middle`.
        return !cr.expand(8.0).intersects(ime_rect);
    }
    if ctx.any_popup_open() {
        return true;
    }
    if ime_center.is_finite() {
        let band_id = ctx.viewport_id().root_ui_id().with("band");
        if let Some(band) = egui::containers::panel::PanelState::load(ctx, band_id)
            && band.outer_rect.contains(ime_center)
        {
            return true;
        }
    }
    if cr.expand(16.0).intersects(ime_rect) {
        return false;
    }
    if let Some(pr) = page_rect
        && !pr.expand(4.0).contains(ime_center)
    {
        return true;
    }
    false
}

/// Whether the keyboard's text goes to egui's own text box this frame.
///
/// As [`should_route_ime_to_egui`], and the page title as well. The title is an egui text box.
/// It used to be typed into by the app instead, through a path that trimmed each piece of
/// text, and a space typed on its own was trimmed to nothing. The text box types its own
/// letters, spaces included.
#[must_use]
pub fn route_ime_to_egui(
    title_focused: bool,
    ime_cursor_rect: Option<egui::Rect>,
    caret_rect: Option<egui::Rect>,
    page_rect: Option<egui::Rect>,
    ctx: &egui::Context,
) -> bool {
    ime_cursor_rect.is_some() && (title_focused || should_route_ime_to_egui(ime_cursor_rect, caret_rect, page_rect, ctx))
}

/// Maps a [`TextEditAction`] to [`PlatformEditAction`].
#[must_use]
pub const fn map_text_edit_action(action: TextEditAction) -> PlatformEditAction {
    match action {
        TextEditAction::Cut => PlatformEditAction::Cut,
        TextEditAction::Copy => PlatformEditAction::Copy,
        TextEditAction::Paste => PlatformEditAction::Paste,
        TextEditAction::SelectAll => PlatformEditAction::SelectAll,
        TextEditAction::Delete => PlatformEditAction::Delete,
    }
}

fn utf16_range_to_char_range(text: &str, selected_utf16: (usize, usize)) -> Range<usize> {
    let (u16_lo, u16_hi) = (selected_utf16.0.min(selected_utf16.1), selected_utf16.0.max(selected_utf16.1));
    let mut u16_pos = 0usize;
    let mut char_pos = 0usize;
    let mut char_lo = None;
    let mut char_hi = None;
    for ch in text.chars() {
        let ch_u16 = ch.len_utf16();
        if char_lo.is_none() && (u16_pos >= u16_lo || u16_lo < u16_pos + ch_u16) {
            char_lo = Some(char_pos);
        }
        if char_hi.is_none() && (u16_pos >= u16_hi || u16_hi < u16_pos + ch_u16) {
            char_hi = Some(char_pos);
        }
        u16_pos += ch_u16;
        char_pos += 1;
    }
    let lo = char_lo.unwrap_or(char_pos);
    let hi = char_hi.unwrap_or(char_pos);
    lo.min(hi)..lo.max(hi)
}

/// Pushes a synthetic key press and release pair onto `events`.
pub fn push_key_press_release(events: &mut Vec<egui::Event>, key: egui::Key, modifiers: egui::Modifiers) {
    events.push(egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers });
    events.push(egui::Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers });
}

/// Pushes a [`PlatformEditAction`] onto `egui::RawInput::events` for `egui`-owned text fields.
pub fn push_edit_action_to_egui(events: &mut Vec<egui::Event>, action: PlatformEditAction, clipboard_text: Option<String>) {
    match action {
        PlatformEditAction::Cut => events.push(egui::Event::Cut),
        PlatformEditAction::Copy => events.push(egui::Event::Copy),
        PlatformEditAction::Paste => {
            if let Some(text) = clipboard_text {
                events.push(egui::Event::Paste(text));
            }
        }
        PlatformEditAction::SelectAll => push_key_press_release(events, egui::Key::A, egui::Modifiers::COMMAND),
        PlatformEditAction::Delete => push_key_press_release(events, egui::Key::Delete, egui::Modifiers::NONE),
    }
}

/// Locks horizontal and vertical arrow-key focus on the currently focused widget once egui
/// confirms it already held focus on the previous frame (`had_focus_last_frame`).
///
/// Returns `true` when focus is locked and arrow keys can be safely delivered without
/// egui's `Focus::begin_pass` navigating focus away to an adjacent widget.
pub fn lock_arrow_focus_if_ready(ctx: &egui::Context) -> bool {
    ctx.memory_mut(|m| {
        let Some(focused_id) = m.focused() else {
            return false;
        };
        if !m.had_focus_last_frame(focused_id) {
            return false;
        }
        m.set_focus_lock_filter(focused_id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() });
        true
    })
}

/// Drains `queue` in strict FIFO order into `app` / `raw_input`.
///
/// When `!egui_wants_ime`, synchronous commands (`InsertText`, `CommitText`, `DeleteBackward`,
/// `EditAction`) run immediately until `PressEnter` or `MoveCursor` is reached, at which point
/// the key event is pushed into `raw_input.events` and draining stops for this frame so
/// `NocturneApp::show` can process the structural/cursor change before remaining commands run.
/// Returns `true` when remaining commands require a repaint on the next frame.
pub fn dispatch_pending_text_commands(
    queue: &mut VecDeque<TextEditCommand>,
    egui_wants_ime: bool,
    egui_ime_preedit: &mut bool,
    app: &mut NocturneApp,
    raw_input: &mut egui::RawInput,
    read_clipboard_text: impl FnMut() -> Option<String>,
) -> bool {
    let mut focus_wait_frames = 0u8;
    dispatch_pending_text_commands_with_context(
        None,
        &mut focus_wait_frames,
        queue,
        egui_wants_ime,
        egui_ime_preedit,
        app,
        raw_input,
        read_clipboard_text,
    )
}

/// Context-aware variant of [`dispatch_pending_text_commands`] that locks arrow-key focus before
/// emitting `Key::ArrowLeft` / `Key::ArrowRight` for [`TextEditCommand::MoveCursor`], deferring
/// cursor movement up to 3 frames if egui has not yet recorded `had_focus_last_frame`.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_pending_text_commands_with_context(
    ctx: Option<&egui::Context>,
    focus_wait_frames: &mut u8,
    queue: &mut VecDeque<TextEditCommand>,
    egui_wants_ime: bool,
    egui_ime_preedit: &mut bool,
    app: &mut NocturneApp,
    raw_input: &mut egui::RawInput,
    mut read_clipboard_text: impl FnMut() -> Option<String>,
) -> bool {
    let mut mutated_app_caret = false;
    while let Some(cmd) = queue.pop_front() {
        if egui_wants_ime {
            match cmd {
                TextEditCommand::InsertText(text) | TextEditCommand::CommitText(text) => {
                    *focus_wait_frames = 0;
                    if !text.is_empty() {
                        if *egui_ime_preedit {
                            raw_input.events.push(egui::Event::Ime(egui::ImeEvent::Commit(text)));
                            *egui_ime_preedit = false;
                        } else {
                            raw_input.events.push(egui::Event::Text(text));
                        }
                    }
                }
                TextEditCommand::PressEnter => {
                    *focus_wait_frames = 0;
                    push_key_press_release(&mut raw_input.events, egui::Key::Enter, egui::Modifiers::NONE);
                    if !queue.is_empty() {
                        return true;
                    }
                }
                TextEditCommand::MoveCursor(delta) if delta != 0 => {
                    if let Some(ctx) = ctx
                        && !lock_arrow_focus_if_ready(ctx)
                        && *focus_wait_frames < 3
                    {
                        *focus_wait_frames += 1;
                        queue.push_front(TextEditCommand::MoveCursor(delta));
                        return true;
                    }
                    *focus_wait_frames = 0;
                    // All of the move in this frame. An egui text box takes its key events in
                    // order within a frame, so the text queued behind a move lands in the same
                    // frame. One step a frame held a typed space back behind every move.
                    let key = if delta > 0 { egui::Key::ArrowRight } else { egui::Key::ArrowLeft };
                    for _ in 0..delta.unsigned_abs().min(MAX_WINDOW_UTF16) {
                        push_key_press_release(&mut raw_input.events, key, egui::Modifiers::NONE);
                    }
                }
                TextEditCommand::MoveCursor(_) => {
                    *focus_wait_frames = 0;
                }
                TextEditCommand::DeleteBackward => {
                    *focus_wait_frames = 0;
                    push_key_press_release(&mut raw_input.events, egui::Key::Backspace, egui::Modifiers::NONE);
                }
                TextEditCommand::EditAction(action) => {
                    *focus_wait_frames = 0;
                    let mapped = map_text_edit_action(action);
                    let clip = if mapped == PlatformEditAction::Paste { read_clipboard_text() } else { None };
                    push_edit_action_to_egui(&mut raw_input.events, mapped, clip);
                }
                TextEditCommand::SetMarkedText { text, selected_utf16 } => {
                    *focus_wait_frames = 0;
                    let active_range_chars = (!text.is_empty()).then(|| utf16_range_to_char_range(&text, selected_utf16));
                    *egui_ime_preedit = !text.is_empty();
                    raw_input.events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text, active_range_chars }));
                }
                TextEditCommand::UnmarkText => {
                    *focus_wait_frames = 0;
                    if *egui_ime_preedit {
                        *egui_ime_preedit = false;
                        raw_input.events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
                    }
                }
            }
        } else {
            match cmd {
                TextEditCommand::InsertText(text) => {
                    *focus_wait_frames = 0;
                    if app.keyboard_caret_rect().is_some() {
                        app.insert_platform_text(&text);
                        mutated_app_caret = true;
                    }
                }
                TextEditCommand::CommitText(text) => {
                    *focus_wait_frames = 0;
                    if app.keyboard_caret_rect().is_some() {
                        let parts = split_insert_text_commands(&text);
                        for sub in parts.into_iter().rev() {
                            queue.push_front(sub);
                        }
                    }
                }
                TextEditCommand::PressEnter => {
                    *focus_wait_frames = 0;
                    if app.keyboard_caret_rect().is_some() {
                        push_key_press_release(&mut raw_input.events, egui::Key::Enter, egui::Modifiers::NONE);
                        if !queue.is_empty() {
                            return true;
                        }
                    }
                }
                TextEditCommand::MoveCursor(delta) if delta != 0 => {
                    if app.keyboard_caret_rect().is_some() {
                        if mutated_app_caret {
                            queue.push_front(TextEditCommand::MoveCursor(delta));
                            return true;
                        }
                        if let Some(ctx) = ctx
                            && !lock_arrow_focus_if_ready(ctx)
                            && *focus_wait_frames < 3
                        {
                            *focus_wait_frames += 1;
                            queue.push_front(TextEditCommand::MoveCursor(delta));
                            return true;
                        }
                        *focus_wait_frames = 0;
                        let key = if delta > 0 { egui::Key::ArrowRight } else { egui::Key::ArrowLeft };
                        push_key_press_release(&mut raw_input.events, key, egui::Modifiers::NONE);
                        let rem = if delta > 0 { delta - 1 } else { delta + 1 };
                        if rem != 0 {
                            queue.push_front(TextEditCommand::MoveCursor(rem));
                        }
                        if !queue.is_empty() {
                            return true;
                        }
                    } else {
                        *focus_wait_frames = 0;
                    }
                }
                TextEditCommand::MoveCursor(_) => {
                    *focus_wait_frames = 0;
                }
                TextEditCommand::DeleteBackward => {
                    *focus_wait_frames = 0;
                    if app.keyboard_caret_rect().is_some() {
                        app.delete_backward_platform();
                        mutated_app_caret = true;
                    }
                }
                TextEditCommand::EditAction(action) => {
                    *focus_wait_frames = 0;
                    app.apply_platform_edit_action(map_text_edit_action(action));
                    mutated_app_caret = true;
                }
                TextEditCommand::SetMarkedText { .. } | TextEditCommand::UnmarkText => {
                    *focus_wait_frames = 0;
                }
            }
        }
    }
    false
}

/// Splits incoming UIKit `insertText:` payloads on `\r\n`, `\n`, and `\r`.
///
/// `NocturneApp::insert_platform_text` replaces `\r`/`\n` with spaces, so line breaks
/// from software/hardware keyboards or dictation must become [`TextEditCommand::PressEnter`].
pub fn split_insert_text_commands(text: &str) -> Vec<TextEditCommand> {
    let mut commands = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i > start {
                    commands.push(TextEditCommand::InsertText(text[start..i].to_owned()));
                }
                commands.push(TextEditCommand::PressEnter);
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                if i > start {
                    commands.push(TextEditCommand::InsertText(text[start..i].to_owned()));
                }
                commands.push(TextEditCommand::PressEnter);
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    if start < text.len() {
        commands.push(TextEditCommand::InsertText(text[start..].to_owned()));
    }
    commands
}

/// Returns the UTF-16 code-unit length of `text`.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Maps a UTF-16 code-unit offset to a valid `(byte_index, snapped_utf16_offset)` scalar boundary.
fn utf16_offset_to_byte_index(text: &str, utf16_offset: usize) -> (usize, usize) {
    let mut u16_pos = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        let ch_u16 = ch.len_utf16();
        if u16_pos >= utf16_offset || utf16_offset < u16_pos + ch_u16 {
            return (byte_idx, u16_pos);
        }
        u16_pos += ch_u16;
    }
    (text.len(), u16_pos)
}

/// Validates and sanitizes an optional caret rect `[x, y, w, h]` into UIKit points.
///
/// Enforces finite non-negative dimensions and at least `2.0 x 18.0` size when
/// zero-sized. Falls back inside `view_bounds` when `caret_rect` is `None` or invalid.
#[must_use]
pub fn caret_rect_or_fallback(caret_rect: Option<[f32; 4]>, view_bounds: [f32; 4]) -> [f32; 4] {
    if let Some([x, y, w, h]) = caret_rect
        && x.is_finite()
        && y.is_finite()
        && w.is_finite()
        && h.is_finite()
        && w >= 0.0
        && h >= 0.0
    {
        return [x, y, w.max(MIN_CARET_WIDTH_PTS), h.max(MIN_CARET_HEIGHT_PTS)];
    }

    let [vx, vy, vw, vh] = view_bounds;
    if vx.is_finite() && vy.is_finite() && vw.is_finite() && vh.is_finite() && vw >= 0.0 && vh >= 0.0 {
        let fx = vx + vw * 0.5;
        let fy = vy + vh * 0.25;
        if fx.is_finite() && fy.is_finite() {
            return [fx, fy, MIN_CARET_WIDTH_PTS, MIN_CARET_HEIGHT_PTS];
        }
    }

    [0.0, 0.0, MIN_CARET_WIDTH_PTS, MIN_CARET_HEIGHT_PTS]
}

/// Computes an expanded Scribble target rect around `caret_rect` only in Write mode.
///
/// Returns `None` when `pointer_is_write` is `false` (so Apple Pencil always inks directly
/// on the canvas) or when `caret_rect` is `None` or non-finite/negative.
#[must_use]
pub fn indirect_scribble_target_rect(caret_rect: Option<[f32; 4]>, pointer_is_write: bool) -> Option<[f32; 4]> {
    if !pointer_is_write {
        return None;
    }
    let [x, y, w, h] = caret_rect?;
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) || w < 0.0 || h < 0.0 {
        return None;
    }
    let safe_w = w.max(MIN_CARET_WIDTH_PTS);
    let safe_h = h.max(MIN_CARET_HEIGHT_PTS);
    let min_x = (x - SCRIBBLE_PAD_X_PTS).max(0.0);
    let min_y = (y - SCRIBBLE_PAD_Y_PTS).max(0.0);
    let max_x = x + safe_w + SCRIBBLE_PAD_X_PTS;
    let max_y = y + safe_h + SCRIBBLE_PAD_Y_PTS;
    let out_w = (max_x - min_x).max(MIN_CARET_WIDTH_PTS);
    let out_h = (max_y - min_y).max(MIN_CARET_HEIGHT_PTS);
    if min_x.is_finite() && min_y.is_finite() && out_w.is_finite() && out_h.is_finite() { Some([min_x, min_y, out_w, out_h]) } else { None }
}

/// Local UTF-16-addressable text window synchronized with `NocturneApp`'s active caret/selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextDocumentModel {
    text: String,
    selection: (usize, usize),
    marked: Option<(usize, usize)>,
    focused: bool,
    last_synced_selection: String,
    /// The text is the whole of an egui field ([`Self::sync_document`]), not the page's selection.
    mirrored: bool,
    /// The field and selection last mirrored, kept only when the field was longer than the
    /// window and had to be cut to fit it.
    mirror_cut_from: Option<(String, (usize, usize))>,
}

impl TextDocumentModel {
    /// Creates an empty, unfocused text document window.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the buffered text window as UTF-8.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the total length of the buffered text window in UTF-16 code units.
    #[must_use]
    pub fn utf16_len(&self) -> usize {
        utf16_len(&self.text)
    }

    /// Returns `true` when the buffered window contains at least one character.
    #[must_use]
    pub fn has_text(&self) -> bool {
        !self.text.is_empty()
    }

    /// Returns the ordered `(start, end)` selection range in UTF-16 code units.
    #[must_use]
    pub fn selection_utf16(&self) -> (usize, usize) {
        self.selection
    }

    /// Returns the active IME composition range in UTF-16 code units, if any.
    #[must_use]
    pub fn marked_range_utf16(&self) -> Option<(usize, usize)> {
        self.marked
    }

    /// Clamps a UTF-16 offset into `0..=self.utf16_len()` snapped to a scalar boundary.
    #[must_use]
    pub fn clamp_offset(&self, offset: usize) -> usize {
        let (_, snapped) = utf16_offset_to_byte_index(&self.text, offset);
        snapped
    }

    /// Updates the selection range, clamping and snapping to scalar boundaries.
    pub fn set_selection_utf16(&mut self, start: usize, end: usize) {
        let (lo, hi) = (start.min(end), start.max(end));
        let (_, snapped_lo) = utf16_offset_to_byte_index(&self.text, lo);
        let (_, snapped_hi) = utf16_offset_to_byte_index(&self.text, hi);
        self.selection = (snapped_lo, snapped_hi);
    }

    /// Returns the currently selected substring inside the local window.
    #[must_use]
    pub fn selected_text(&self) -> String {
        self.text_in_range(self.selection.0, self.selection.1)
    }

    /// Extracts text in the UTF-16 range `[start_utf16, end_utf16]`, safely clamping and
    /// snapping surrogate-pair boundaries so slicing never panics.
    #[must_use]
    pub fn text_in_range(&self, start_utf16: usize, end_utf16: usize) -> String {
        let (lo, hi) = (start_utf16.min(end_utf16), start_utf16.max(end_utf16));
        let (lo_byte, _) = utf16_offset_to_byte_index(&self.text, lo);
        let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, hi);
        if lo_byte <= hi_byte && hi_byte <= self.text.len() { self.text[lo_byte..hi_byte].to_owned() } else { String::new() }
    }

    /// Synchronizes external selection/focus state from `NocturneApp`.
    ///
    /// Returns `true` if the document text or selection changed from UIKit's perspective.
    pub fn sync_from_platform(&mut self, selected_text: &str, has_focus: bool, is_writable: bool) -> bool {
        if !has_focus || !is_writable {
            let changed = self.focused || !self.text.is_empty() || self.selection != (0, 0) || self.marked.is_some();
            self.text.clear();
            self.selection = (0, 0);
            self.marked = None;
            self.focused = false;
            self.mirrored = false;
            self.mirror_cut_from = None;
            self.last_synced_selection.clear();
            return changed;
        }

        // The keyboard moved from a field mirrored whole ([`Self::sync_document`]) to the page's
        // own caret without a blur in between: the field's text is not the page's.
        if self.mirrored {
            self.mirrored = false;
            self.mirror_cut_from = None;
            self.focused = false;
        }

        if !self.focused {
            self.focused = true;
            self.text = selected_text.to_owned();
            let len = utf16_len(&self.text);
            self.selection = (0, len);
            self.marked = None;
            self.last_synced_selection = selected_text.to_owned();
            self.enforce_window_bound();
            return true;
        }

        if self.marked.is_some() {
            return false;
        }

        if selected_text == self.last_synced_selection {
            return false;
        }

        let current_selected = self.selected_text();
        let mut changed = false;
        if current_selected != selected_text {
            if selected_text.is_empty() {
                if self.selection.0 != self.selection.1 {
                    let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, self.selection.0);
                    let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, self.selection.1);
                    if lo_byte < hi_byte && hi_byte <= self.text.len() {
                        self.text.replace_range(lo_byte..hi_byte, "");
                    }
                    self.selection = (snapped_lo, snapped_lo);
                    self.marked = None;
                    changed = true;
                }
            } else {
                self.text = selected_text.to_owned();
                let len = utf16_len(&self.text);
                self.selection = (0, len);
                self.marked = None;
                self.enforce_window_bound();
                changed = true;
            }
        }
        self.last_synced_selection = selected_text.to_owned();
        changed
    }

    /// Mirrors the whole of a focused one-line field that egui owns (a name in a dialog, the page
    /// title, search): all of its text, and its selection given in characters.
    ///
    /// Returns `true` when UIKit must be told the selection changed, which is only when the app
    /// changed it: the field has just taken the keyboard, or its text or selection are no longer
    /// what the keyboard's own edits left. Typing never is. UIKit edits this model first
    /// (`insertText:`, `deleteBackward`), the app applies the same edit a frame later, and the
    /// two then agree. Telling UIKit the selection changed after every letter makes it throw
    /// away its suggestions and autocorrection.
    ///
    /// While `pending_commands` is true the app has not yet applied what the keyboard did, so
    /// its text is the older one and is not taken. Marked text is the keyboard's and is left alone.
    pub fn sync_document(&mut self, text: &str, selection_chars: (usize, usize), pending_commands: bool) -> bool {
        let to_utf16 = |chars: usize| text.chars().take(chars).map(char::len_utf16).sum::<usize>();
        let (a, b) = (to_utf16(selection_chars.0), to_utf16(selection_chars.1));
        let selection = (a.min(b), a.max(b));
        if self.focused && self.mirrored {
            if self.marked.is_some() || pending_commands {
                return false;
            }
            if self.text == text && self.selection == selection {
                self.mirror_cut_from = None;
                return false;
            }
            // A field longer than the window is cut to it below; the same field next frame is not news.
            if self.mirror_cut_from.as_ref().is_some_and(|(seen, at)| seen == text && *at == selection) {
                return false;
            }
        }
        self.focused = true;
        self.mirrored = true;
        self.text = text.to_owned();
        self.selection = selection;
        self.marked = None;
        self.last_synced_selection.clear();
        self.enforce_window_bound();
        self.mirror_cut_from = (self.text != text).then(|| (text.to_owned(), selection));
        true
    }

    /// Synchronizes local buffer state when an edit action (`Cut`, `Delete`, `SelectAll`) is queued.
    pub fn apply_edit_action(&mut self, action: TextEditAction) {
        match action {
            TextEditAction::Cut => {
                let (sel_lo, sel_hi) = self.selection;
                if sel_lo < sel_hi {
                    let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, sel_lo);
                    let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, sel_hi);
                    self.text.replace_range(lo_byte..hi_byte, "");
                    self.selection = (snapped_lo, snapped_lo);
                    self.marked = None;
                    self.last_synced_selection.clear();
                }
            }
            TextEditAction::Delete => {
                let (sel_lo, sel_hi) = self.selection;
                if sel_lo < sel_hi {
                    let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, sel_lo);
                    let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, sel_hi);
                    self.text.replace_range(lo_byte..hi_byte, "");
                    self.selection = (snapped_lo, snapped_lo);
                } else {
                    let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, sel_lo);
                    if let Some(ch) = self.text[lo_byte..].chars().next() {
                        self.text.replace_range(lo_byte..lo_byte + ch.len_utf8(), "");
                        self.selection = (snapped_lo, snapped_lo);
                    }
                }
                self.marked = None;
                self.last_synced_selection.clear();
            }
            TextEditAction::SelectAll => {
                let end = self.utf16_len();
                self.selection = (0, end);
                self.marked = None;
            }
            TextEditAction::Copy | TextEditAction::Paste => {}
        }
    }

    /// Applies a UIKit `insertText:` commit to the local window and returns platform commands.
    pub fn insert_text(&mut self, text: &str) -> Vec<TextEditCommand> {
        let (lo_u16, hi_u16) = self.marked.take().unwrap_or(self.selection);
        let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, lo_u16);
        let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, hi_u16);
        self.text.replace_range(lo_byte..hi_byte, text);
        let caret = snapped_lo + utf16_len(text);
        self.selection = (caret, caret);
        self.enforce_window_bound();
        self.last_synced_selection.clear();
        split_insert_text_commands(text)
    }

    /// Applies a UIKit `deleteBackward` action to the local window and returns platform commands.
    ///
    /// Cancelling/deleting an uncommitted IME `marked` composition clears the local marked
    /// span and emits [`TextEditCommand::UnmarkText`] (clearing `egui` preedit without
    /// emitting `DeleteBackward` into `NocturneApp`).
    pub fn delete_backward(&mut self) -> Vec<TextEditCommand> {
        if let Some((m_lo, m_hi)) = self.marked.take() {
            let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, m_lo);
            let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, m_hi);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.selection = (snapped_lo, snapped_lo);
            self.last_synced_selection.clear();
            return vec![TextEditCommand::UnmarkText];
        }

        let (sel_lo, sel_hi) = self.selection;
        if sel_lo < sel_hi {
            let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, sel_lo);
            let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, sel_hi);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.selection = (snapped_lo, snapped_lo);
        } else if sel_lo > 0 {
            let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, sel_lo);
            if let Some((prev_byte, _ch)) = self.text[..hi_byte].char_indices().next_back() {
                let new_caret = utf16_len(&self.text[..prev_byte]);
                self.text.replace_range(prev_byte..hi_byte, "");
                self.selection = (new_caret, new_caret);
            }
        }
        self.last_synced_selection.clear();
        vec![TextEditCommand::DeleteBackward]
    }

    /// Replaces `[start_utf16, end_utf16]` with `replacement`, emitting cursor moves and
    /// backward/forward deletes as needed so arbitrary non-caret ranges stay synchronized.
    pub fn replace_range(&mut self, start_utf16: usize, end_utf16: usize, replacement: &str) -> Vec<TextEditCommand> {
        let (raw_lo, raw_hi) = (start_utf16.min(end_utf16), start_utf16.max(end_utf16));
        let mut prefix_commands = Vec::new();
        let (adj_lo, adj_hi) = if let Some((raw_m_lo, raw_m_hi)) = self.marked.take() {
            let (m_lo_u16, m_hi_u16) = (raw_m_lo.min(raw_m_hi), raw_m_lo.max(raw_m_hi));
            let (m_lo_byte, m_snapped_lo) = utf16_offset_to_byte_index(&self.text, m_lo_u16);
            let (m_hi_byte, m_snapped_hi) = utf16_offset_to_byte_index(&self.text, m_hi_u16);
            let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, raw_lo);
            let (hi_byte, snapped_hi) = utf16_offset_to_byte_index(&self.text, raw_hi);
            self.last_synced_selection.clear();
            if lo_byte <= m_lo_byte && hi_byte >= m_hi_byte {
                // Case A: replacement equals or covers the active marked range.
                // Cancel the uncommitted marked span first, then apply the replacement to committed text.
                self.text.replace_range(m_lo_byte..m_hi_byte, "");
                self.selection = (m_snapped_lo, m_snapped_lo);
                prefix_commands.push(TextEditCommand::UnmarkText);
                let removed_u16 = m_snapped_hi.saturating_sub(m_snapped_lo);
                let next_lo = snapped_lo.min(m_snapped_lo);
                let next_hi = snapped_hi.saturating_sub(removed_u16).max(m_snapped_lo);
                (next_lo, next_hi)
            } else {
                // Case B: disjoint or partially overlapping marked range.
                // Commit the marked text first so platform and local document share the full string.
                let marked_text = self.text[m_lo_byte..m_hi_byte].to_owned();
                if marked_text.is_empty() {
                    prefix_commands.push(TextEditCommand::UnmarkText);
                } else {
                    prefix_commands.push(TextEditCommand::CommitText(marked_text));
                }
                self.selection = (m_snapped_hi, m_snapped_hi);
                (snapped_lo, snapped_hi)
            }
        } else {
            (raw_lo, raw_hi)
        };

        let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, adj_lo);
        let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, adj_hi);
        let deleted_chars = self.text[lo_byte..hi_byte].chars().count();
        let (raw_sel_lo, raw_sel_hi) = (self.selection.0.min(self.selection.1), self.selection.0.max(self.selection.1));
        let (sel_lo_byte, _) = utf16_offset_to_byte_index(&self.text, raw_sel_lo);
        let (sel_hi_byte, _) = utf16_offset_to_byte_index(&self.text, raw_sel_hi);
        let has_selection = sel_lo_byte < sel_hi_byte;

        let mut commands = prefix_commands;
        if lo_byte == sel_lo_byte && hi_byte == sel_hi_byte {
            // Case 1: exact match with active selection or caret.
            if replacement.is_empty() {
                if deleted_chars > 0 {
                    commands.push(TextEditCommand::DeleteBackward);
                }
            } else {
                if has_selection && replacement.starts_with(['\r', '\n']) {
                    commands.push(TextEditCommand::DeleteBackward);
                }
                commands.extend(split_insert_text_commands(replacement));
            }
        } else if lo_byte <= sel_lo_byte && hi_byte >= sel_hi_byte {
            // Case 2: replacement range is a strict superset of selection (`lo <= sel_lo <= sel_hi <= hi`).
            if has_selection {
                commands.push(TextEditCommand::DeleteBackward);
            }
            let suffix_chars = self.text[sel_hi_byte..hi_byte].chars().count();
            for _ in 0..suffix_chars {
                commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
            }
            let prefix_chars = self.text[lo_byte..sel_lo_byte].chars().count();
            for _ in 0..prefix_chars {
                commands.push(TextEditCommand::DeleteBackward);
            }
            commands.extend(split_insert_text_commands(replacement));
        } else if lo_byte >= sel_hi_byte {
            // Case 3: range lies strictly at or after `sel_hi` (if non-empty selection, 1 `ArrowRight` collapses to `sel_hi`).
            let gap_chars = self.text[sel_hi_byte..lo_byte].chars().count();
            let steps = gap_chars.saturating_add(usize::from(has_selection));
            if let Ok(steps_i) = isize::try_from(steps)
                && steps_i > 0
            {
                commands.push(TextEditCommand::MoveCursor(steps_i));
            }
            for _ in 0..deleted_chars {
                commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
            }
            commands.extend(split_insert_text_commands(replacement));
        } else if hi_byte <= sel_lo_byte {
            // Case 4: range lies strictly at or before `sel_lo` (if non-empty selection, 1 `ArrowLeft` collapses to `sel_lo`).
            let gap_chars = self.text[hi_byte..sel_lo_byte].chars().count();
            let steps = gap_chars.saturating_add(usize::from(has_selection));
            if let Ok(steps_i) = isize::try_from(steps)
                && steps_i > 0
            {
                commands.push(TextEditCommand::MoveCursor(-steps_i));
            }
            for _ in 0..deleted_chars {
                commands.push(TextEditCommand::DeleteBackward);
            }
            commands.extend(split_insert_text_commands(replacement));
        } else if lo_byte <= sel_lo_byte && hi_byte < sel_hi_byte {
            // Case 5a: left overlap (`lo <= sel_lo < hi < sel_hi`).
            commands.push(TextEditCommand::MoveCursor(-1));
            let suffix_chars = self.text[sel_lo_byte..hi_byte].chars().count();
            for _ in 0..suffix_chars {
                commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
            }
            let prefix_chars = self.text[lo_byte..sel_lo_byte].chars().count();
            for _ in 0..prefix_chars {
                commands.push(TextEditCommand::DeleteBackward);
            }
            commands.extend(split_insert_text_commands(replacement));
        } else if lo_byte > sel_lo_byte && hi_byte >= sel_hi_byte {
            // Case 5b: right overlap (`sel_lo < lo < sel_hi <= hi`).
            commands.push(TextEditCommand::MoveCursor(1));
            let suffix_chars = self.text[sel_hi_byte..hi_byte].chars().count();
            for _ in 0..suffix_chars {
                commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
            }
            let prefix_chars = self.text[lo_byte..sel_hi_byte].chars().count();
            for _ in 0..prefix_chars {
                commands.push(TextEditCommand::DeleteBackward);
            }
            commands.extend(split_insert_text_commands(replacement));
        } else {
            // Case 5c: strictly interior to a non-empty selection (`sel_lo < lo <= hi < sel_hi`).
            commands.push(TextEditCommand::MoveCursor(-1));
            let offset_chars = self.text[sel_lo_byte..lo_byte].chars().count();
            if let Ok(steps_i) = isize::try_from(offset_chars)
                && steps_i > 0
            {
                commands.push(TextEditCommand::MoveCursor(steps_i));
            }
            for _ in 0..deleted_chars {
                commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
            }
            commands.extend(split_insert_text_commands(replacement));
        }

        self.text.replace_range(lo_byte..hi_byte, replacement);
        self.marked = None;
        let new_caret = snapped_lo + utf16_len(replacement);
        self.selection = (new_caret, new_caret);
        self.enforce_window_bound();
        self.last_synced_selection.clear();
        commands
    }

    /// Sets or updates IME marked composition text and relative UTF-16 selection.
    pub fn set_marked_text(&mut self, marked_text: Option<&str>, selected_relative_utf16: (usize, usize)) -> TextEditCommand {
        let replacement = marked_text.unwrap_or("");
        let (lo_u16, hi_u16) = self.marked.unwrap_or(self.selection);
        let (lo_byte, snapped_lo) = utf16_offset_to_byte_index(&self.text, lo_u16);
        let (hi_byte, _) = utf16_offset_to_byte_index(&self.text, hi_u16);
        self.text.replace_range(lo_byte..hi_byte, replacement);

        let marked_len_u16 = utf16_len(replacement);
        let rel_lo = selected_relative_utf16.0.min(marked_len_u16);
        let rel_hi = selected_relative_utf16.1.min(marked_len_u16);
        let (rel_lo, rel_hi) = (rel_lo.min(rel_hi), rel_lo.max(rel_hi));

        if marked_len_u16 == 0 {
            self.marked = None;
            self.selection = (snapped_lo, snapped_lo);
        } else {
            self.marked = Some((snapped_lo, snapped_lo + marked_len_u16));
            self.selection = (snapped_lo + rel_lo, snapped_lo + rel_hi);
        }
        self.enforce_window_bound();
        self.last_synced_selection.clear();

        TextEditCommand::SetMarkedText { text: replacement.to_owned(), selected_utf16: (rel_lo, rel_hi) }
    }

    /// Clears the active IME marked composition range, placing the caret at its end and
    /// emitting [`TextEditCommand::CommitText`] for the finalized marked text.
    pub fn unmark_text(&mut self) -> Option<TextEditCommand> {
        let (m_lo, m_hi) = self.marked.take()?;
        let committed = self.text_in_range(m_lo, m_hi);
        let clamped_hi = self.clamp_offset(m_hi);
        self.selection = (clamped_hi, clamped_hi);
        self.last_synced_selection.clear();
        if committed.is_empty() { Some(TextEditCommand::UnmarkText) } else { Some(TextEditCommand::CommitText(committed)) }
    }

    /// Computes a new UTF-16 position by adding `delta` to `origin`, returning `None` if out of bounds.
    #[must_use]
    pub fn position_from_offset(&self, origin: usize, delta: isize) -> Option<usize> {
        let len = self.utf16_len();
        let clamped_origin = origin.min(len);
        let target = isize::try_from(clamped_origin).ok()?.checked_add(delta)?;
        if target < 0 {
            return None;
        }
        let target_usize = usize::try_from(target).ok()?;
        (target_usize <= len).then_some(target_usize)
    }

    /// Compares two UTF-16 document offsets.
    #[must_use]
    pub fn compare_positions(a: usize, b: usize) -> Ordering {
        a.cmp(&b)
    }

    /// Returns signed UTF-16 offset `to - from`.
    #[must_use]
    pub fn offset_between(from: usize, to: usize) -> isize {
        let from_i = isize::try_from(from).unwrap_or(isize::MAX);
        let to_i = isize::try_from(to).unwrap_or(isize::MAX);
        to_i.saturating_sub(from_i)
    }

    fn enforce_window_bound(&mut self) {
        let total = utf16_len(&self.text);
        if total <= MAX_WINDOW_UTF16 {
            return;
        }
        let mut excess = total - MAX_WINDOW_UTF16;
        let (sel_lo, sel_hi) = self.selection;
        let (keep_lo, keep_hi) = match self.marked {
            Some((m_lo, m_hi)) => (sel_lo.min(m_lo), sel_hi.max(m_hi)),
            None => (sel_lo, sel_hi),
        };
        let front_slack = keep_lo.min(total);
        let back_slack = total.saturating_sub(keep_hi);

        if front_slack < excess || back_slack > front_slack {
            let target_len_u16 = total - excess;
            if keep_hi <= target_len_u16 {
                let mut cut_u16 = 0usize;
                let mut keep_bytes = 0usize;
                for (byte_idx, ch) in self.text.char_indices() {
                    let next_u16 = cut_u16 + ch.len_utf16();
                    if next_u16 > target_len_u16 {
                        break;
                    }
                    cut_u16 = next_u16;
                    keep_bytes = byte_idx + ch.len_utf8();
                }
                self.text.truncate(keep_bytes);
                let new_len = utf16_len(&self.text);
                self.selection = (self.selection.0.min(new_len), self.selection.1.min(new_len));
                self.marked = self.marked.and_then(|(m_lo, m_hi)| {
                    let lo = m_lo.min(new_len);
                    let hi = m_hi.min(new_len);
                    (lo < hi).then_some((lo, hi))
                });
                return;
            } else if back_slack > 0 {
                let (hi_byte, snapped_hi) = utf16_offset_to_byte_index(&self.text, keep_hi);
                self.text.truncate(hi_byte);
                let trimmed_tail = total.saturating_sub(snapped_hi);
                excess = excess.saturating_sub(trimmed_tail);
                if excess == 0 {
                    return;
                }
            }
        }

        let mut dropped_u16 = 0usize;
        let mut split_byte = 0usize;
        for (byte_idx, ch) in self.text.char_indices() {
            if dropped_u16 >= excess {
                split_byte = byte_idx;
                break;
            }
            dropped_u16 += ch.len_utf16();
            split_byte = byte_idx + ch.len_utf8();
        }
        if split_byte > 0 && split_byte <= self.text.len() {
            self.text.drain(..split_byte);
            self.selection = (self.selection.0.saturating_sub(dropped_u16), self.selection.1.saturating_sub(dropped_u16));
            self.marked = self.marked.and_then(|(m_lo, m_hi)| {
                let next_lo = m_lo.saturating_sub(dropped_u16);
                let next_hi = m_hi.saturating_sub(dropped_u16);
                (next_lo < next_hi).then_some((next_lo, next_hi))
            });
        }
    }
}

#[cfg(target_os = "ios")]
pub mod ios {
    use super::{TextDocumentModel, TextEditAction, TextEditCommand, caret_rect_or_fallback, indirect_scribble_target_rect};
    use block2::DynBlock;
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
    use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
    use objc2_core_foundation::{CGAffineTransform, CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSArray, NSAttributedStringKey, NSComparisonResult, NSDictionary, NSInteger, NSRange, NSString, ns_string};
    use objc2_ui_kit::{
        NSWritingDirection, UIEvent, UIIndirectScribbleInteraction, UIIndirectScribbleInteractionDelegate, UIKeyInput, UIPasteboard, UIResponder,
        UIScribbleElementIdentifier, UITextInput, UITextInputDelegate, UITextInputStringTokenizer, UITextInputTokenizer, UITextInputTraits,
        UITextLayoutDirection, UITextPosition, UITextRange, UITextSelectionRect, UITextStorageDirection, UIView, UIViewAutoresizing,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::rc::Rc;

    const SCRIBBLE_ELEMENT_ID: &str = "nocturne-active-text";

    fn to_cg_rect(r: [f32; 4]) -> CGRect {
        CGRect::new(CGPoint::new(f64::from(r[0]), f64::from(r[1])), CGSize::new(f64::from(r[2]), f64::from(r[3])))
    }

    fn cg_rect_intersects(a: CGRect, b: [f32; 4]) -> bool {
        let a_min_x = a.origin.x;
        let a_min_y = a.origin.y;
        let a_max_x = a.origin.x + a.size.width;
        let a_max_y = a.origin.y + a.size.height;
        let b_min_x = f64::from(b[0]);
        let b_min_y = f64::from(b[1]);
        let b_max_x = b_min_x + f64::from(b[2]);
        let b_max_y = b_min_y + f64::from(b[3]);
        a_min_x <= b_max_x && a_max_x >= b_min_x && a_min_y <= b_max_y && a_max_y >= b_min_y
    }

    struct TextInputBridgeState {
        document: TextDocumentModel,
        commands: Vec<TextEditCommand>,
        should_focus: bool,
        is_writable: bool,
        pointer_is_write: bool,
        caret_rect: Option<[f32; 4]>,
        view_bounds: [f32; 4],
        last_selected_text: String,
        writing_direction: NSWritingDirection,
    }

    impl Default for TextInputBridgeState {
        fn default() -> Self {
            Self {
                document: TextDocumentModel::new(),
                commands: Vec::new(),
                should_focus: false,
                is_writable: false,
                pointer_is_write: true,
                caret_rect: None,
                view_bounds: [0.0, 0.0, 1024.0, 768.0],
                last_selected_text: String::new(),
                writing_direction: NSWritingDirection::Natural,
            }
        }
    }

    #[allow(unsafe_code)]
    // SAFETY: All classes below (`NocturneTextPosition`, `NocturneTextRange`, `NocturneTextSelectionRect`,
    // `NocturneTextInputView`, `NocturneScribbleDelegate`) are `MainThreadOnly` UIKit subclasses without
    // custom `Drop`; `RefCell` borrows are transient and confined to the UIKit main thread.
    mod classes {
        use super::*;

        fn extract_position(pos: &UITextPosition) -> Option<usize> {
            let obj: &AnyObject = pos.as_super().as_super();
            obj.downcast_ref::<NocturneTextPosition>().map(NocturneTextPosition::offset)
        }

        fn extract_range(range: &UITextRange) -> Option<(usize, usize)> {
            let obj: &AnyObject = range.as_super().as_super();
            if let Some(r) = obj.downcast_ref::<NocturneTextRange>() {
                return Some((r.start_offset(), r.end_offset()));
            }
            let start = extract_position(&range.start())?;
            let end = extract_position(&range.end())?;
            Some((start.min(end), start.max(end)))
        }

        struct TextPositionIvars {
            offset: usize,
        }

        // SAFETY: `UITextPosition` inherits `NSObject` (`MainThreadOnly`) with no extra subclass invariants,
        // and `NocturneTextPosition` does not implement `Drop`.
        define_class!(
            #[unsafe(super(UITextPosition, NSObject))]
            #[ivars = TextPositionIvars]
            struct NocturneTextPosition;

            unsafe impl NSObjectProtocol for NocturneTextPosition {}
        );

        impl NocturneTextPosition {
            fn new(offset: usize, mtm: MainThreadMarker) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(TextPositionIvars { offset });
                // SAFETY: `UITextPosition::init` is the designated initializer of superclass `UITextPosition`.
                unsafe { msg_send![super(this), init] }
            }

            fn offset(&self) -> usize {
                self.ivars().offset
            }
        }

        struct TextRangeIvars {
            start: usize,
            end: usize,
        }

        // SAFETY: `UITextRange` inherits `NSObject` (`MainThreadOnly`) and requires overriding
        // `isEmpty`, `start`, and `end`; `NocturneTextRange` does not implement `Drop`.
        define_class!(
            #[unsafe(super(UITextRange, NSObject))]
            #[ivars = TextRangeIvars]
            struct NocturneTextRange;

            impl NocturneTextRange {
                #[unsafe(method(isEmpty))]
                fn __is_empty(&self) -> bool {
                    self.ivars().start >= self.ivars().end
                }

                #[unsafe(method_id(start))]
                fn __start(&self) -> Retained<UITextPosition> {
                    let mtm = MainThreadMarker::from(self);
                    NocturneTextPosition::new(self.ivars().start, mtm).into_super()
                }

                #[unsafe(method_id(end))]
                fn __end(&self) -> Retained<UITextPosition> {
                    let mtm = MainThreadMarker::from(self);
                    NocturneTextPosition::new(self.ivars().end, mtm).into_super()
                }
            }

            unsafe impl NSObjectProtocol for NocturneTextRange {}
        );

        impl NocturneTextRange {
            fn new(start: usize, end: usize, mtm: MainThreadMarker) -> Retained<Self> {
                let (lo, hi) = (start.min(end), start.max(end));
                let this = Self::alloc(mtm).set_ivars(TextRangeIvars { start: lo, end: hi });
                // SAFETY: `UITextRange::init` is the designated initializer of superclass `UITextRange`.
                unsafe { msg_send![super(this), init] }
            }

            fn start_offset(&self) -> usize {
                self.ivars().start
            }

            fn end_offset(&self) -> usize {
                self.ivars().end
            }
        }

        struct TextSelectionRectIvars {
            rect: CGRect,
            writing_direction: NSWritingDirection,
            contains_start: bool,
            contains_end: bool,
            is_vertical: bool,
        }

        // SAFETY: `UITextSelectionRect` inherits `NSObject` (`MainThreadOnly`) with read-only geometry
        // properties; `NocturneTextSelectionRect` does not implement `Drop`.
        define_class!(
            #[unsafe(super(UITextSelectionRect, NSObject))]
            #[ivars = TextSelectionRectIvars]
            struct NocturneTextSelectionRect;

            impl NocturneTextSelectionRect {
                #[unsafe(method(rect))]
                fn __rect(&self) -> CGRect {
                    self.ivars().rect
                }

                #[unsafe(method(writingDirection))]
                fn __writing_direction(&self) -> NSWritingDirection {
                    self.ivars().writing_direction
                }

                #[unsafe(method(containsStart))]
                fn __contains_start(&self) -> bool {
                    self.ivars().contains_start
                }

                #[unsafe(method(containsEnd))]
                fn __contains_end(&self) -> bool {
                    self.ivars().contains_end
                }

                #[unsafe(method(isVertical))]
                fn __is_vertical(&self) -> bool {
                    self.ivars().is_vertical
                }

                #[unsafe(method(transform))]
                fn __transform(&self) -> CGAffineTransform {
                    CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 }
                }
            }

            unsafe impl NSObjectProtocol for NocturneTextSelectionRect {}
        );

        impl NocturneTextSelectionRect {
            fn new(
                rect: CGRect,
                writing_direction: NSWritingDirection,
                contains_start: bool,
                contains_end: bool,
                is_vertical: bool,
                mtm: MainThreadMarker,
            ) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(TextSelectionRectIvars { rect, writing_direction, contains_start, contains_end, is_vertical });
                // SAFETY: `UITextSelectionRect::init` is the designated initializer of `UITextSelectionRect`.
                unsafe { msg_send![super(this), init] }
            }
        }

        pub(super) struct TextInputViewIvars {
            state: Rc<RefCell<TextInputBridgeState>>,
            tokenizer: RefCell<Option<Retained<ProtocolObject<dyn UITextInputTokenizer>>>>,
            input_delegate: RefCell<Option<Weak<ProtocolObject<dyn UITextInputDelegate>>>>,
            marked_text_style: RefCell<Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>>>,
            /// Keyboard, IME and edit-menu calls land on this view, not on winit's, so no winit
            /// event follows them; this wakes eframe for the frame that drains their commands.
            egui_ctx: egui::Context,
        }

        // SAFETY: `NocturneTextInputView` subclasses `UIView` (`MainThreadOnly`), uses `RefCell` only
        // from the UIKit main thread, implements all required `UIKeyInput` and `UITextInput` methods,
        // and does not implement `Drop`.
        define_class!(
            #[unsafe(super(UIView, UIResponder, NSObject))]
            #[ivars = TextInputViewIvars]
            pub(super) struct NocturneTextInputView;

            impl NocturneTextInputView {
                #[unsafe(method(canBecomeFirstResponder))]
                fn __can_become_first_responder(&self) -> bool {
                    true
                }

                #[unsafe(method(pointInside:withEvent:))]
                fn __point_inside_with_event(&self, _point: CGPoint, _event: Option<&UIEvent>) -> bool {
                    false
                }

                #[unsafe(method(canPerformAction:withSender:))]
                fn __can_perform_action_with_sender(&self, action: Sel, _sender: Option<&AnyObject>) -> bool {
                    self.can_perform_action(action)
                }

                #[unsafe(method(cut:))]
                fn __cut(&self, _sender: Option<&AnyObject>) {
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        state.document.apply_edit_action(TextEditAction::Cut);
                        state.commands.push(TextEditCommand::EditAction(TextEditAction::Cut));
                    }
                    self.wake();
                }

                #[unsafe(method(copy:))]
                fn __copy(&self, _sender: Option<&AnyObject>) {
                    self.ivars().state.borrow_mut().commands.push(TextEditCommand::EditAction(TextEditAction::Copy));
                    self.wake();
                }

                #[unsafe(method(paste:))]
                fn __paste(&self, _sender: Option<&AnyObject>) {
                    self.ivars().state.borrow_mut().commands.push(TextEditCommand::EditAction(TextEditAction::Paste));
                    self.wake();
                }

                #[unsafe(method(selectAll:))]
                fn __select_all(&self, _sender: Option<&AnyObject>) {
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        state.document.apply_edit_action(TextEditAction::SelectAll);
                        state.commands.push(TextEditCommand::EditAction(TextEditAction::SelectAll));
                    }
                    self.wake();
                }

                #[unsafe(method(delete:))]
                fn __delete(&self, _sender: Option<&AnyObject>) {
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        state.document.apply_edit_action(TextEditAction::Delete);
                        state.commands.push(TextEditCommand::EditAction(TextEditAction::Delete));
                    }
                    self.wake();
                }
            }

            unsafe impl NSObjectProtocol for NocturneTextInputView {}

            unsafe impl UITextInputTraits for NocturneTextInputView {}

            unsafe impl UIKeyInput for NocturneTextInputView {
                #[unsafe(method(hasText))]
                fn __has_text(&self) -> bool {
                    self.ivars().state.borrow().document.has_text()
                }

                #[unsafe(method(insertText:))]
                fn __insert_text(&self, text: &NSString) {
                    let raw = text.to_string();
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        let cmds = state.document.insert_text(&raw);
                        state.commands.extend(cmds);
                    }
                    self.wake();
                }

                #[unsafe(method(deleteBackward))]
                fn __delete_backward(&self) {
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        let cmds = state.document.delete_backward();
                        state.commands.extend(cmds);
                    }
                    self.wake();
                }
            }

            unsafe impl UITextInput for NocturneTextInputView {
                #[unsafe(method_id(textInRange:))]
                fn __text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
                    self.text_in_range(range)
                }

                #[unsafe(method(replaceRange:withText:))]
                fn __replace_range_with_text(&self, range: &UITextRange, text: &NSString) {
                    let Some((start, end)) = extract_range(range) else {
                        return;
                    };
                    let replacement = text.to_string();
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        let cmds = state.document.replace_range(start, end, &replacement);
                        state.commands.extend(cmds);
                    }
                    self.wake();
                }

                #[unsafe(method_id(selectedTextRange))]
                fn __selected_text_range(&self) -> Option<Retained<UITextRange>> {
                    let mtm = MainThreadMarker::from(self);
                    let (start, end) = self.ivars().state.borrow().document.selection_utf16();
                    Some(NocturneTextRange::new(start, end, mtm).into_super())
                }

                #[unsafe(method(setSelectedTextRange:))]
                fn __set_selected_text_range(&self, selected_text_range: Option<&UITextRange>) {
                    let extracted = selected_text_range.and_then(extract_range);
                    let mut state = self.ivars().state.borrow_mut();
                    if let Some((start, end)) = extracted {
                        state.document.set_selection_utf16(start, end);
                    } else {
                        let end = state.document.utf16_len();
                        state.document.set_selection_utf16(end, end);
                    }
                }

                #[unsafe(method_id(markedTextRange))]
                fn __marked_text_range(&self) -> Option<Retained<UITextRange>> {
                    let mtm = MainThreadMarker::from(self);
                    self.ivars()
                        .state
                        .borrow()
                        .document
                        .marked_range_utf16()
                        .map(|(start, end)| NocturneTextRange::new(start, end, mtm).into_super())
                }

                #[unsafe(method_id(markedTextStyle))]
                fn __marked_text_style(&self) -> Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>> {
                    self.ivars().marked_text_style.borrow().clone()
                }

                #[unsafe(method(setMarkedTextStyle:))]
                unsafe fn __set_marked_text_style(&self, marked_text_style: Option<&NSDictionary<NSAttributedStringKey, AnyObject>>) {
                    *self.ivars().marked_text_style.borrow_mut() = marked_text_style.map(|dict| dict.retain());
                }

                #[unsafe(method(setMarkedText:selectedRange:))]
                fn __set_marked_text_selected_range(&self, marked_text: Option<&NSString>, selected_range: NSRange) {
                    let owned = marked_text.map(|s| s.to_string());
                    let sel_end = selected_range.location.saturating_add(selected_range.length);
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        let cmd = state.document.set_marked_text(owned.as_deref(), (selected_range.location, sel_end));
                        state.commands.push(cmd);
                    }
                    self.wake();
                }

                #[unsafe(method(unmarkText))]
                fn __unmark_text(&self) {
                    {
                        let mut state = self.ivars().state.borrow_mut();
                        if let Some(cmd) = state.document.unmark_text() {
                            state.commands.push(cmd);
                        }
                    }
                    self.wake();
                }

                #[unsafe(method_id(beginningOfDocument))]
                fn __beginning_of_document(&self) -> Retained<UITextPosition> {
                    let mtm = MainThreadMarker::from(self);
                    NocturneTextPosition::new(0, mtm).into_super()
                }

                #[unsafe(method_id(endOfDocument))]
                fn __end_of_document(&self) -> Retained<UITextPosition> {
                    let mtm = MainThreadMarker::from(self);
                    let len = self.ivars().state.borrow().document.utf16_len();
                    NocturneTextPosition::new(len, mtm).into_super()
                }

                #[unsafe(method_id(textRangeFromPosition:toPosition:))]
                fn __text_range_from_position_to_position(
                    &self,
                    from_position: &UITextPosition,
                    to_position: &UITextPosition,
                ) -> Option<Retained<UITextRange>> {
                    self.text_range_from_positions(from_position, to_position)
                }

                #[unsafe(method_id(positionFromPosition:offset:))]
                fn __position_from_position_offset(&self, position: &UITextPosition, offset: NSInteger) -> Option<Retained<UITextPosition>> {
                    self.position_from_offset(position, offset)
                }

                #[unsafe(method_id(positionFromPosition:inDirection:offset:))]
                fn __position_from_position_in_direction_offset(
                    &self,
                    position: &UITextPosition,
                    direction: UITextLayoutDirection,
                    offset: NSInteger,
                ) -> Option<Retained<UITextPosition>> {
                    self.position_in_direction(position, direction, offset)
                }

                #[unsafe(method(comparePosition:toPosition:))]
                fn __compare_position_to_position(&self, position: &UITextPosition, other: &UITextPosition) -> NSComparisonResult {
                    let a = extract_position(position).unwrap_or(0);
                    let b = extract_position(other).unwrap_or(0);
                    NSComparisonResult::from(TextDocumentModel::compare_positions(a, b))
                }

                #[unsafe(method(offsetFromPosition:toPosition:))]
                fn __offset_from_position_to_position(&self, from: &UITextPosition, to_position: &UITextPosition) -> NSInteger {
                    let a = extract_position(from).unwrap_or(0);
                    let b = extract_position(to_position).unwrap_or(0);
                    TextDocumentModel::offset_between(a, b)
                }

                #[unsafe(method_id(inputDelegate))]
                fn __input_delegate(&self) -> Option<Retained<ProtocolObject<dyn UITextInputDelegate>>> {
                    self.ivars().input_delegate.borrow().as_ref().and_then(Weak::load)
                }

                #[unsafe(method(setInputDelegate:))]
                fn __set_input_delegate(&self, input_delegate: Option<&ProtocolObject<dyn UITextInputDelegate>>) {
                    *self.ivars().input_delegate.borrow_mut() = input_delegate.map(Weak::new);
                }

                #[unsafe(method_id(tokenizer))]
                fn __tokenizer(&self) -> Retained<ProtocolObject<dyn UITextInputTokenizer>> {
                    self.ensure_tokenizer()
                }

                #[unsafe(method_id(positionWithinRange:farthestInDirection:))]
                fn __position_within_range_farthest_in_direction(
                    &self,
                    range: &UITextRange,
                    direction: UITextLayoutDirection,
                ) -> Option<Retained<UITextPosition>> {
                    self.position_within_range(range, direction)
                }

                #[unsafe(method_id(characterRangeByExtendingPosition:inDirection:))]
                fn __character_range_by_extending_position_in_direction(
                    &self,
                    position: &UITextPosition,
                    direction: UITextLayoutDirection,
                ) -> Option<Retained<UITextRange>> {
                    self.extend_position(position, direction)
                }

                #[unsafe(method(baseWritingDirectionForPosition:inDirection:))]
                fn __base_writing_direction_for_position_in_direction(
                    &self,
                    _position: &UITextPosition,
                    _direction: UITextStorageDirection,
                ) -> NSWritingDirection {
                    self.ivars().state.borrow().writing_direction
                }

                #[unsafe(method(setBaseWritingDirection:forRange:))]
                fn __set_base_writing_direction_for_range(&self, writing_direction: NSWritingDirection, _range: &UITextRange) {
                    self.ivars().state.borrow_mut().writing_direction = writing_direction;
                }

                #[unsafe(method(firstRectForRange:))]
                fn __first_rect_for_range(&self, _range: &UITextRange) -> CGRect {
                    let state = self.ivars().state.borrow();
                    to_cg_rect(caret_rect_or_fallback(state.caret_rect, state.view_bounds))
                }

                #[unsafe(method(caretRectForPosition:))]
                fn __caret_rect_for_position(&self, _position: &UITextPosition) -> CGRect {
                    let state = self.ivars().state.borrow();
                    to_cg_rect(caret_rect_or_fallback(state.caret_rect, state.view_bounds))
                }

                #[unsafe(method_id(selectionRectsForRange:))]
                fn __selection_rects_for_range(&self, range: &UITextRange) -> Retained<NSArray<UITextSelectionRect>> {
                    self.selection_rects(range)
                }

                #[unsafe(method_id(closestPositionToPoint:))]
                fn __closest_position_to_point(&self, _point: CGPoint) -> Option<Retained<UITextPosition>> {
                    let mtm = MainThreadMarker::from(self);
                    let (_, end) = self.ivars().state.borrow().document.selection_utf16();
                    Some(NocturneTextPosition::new(end, mtm).into_super())
                }

                #[unsafe(method_id(closestPositionToPoint:withinRange:))]
                fn __closest_position_to_point_within_range(&self, _point: CGPoint, range: &UITextRange) -> Option<Retained<UITextPosition>> {
                    self.closest_position_within_range(range)
                }

                #[unsafe(method_id(characterRangeAtPoint:))]
                fn __character_range_at_point(&self, _point: CGPoint) -> Option<Retained<UITextRange>> {
                    let mtm = MainThreadMarker::from(self);
                    let (start, end) = self.ivars().state.borrow().document.selection_utf16();
                    Some(NocturneTextRange::new(start, end, mtm).into_super())
                }

                #[unsafe(method_id(textInputView))]
                fn __text_input_view(&self) -> Retained<UIView> {
                    self.as_super().retain()
                }

                #[unsafe(method(isEditable))]
                fn __is_editable(&self) -> bool {
                    self.ivars().state.borrow().is_writable
                }
            }
        );

        impl NocturneTextInputView {
            pub(super) fn new(
                frame: CGRect,
                state: Rc<RefCell<TextInputBridgeState>>,
                egui_ctx: egui::Context,
                mtm: MainThreadMarker,
            ) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(TextInputViewIvars {
                    state,
                    tokenizer: RefCell::new(None),
                    input_delegate: RefCell::new(None),
                    marked_text_style: RefCell::new(None),
                    egui_ctx,
                });
                // SAFETY: `UIView::initWithFrame:` is the designated initializer of superclass `UIView`.
                let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
                view.setUserInteractionEnabled(false);
                view.setOpaque(false);
                view.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
                view
            }

            /// Asks eframe for the frame that drains the commands just queued. Called after the
            /// state borrow is released; `request_repaint` does not touch that state.
            fn wake(&self) {
                self.ivars().egui_ctx.request_repaint();
            }

            pub(super) fn notify_selection_changed(&self) {
                let delegate = self.ivars().input_delegate.borrow().as_ref().and_then(Weak::load);
                if let Some(delegate) = delegate {
                    let text_input: &ProtocolObject<dyn UITextInput> = ProtocolObject::from_ref(self);
                    delegate.selectionWillChange(Some(text_input));
                    delegate.selectionDidChange(Some(text_input));
                }
            }

            fn can_perform_action(&self, action: Sel) -> bool {
                let _mtm = MainThreadMarker::from(self);
                let state = self.ivars().state.borrow();
                if !state.should_focus {
                    return false;
                }
                let has_selection = !state.document.selected_text().is_empty() || !state.last_selected_text.is_empty();
                let is_writable = state.is_writable;
                let has_text = state.document.has_text() || !state.last_selected_text.is_empty();
                drop(state);

                if action == sel!(copy:) {
                    return has_selection;
                }
                if action == sel!(cut:) {
                    return is_writable && has_selection;
                }
                if action == sel!(paste:) {
                    if !is_writable {
                        return false;
                    }
                    let board = UIPasteboard::generalPasteboard();
                    let html_types = NSArray::from_slice(&[ns_string!("public.html")]);
                    // SAFETY: `MainThreadMarker::from(self)` above guarantees main-thread execution.
                    let has_str_or_img = unsafe { board.hasStrings() || board.hasImages() };
                    return has_str_or_img || board.containsPasteboardTypes(&html_types);
                }
                if action == sel!(selectAll:) {
                    return true;
                }
                if action == sel!(delete:) {
                    return is_writable && has_text;
                }
                false
            }

            fn text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
                let (start, end) = extract_range(range)?;
                let text = self.ivars().state.borrow().document.text_in_range(start, end);
                Some(NSString::from_str(&text))
            }

            fn text_range_from_positions(&self, from_position: &UITextPosition, to_position: &UITextPosition) -> Option<Retained<UITextRange>> {
                let mtm = MainThreadMarker::from(self);
                let from_off = extract_position(from_position)?;
                let to_off = extract_position(to_position)?;
                let doc = &self.ivars().state.borrow().document;
                let a = doc.clamp_offset(from_off);
                let b = doc.clamp_offset(to_off);
                Some(NocturneTextRange::new(a, b, mtm).into_super())
            }

            fn position_from_offset(&self, position: &UITextPosition, offset: NSInteger) -> Option<Retained<UITextPosition>> {
                let mtm = MainThreadMarker::from(self);
                let pos = extract_position(position)?;
                let next = self.ivars().state.borrow().document.position_from_offset(pos, offset)?;
                Some(NocturneTextPosition::new(next, mtm).into_super())
            }

            fn position_in_direction(
                &self,
                position: &UITextPosition,
                direction: UITextLayoutDirection,
                offset: NSInteger,
            ) -> Option<Retained<UITextPosition>> {
                let mtm = MainThreadMarker::from(self);
                let pos = extract_position(position)?;
                let state = self.ivars().state.borrow();
                let next = match direction {
                    UITextLayoutDirection::Right => state.document.position_from_offset(pos, offset)?,
                    UITextLayoutDirection::Left => state.document.position_from_offset(pos, offset.saturating_neg())?,
                    UITextLayoutDirection::Up => {
                        if offset == 0 {
                            state.document.clamp_offset(pos)
                        } else if offset > 0 {
                            0
                        } else {
                            state.document.utf16_len()
                        }
                    }
                    UITextLayoutDirection::Down => {
                        if offset == 0 {
                            state.document.clamp_offset(pos)
                        } else if offset > 0 {
                            state.document.utf16_len()
                        } else {
                            0
                        }
                    }
                    _ => return None,
                };
                Some(NocturneTextPosition::new(next, mtm).into_super())
            }

            fn ensure_tokenizer(&self) -> Retained<ProtocolObject<dyn UITextInputTokenizer>> {
                if let Some(existing) = self.ivars().tokenizer.borrow().as_ref() {
                    return existing.clone();
                }
                let mtm = MainThreadMarker::from(self);
                let responder: &UIResponder = self.as_super().as_super();
                // SAFETY: `NocturneTextInputView` conforms to `UITextInput` and inherits `UIResponder`.
                let created = unsafe { UITextInputStringTokenizer::initWithTextInput(mtm.alloc(), responder) };
                let proto = ProtocolObject::from_retained(created);
                *self.ivars().tokenizer.borrow_mut() = Some(proto.clone());
                proto
            }

            fn position_within_range(&self, range: &UITextRange, direction: UITextLayoutDirection) -> Option<Retained<UITextPosition>> {
                let mtm = MainThreadMarker::from(self);
                let (start, end) = extract_range(range)?;
                let (lo, hi) = (start.min(end), start.max(end));
                let offset = match direction {
                    UITextLayoutDirection::Left | UITextLayoutDirection::Up => lo,
                    _ => hi,
                };
                Some(NocturneTextPosition::new(offset, mtm).into_super())
            }

            fn extend_position(&self, position: &UITextPosition, direction: UITextLayoutDirection) -> Option<Retained<UITextRange>> {
                let mtm = MainThreadMarker::from(self);
                let pos = extract_position(position)?;
                let doc = &self.ivars().state.borrow().document;
                let clamped = doc.clamp_offset(pos);
                let len = doc.utf16_len();
                let (a, b) = match direction {
                    UITextLayoutDirection::Left | UITextLayoutDirection::Up => (0, clamped),
                    UITextLayoutDirection::Right | UITextLayoutDirection::Down => (clamped, len),
                    _ => (clamped, clamped),
                };
                Some(NocturneTextRange::new(a, b, mtm).into_super())
            }

            fn selection_rects(&self, range: &UITextRange) -> Retained<NSArray<UITextSelectionRect>> {
                let mtm = MainThreadMarker::from(self);
                let Some((start, end)) = extract_range(range) else {
                    return NSArray::new();
                };
                if start >= end {
                    return NSArray::new();
                }
                let state = self.ivars().state.borrow();
                let rect = to_cg_rect(caret_rect_or_fallback(state.caret_rect, state.view_bounds));
                let writing_dir = state.writing_direction;
                drop(state);
                let sel_rect = NocturneTextSelectionRect::new(rect, writing_dir, true, true, false, mtm);
                let base_rect: Retained<UITextSelectionRect> = sel_rect.into_super();
                NSArray::from_retained_slice(&[base_rect])
            }

            fn closest_position_within_range(&self, range: &UITextRange) -> Option<Retained<UITextPosition>> {
                let mtm = MainThreadMarker::from(self);
                let (start, end) = extract_range(range)?;
                let (_, sel_end) = self.ivars().state.borrow().document.selection_utf16();
                let clamped = sel_end.clamp(start.min(end), start.max(end));
                Some(NocturneTextPosition::new(clamped, mtm).into_super())
            }
        }

        pub(super) struct ScribbleDelegateIvars {
            state: Rc<RefCell<TextInputBridgeState>>,
            text_input_view: Retained<NocturneTextInputView>,
        }

        // SAFETY: `NocturneScribbleDelegate` inherits `NSObject` (`MainThreadOnly`), implements all
        // required `UIIndirectScribbleInteractionDelegate` methods, and does not implement `Drop`.
        define_class!(
            #[unsafe(super(NSObject))]
            #[thread_kind = MainThreadOnly]
            #[ivars = ScribbleDelegateIvars]
            pub(super) struct NocturneScribbleDelegate;

            unsafe impl NSObjectProtocol for NocturneScribbleDelegate {}

            unsafe impl UIIndirectScribbleInteractionDelegate for NocturneScribbleDelegate {
                #[unsafe(method(indirectScribbleInteraction:requestElementsInRect:completion:))]
                fn __request_elements_in_rect(
                    &self,
                    _interaction: &UIIndirectScribbleInteraction,
                    rect: CGRect,
                    completion: &DynBlock<dyn Fn(NonNull<NSArray<UIScribbleElementIdentifier>>)>,
                ) {
                    let state = self.ivars().state.borrow();
                    let target = if state.should_focus && state.is_writable {
                        indirect_scribble_target_rect(state.caret_rect, state.pointer_is_write)
                    } else {
                        None
                    };
                    drop(state);

                    if let Some(target_rect) = target
                        && cg_rect_intersects(rect, target_rect)
                    {
                        let ident = ns_string!(SCRIBBLE_ELEMENT_ID);
                        let obj: &UIScribbleElementIdentifier = ident.as_super().as_super();
                        let arr: Retained<NSArray<UIScribbleElementIdentifier>> = NSArray::from_slice(&[obj]);
                        completion.call((NonNull::from(&*arr),));
                        return;
                    }

                    let empty: Retained<NSArray<UIScribbleElementIdentifier>> = NSArray::new();
                    completion.call((NonNull::from(&*empty),));
                }

                #[unsafe(method(indirectScribbleInteraction:isElementFocused:))]
                unsafe fn __is_element_focused(
                    &self,
                    _interaction: &UIIndirectScribbleInteraction,
                    _element_identifier: &UIScribbleElementIdentifier,
                ) -> bool {
                    let state = self.ivars().state.borrow();
                    let active = state.should_focus && state.is_writable && state.pointer_is_write;
                    drop(state);
                    active && self.ivars().text_input_view.isFirstResponder()
                }

                #[unsafe(method(indirectScribbleInteraction:frameForElement:))]
                unsafe fn __frame_for_element(
                    &self,
                    _interaction: &UIIndirectScribbleInteraction,
                    _element_identifier: &UIScribbleElementIdentifier,
                ) -> CGRect {
                    let state = self.ivars().state.borrow();
                    let target = if state.should_focus && state.is_writable {
                        indirect_scribble_target_rect(state.caret_rect, state.pointer_is_write)
                    } else {
                        None
                    };
                    target.map_or(CGRect::ZERO, to_cg_rect)
                }

                #[unsafe(method(indirectScribbleInteraction:focusElementIfNeeded:referencePoint:completion:))]
                unsafe fn __focus_element_if_needed(
                    &self,
                    _interaction: &UIIndirectScribbleInteraction,
                    _element_identifier: &UIScribbleElementIdentifier,
                    _focus_reference_point: CGPoint,
                    completion: &DynBlock<dyn Fn(*mut UIResponder)>,
                ) {
                    let state = self.ivars().state.borrow();
                    let can_focus = state.should_focus && state.is_writable && state.pointer_is_write;
                    drop(state);
                    if !can_focus {
                        completion.call((std::ptr::null_mut(),));
                        return;
                    }
                    let view = &self.ivars().text_input_view;
                    if !view.isFirstResponder() {
                        let _ = view.becomeFirstResponder();
                    }
                    let responder: &UIResponder = view.as_super().as_super();
                    completion.call((NonNull::from(responder).as_ptr(),));
                }
            }
        );

        impl NocturneScribbleDelegate {
            pub(super) fn new(
                state: Rc<RefCell<TextInputBridgeState>>,
                text_input_view: Retained<NocturneTextInputView>,
                mtm: MainThreadMarker,
            ) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(ScribbleDelegateIvars { state, text_input_view });
                // SAFETY: `NSObject::init` is the designated initializer of superclass `NSObject`.
                unsafe { msg_send![super(this), init] }
            }
        }
    }

    use classes::{NocturneScribbleDelegate, NocturneTextInputView};

    /// Owns the transparent `NocturneTextInputView` responder and its `UIIndirectScribbleInteraction`.
    pub struct TextInputBridge {
        view: Retained<NocturneTextInputView>,
        scribble_interaction: Retained<UIIndirectScribbleInteraction>,
        _scribble_delegate: Retained<NocturneScribbleDelegate>,
        state: Rc<RefCell<TextInputBridgeState>>,
    }

    impl TextInputBridge {
        /// Attaches the non-touch-blocking `UITextInput` responder and Scribble interaction to `parent_view`.
        pub fn attach(parent_view: &UIView, mtm: MainThreadMarker, egui_ctx: &egui::Context) -> Self {
            let bounds = parent_view.bounds();
            let state = Rc::new(RefCell::new(TextInputBridgeState {
                view_bounds: [bounds.origin.x as f32, bounds.origin.y as f32, bounds.size.width as f32, bounds.size.height as f32],
                ..TextInputBridgeState::default()
            }));
            let view = NocturneTextInputView::new(bounds, Rc::clone(&state), egui_ctx.clone(), mtm);
            parent_view.addSubview(view.as_super());

            let scribble_delegate = NocturneScribbleDelegate::new(Rc::clone(&state), view.clone(), mtm);
            let scribble_interaction = UIIndirectScribbleInteraction::initWithDelegate(mtm.alloc(), ProtocolObject::from_ref(&*scribble_delegate));
            parent_view.addInteraction(ProtocolObject::from_ref(&*scribble_interaction));

            Self { view, scribble_interaction, _scribble_delegate: scribble_delegate, state }
        }

        /// Synchronizes focus, caret geometry, selected text, and tool mode from `NocturneApp`.
        ///
        /// `document` is the whole text and character selection of the egui field that has the
        /// keyboard, when one has ([`NocturneApp::focused_text_field`]); `pending_commands` says
        /// the app has not yet applied everything the keyboard did. With no `document` the
        /// page's own caret has the keyboard and only `selected_text` is mirrored, as before.
        #[allow(clippy::too_many_arguments)]
        pub fn sync_state(
            &self,
            should_focus: bool,
            caret_rect: Option<egui::Rect>,
            selected_text: &str,
            document: Option<(&str, (usize, usize))>,
            pending_commands: bool,
            pointer_is_write: bool,
            is_writable: bool,
        ) {
            if let Some(superview) = self.view.superview() {
                let bounds = superview.bounds();
                self.view.setFrame(bounds);
                let mut state = self.state.borrow_mut();
                state.view_bounds = [bounds.origin.x as f32, bounds.origin.y as f32, bounds.size.width as f32, bounds.size.height as f32];
            }

            let caret_arr = caret_rect.filter(|r| r.is_finite()).map(|r| [r.min.x, r.min.y, r.width(), r.height()]);

            let (focus_transition, selection_changed) = {
                let mut state = self.state.borrow_mut();
                let prev_focus = state.should_focus;
                state.should_focus = should_focus;
                state.is_writable = is_writable;
                state.pointer_is_write = pointer_is_write;
                state.caret_rect = caret_arr;
                state.last_selected_text = selected_text.to_owned();
                let pending = pending_commands || !state.commands.is_empty();
                let selection_changed = match document {
                    Some((text, selection)) if should_focus && is_writable => state.document.sync_document(text, selection, pending),
                    _ => state.document.sync_from_platform(selected_text, should_focus, is_writable),
                };
                ((prev_focus, should_focus), selection_changed)
            };

            // Only a change the app made to the text or the selection. Where the caret is drawn
            // moves with every letter typed, and is not one: telling UIKit the selection changed
            // after each keystroke made it drop its suggestions and ask for the text again.
            if selection_changed {
                self.view.notify_selection_changed();
            }

            match focus_transition {
                (false, true) => {
                    if !self.view.isFirstResponder() {
                        let _ = self.view.becomeFirstResponder();
                    }
                }
                (true, false) if self.view.isFirstResponder() => {
                    let _ = self.view.resignFirstResponder();
                }
                _ => {}
            }
        }

        /// Drains pending text-edit commands accumulated from UIKit callbacks.
        pub fn drain_commands(&self) -> Vec<TextEditCommand> {
            let mut state = self.state.borrow_mut();
            std::mem::take(&mut state.commands)
        }

        /// Returns the underlying `UIView` responder hosted inside `parent_view`.
        #[must_use]
        pub fn responder_view(&self) -> &UIView {
            self.view.as_super()
        }
    }

    impl Drop for TextInputBridge {
        fn drop(&mut self) {
            if let Some(parent) = self.view.superview() {
                parent.removeInteraction(ProtocolObject::from_ref(&*self.scribble_interaction));
            }
            if self.view.isFirstResponder() {
                let _ = self.view.resignFirstResponder();
            }
            self.view.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_WINDOW_UTF16, TextDocumentModel, TextEditAction, TextEditCommand, caret_rect_or_fallback, indirect_scribble_target_rect,
        split_insert_text_commands, utf16_len,
    };
    use std::cmp::Ordering;

    #[test]
    fn split_insert_text_maps_newlines_to_press_enter() {
        assert_eq!(split_insert_text_commands(""), Vec::<TextEditCommand>::new());
        assert_eq!(split_insert_text_commands("\n"), vec![TextEditCommand::PressEnter]);
        assert_eq!(split_insert_text_commands("\r\n"), vec![TextEditCommand::PressEnter]);
        assert_eq!(split_insert_text_commands("\r"), vec![TextEditCommand::PressEnter]);
        assert_eq!(split_insert_text_commands("\r\n\r\n"), vec![TextEditCommand::PressEnter, TextEditCommand::PressEnter]);
        assert_eq!(split_insert_text_commands("\n\r"), vec![TextEditCommand::PressEnter, TextEditCommand::PressEnter]);
        assert_eq!(
            split_insert_text_commands("hello\nworld\r\n!"),
            vec![
                TextEditCommand::InsertText("hello".to_owned()),
                TextEditCommand::PressEnter,
                TextEditCommand::InsertText("world".to_owned()),
                TextEditCommand::PressEnter,
                TextEditCommand::InsertText("!".to_owned()),
            ]
        );
    }

    #[test]
    fn utf16_surrogate_pairs_and_clamping_never_panic() {
        let mut doc = TextDocumentModel::new();
        let cmds = doc.insert_text("a😀b");
        assert_eq!(cmds, vec![TextEditCommand::InsertText("a😀b".to_owned())]);
        assert_eq!(doc.utf16_len(), 4);
        assert_eq!(doc.selection_utf16(), (4, 4));
        assert_eq!(doc.text_in_range(0, 1), "a");
        assert_eq!(doc.text_in_range(1, 3), "😀");
        assert_eq!(doc.text_in_range(3, 4), "b");

        // Mid-surrogate and out-of-bounds indices clamp cleanly without panicking.
        assert_eq!(doc.text_in_range(1, 2), "");
        assert_eq!(doc.text_in_range(2, 100), "😀b");
        assert_eq!(doc.text_in_range(4, 1), "😀b");

        doc.set_selection_utf16(2, 999);
        assert_eq!(doc.selection_utf16(), (1, 4));
        assert_eq!(doc.selected_text(), "😀b");

        assert_eq!(doc.position_from_offset(1, -2), None);
        assert_eq!(doc.position_from_offset(3, 2), None);
        assert_eq!(doc.position_from_offset(1, 2), Some(3));
        assert_eq!(TextDocumentModel::compare_positions(1, 3), Ordering::Less);
        assert_eq!(TextDocumentModel::offset_between(3, 1), -2);
    }

    #[test]
    fn delete_backward_removes_surrogate_pair_atomically_and_works_when_empty() {
        let mut doc = TextDocumentModel::new();
        assert_eq!(doc.delete_backward(), vec![TextEditCommand::DeleteBackward]);

        doc.insert_text("a😀");
        assert_eq!(doc.utf16_len(), 3);
        assert_eq!(doc.delete_backward(), vec![TextEditCommand::DeleteBackward]);
        assert_eq!(doc.text(), "a");
        assert_eq!(doc.selection_utf16(), (1, 1));
    }

    #[test]
    fn marked_text_composition_commit_and_unmark_flow() {
        let mut doc = TextDocumentModel::new();
        doc.insert_text("hi ");
        let cmd = doc.set_marked_text(Some("nihao"), (2, 5));
        assert_eq!(cmd, TextEditCommand::SetMarkedText { text: "nihao".to_owned(), selected_utf16: (2, 5) });
        assert_eq!(doc.marked_range_utf16(), Some((3, 8)));
        assert_eq!(doc.selection_utf16(), (5, 8));
        assert_eq!(doc.selected_text(), "hao");

        let committed = doc.insert_text("你好");
        assert_eq!(committed, vec![TextEditCommand::InsertText("你好".to_owned())]);
        assert_eq!(doc.text(), "hi 你好");
        assert_eq!(doc.marked_range_utf16(), None);
        assert_eq!(doc.selection_utf16(), (5, 5));

        doc.set_marked_text(Some("test"), (10, 20));
        assert_eq!(doc.marked_range_utf16(), Some((5, 9)));
        assert_eq!(doc.selection_utf16(), (9, 9));
        assert_eq!(doc.unmark_text(), Some(TextEditCommand::CommitText("test".to_owned())));
        assert_eq!(doc.marked_range_utf16(), None);
        assert_eq!(doc.unmark_text(), None);

        // Deleting backward while marked composition is active cancels only the uncommitted marked text
        // and emits `UnmarkText` so `egui` preedit is cleared when `egui_wants_ime == true`.
        doc.set_marked_text(Some("nihao"), (5, 5));
        assert_eq!(doc.marked_range_utf16(), Some((9, 14)));
        assert_eq!(doc.delete_backward(), vec![TextEditCommand::UnmarkText]);
        assert_eq!(doc.text(), "hi 你好test");
        assert_eq!(doc.marked_range_utf16(), None);
        assert_eq!(doc.selection_utf16(), (9, 9));
    }

    #[test]
    fn replace_range_before_caret_emits_char_deletes_and_insert() {
        let mut doc = TextDocumentModel::new();
        doc.insert_text("teh😀");
        let cmds = doc.replace_range(0, 5, "the");
        assert_eq!(
            cmds,
            vec![
                TextEditCommand::DeleteBackward,
                TextEditCommand::DeleteBackward,
                TextEditCommand::DeleteBackward,
                TextEditCommand::DeleteBackward,
                TextEditCommand::InsertText("the".to_owned()),
            ]
        );
        assert_eq!(doc.text(), "the");
        assert_eq!(doc.selection_utf16(), (3, 3));
    }

    #[test]
    fn document_window_stays_bounded_for_large_inputs() {
        let mut doc = TextDocumentModel::new();
        let chunk = "a".repeat(3000);
        doc.insert_text(&chunk);
        doc.insert_text(&chunk);
        assert!(doc.utf16_len() <= super::MAX_WINDOW_UTF16);
        assert_eq!(doc.selection_utf16().0, doc.utf16_len());
        assert_eq!(utf16_len(doc.text()), doc.utf16_len());
    }

    #[test]
    fn caret_rect_sanitization_enforces_minimum_and_fallback() {
        assert_eq!(caret_rect_or_fallback(Some([40.0, 80.0, 0.0, 0.0]), [0.0, 0.0, 1024.0, 768.0]), [40.0, 80.0, 2.0, 18.0]);
        assert_eq!(caret_rect_or_fallback(Some([40.0, 80.0, 12.0, 24.0]), [0.0, 0.0, 1024.0, 768.0]), [40.0, 80.0, 12.0, 24.0]);
        assert_eq!(caret_rect_or_fallback(Some([f32::NAN, 80.0, 2.0, 18.0]), [0.0, 0.0, 1000.0, 800.0]), [500.0, 200.0, 2.0, 18.0]);
        assert_eq!(caret_rect_or_fallback(Some([10.0, 20.0, -5.0, 18.0]), [0.0, 0.0, -1.0, 800.0]), [0.0, 0.0, 2.0, 18.0]);
        assert_eq!(caret_rect_or_fallback(None, [0.0, 0.0, 800.0, 600.0]), [400.0, 150.0, 2.0, 18.0]);
    }

    #[test]
    fn indirect_scribble_target_rect_gates_on_write_mode() {
        assert_eq!(indirect_scribble_target_rect(Some([200.0, 100.0, 2.0, 18.0]), false), None);
        assert_eq!(indirect_scribble_target_rect(None, true), None);
        assert_eq!(indirect_scribble_target_rect(Some([f32::INFINITY, 100.0, 2.0, 18.0]), true), None);
        assert_eq!(indirect_scribble_target_rect(Some([200.0, 100.0, -2.0, 18.0]), true), None);
        assert_eq!(indirect_scribble_target_rect(Some([200.0, 100.0, 2.0, 18.0]), true), Some([40.0, 52.0, 322.0, 114.0]));
        assert_eq!(indirect_scribble_target_rect(Some([10.0, 10.0, 0.0, 0.0]), true), Some([0.0, 0.0, 172.0, 76.0]));
        let _ = TextEditCommand::EditAction(TextEditAction::Copy);
    }

    #[test]
    fn sync_from_platform_preserves_typing_and_resets_on_blur() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_from_platform("hello", true, true));
        assert_eq!(doc.selected_text(), "hello");
        assert!(!doc.sync_from_platform("hello", true, true));

        doc.insert_text("world");
        assert_eq!(doc.text(), "world");
        // Empty platform selection while caret stays focused must not wipe local buffer.
        assert!(!doc.sync_from_platform("", true, true));
        assert_eq!(doc.text(), "world");

        // Blur clears local buffer.
        assert!(doc.sync_from_platform("", false, true));
        assert!(!doc.has_text());
    }

    #[test]
    fn document_window_preserves_insert_and_marked_text_at_start_of_full_buffer() {
        let mut doc = TextDocumentModel::new();
        doc.insert_text(&"a".repeat(MAX_WINDOW_UTF16));
        doc.set_selection_utf16(0, 0);

        let cmds = doc.insert_text("🎉");
        assert_eq!(cmds, vec![TextEditCommand::InsertText("🎉".to_owned())]);
        assert!(doc.utf16_len() <= MAX_WINDOW_UTF16);
        assert!(doc.text().starts_with("🎉"));
        assert_eq!(doc.selection_utf16(), (2, 2));

        doc.set_selection_utf16(0, 0);
        let marked_cmd = doc.set_marked_text(Some("nihao"), (2, 5));
        assert_eq!(marked_cmd, TextEditCommand::SetMarkedText { text: "nihao".to_owned(), selected_utf16: (2, 5) });
        assert!(doc.utf16_len() <= MAX_WINDOW_UTF16);
        assert!(doc.text().starts_with("nihao"));
        assert_eq!(doc.marked_range_utf16(), Some((0, 5)));
        assert_eq!(doc.selection_utf16(), (2, 5));
    }

    #[test]
    fn sync_from_platform_and_edit_actions_clear_cut_and_deleted_selections() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_from_platform("Hello world", true, true));
        assert_eq!(doc.selection_utf16(), (0, 11));

        // Platform Cut/Delete collapses platform selection to "".
        assert!(doc.sync_from_platform("", true, true));
        assert!(!doc.has_text());
        assert_eq!(doc.selection_utf16(), (0, 0));

        // Direct __cut / __delete action updates local TextDocumentModel synchronously.
        doc.insert_text("Hello world");
        doc.apply_edit_action(TextEditAction::SelectAll);
        assert_eq!(doc.selection_utf16(), (0, 11));
        doc.apply_edit_action(TextEditAction::Cut);
        assert_eq!(doc.text(), "");
        assert_eq!(doc.selection_utf16(), (0, 0));

        doc.insert_text("abc");
        doc.set_selection_utf16(1, 1);
        doc.apply_edit_action(TextEditAction::Delete);
        assert_eq!(doc.text(), "ac");
        assert_eq!(doc.selection_utf16(), (1, 1));
    }

    #[test]
    fn replace_range_handles_ranges_after_or_spanning_caret_without_dropping_deletes() {
        let mut doc = TextDocumentModel::new();
        doc.insert_text("abcd");
        doc.set_selection_utf16(1, 1);

        // Replace "cd" (2..4) while caret is at 1: moves 1 char right, deletes 2 chars forward, inserts "XY".
        let cmds = doc.replace_range(2, 4, "XY");
        assert_eq!(
            cmds,
            vec![
                TextEditCommand::MoveCursor(1),
                TextEditCommand::EditAction(TextEditAction::Delete),
                TextEditCommand::EditAction(TextEditAction::Delete),
                TextEditCommand::InsertText("XY".to_owned()),
            ]
        );
        assert_eq!(doc.text(), "abXY");
        assert_eq!(doc.selection_utf16(), (4, 4));

        // Replace "bX" (1..3) while caret is at 2: deletes 'X' forward, 'b' backward, inserts "Z".
        doc.set_selection_utf16(2, 2);
        let cmds_span = doc.replace_range(1, 3, "Z");
        assert_eq!(
            cmds_span,
            vec![TextEditCommand::EditAction(TextEditAction::Delete), TextEditCommand::DeleteBackward, TextEditCommand::InsertText("Z".to_owned()),]
        );
        assert_eq!(doc.text(), "aZY");
        assert_eq!(doc.selection_utf16(), (2, 2));
    }

    fn simulate_egui_commands(initial: &str, sel_utf16: (usize, usize), cmds: &[TextEditCommand]) -> String {
        let mut chars: Vec<char> = initial.chars().collect();
        let (sel_lo_u16, sel_hi_u16) = (sel_utf16.0.min(sel_utf16.1), sel_utf16.0.max(sel_utf16.1));
        let (lo_byte, _) = super::utf16_offset_to_byte_index(initial, sel_lo_u16);
        let (hi_byte, _) = super::utf16_offset_to_byte_index(initial, sel_hi_u16);
        let mut lo = initial[..lo_byte].chars().count();
        let mut hi = initial[..hi_byte].chars().count();

        for cmd in cmds {
            match cmd {
                TextEditCommand::MoveCursor(delta) => {
                    let mut steps = *delta;
                    while steps != 0 {
                        if lo < hi {
                            if steps < 0 {
                                hi = lo;
                            } else {
                                lo = hi;
                            }
                        } else if steps < 0 {
                            lo = lo.saturating_sub(1);
                            hi = lo;
                        } else {
                            lo = (lo + 1).min(chars.len());
                            hi = lo;
                        }
                        if steps < 0 {
                            steps += 1;
                        } else {
                            steps -= 1;
                        }
                    }
                }
                TextEditCommand::DeleteBackward => {
                    if lo < hi {
                        chars.drain(lo..hi);
                        hi = lo;
                    } else if lo > 0 {
                        lo -= 1;
                        chars.remove(lo);
                        hi = lo;
                    }
                }
                TextEditCommand::EditAction(TextEditAction::Delete) => {
                    if lo < hi {
                        chars.drain(lo..hi);
                        hi = lo;
                    } else if lo < chars.len() {
                        chars.remove(lo);
                    }
                }
                TextEditCommand::InsertText(text) => {
                    let ins: Vec<char> = text.chars().collect();
                    let ins_len = ins.len();
                    chars.splice(lo..hi, ins);
                    lo += ins_len;
                    hi = lo;
                }
                TextEditCommand::PressEnter => {
                    chars.splice(lo..hi, ['\n']);
                    lo += 1;
                    hi = lo;
                }
                other => assert_eq!(format!("{other:?}"), "", "unexpected command from replace_range"),
            }
        }
        chars.into_iter().collect()
    }

    #[test]
    fn replace_range_exhaustive_matrix_never_panics_and_matches_oracle() {
        for seed in ["abcdefgh", "a😀b🎉c"] {
            let max_u16 = utf16_len(seed);
            for sel_lo in 0..=max_u16 {
                for sel_hi in sel_lo..=max_u16 {
                    for rep_lo in 0..=max_u16 {
                        for rep_hi in rep_lo..=max_u16 {
                            for replacement in ["", "X", "Z\nW"] {
                                let mut doc = TextDocumentModel::new();
                                doc.insert_text(seed);
                                doc.set_selection_utf16(sel_lo, sel_hi);
                                let snapped_sel = doc.selection_utf16();

                                let cmds = doc.replace_range(rep_lo, rep_hi, replacement);
                                let simulated = simulate_egui_commands(seed, snapped_sel, &cmds);
                                assert_eq!(
                                    doc.text(),
                                    simulated,
                                    "mismatch for seed={seed:?}, sel=({sel_lo},{sel_hi}), rep=({rep_lo},{rep_hi}), repl={replacement:?}, cmds={cmds:?}"
                                );
                                assert_eq!(doc.marked_range_utf16(), None);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sync_from_platform_preserves_active_marked_composition_and_clears_on_blur() {
        // Non-empty platform selection (`!egui_wants_ime`): keeps marked composition intact.
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_from_platform("Alpha", true, true));
        let _ = doc.set_marked_text(Some("ni"), (2, 2));
        assert_eq!(doc.marked_range_utf16(), Some((0, 2)));
        assert!(!doc.sync_from_platform("Alpha", true, true));
        assert_eq!(doc.marked_range_utf16(), Some((0, 2)));
        assert_eq!(doc.text(), "ni");

        // `egui_wants_ime == true` (`selected_text == ""`) with relative marked selection `(2, 5)`.
        let mut ime_doc = TextDocumentModel::new();
        assert!(ime_doc.sync_from_platform("", true, true));
        let _ = ime_doc.set_marked_text(Some("nihao"), (2, 5));
        assert_eq!(ime_doc.marked_range_utf16(), Some((0, 5)));
        assert_eq!(ime_doc.selection_utf16(), (2, 5));
        assert!(!ime_doc.sync_from_platform("", true, true));
        assert_eq!(ime_doc.marked_range_utf16(), Some((0, 5)));
        assert_eq!(ime_doc.selection_utf16(), (2, 5));
        assert_eq!(ime_doc.text(), "nihao");

        // Blur still clears marked text.
        assert!(ime_doc.sync_from_platform("", false, true));
        assert_eq!(ime_doc.marked_range_utf16(), None);
        assert!(!ime_doc.has_text());
    }

    #[test]
    fn delete_backward_cancels_preedit_via_unmark_text_for_egui_ime() {
        let mut doc = TextDocumentModel::new();
        doc.set_marked_text(Some("nihao"), (5, 5));
        let cmds = doc.delete_backward();
        assert_eq!(cmds, vec![TextEditCommand::UnmarkText]);

        let mut queue = std::collections::VecDeque::from(cmds);
        let mut preedit = true;
        let dir = nocturne_store::testing::TempDir::new("ios-unit-unmark");
        let notebooks = dir.path().join("notebooks");
        let _ = std::fs::create_dir_all(&notebooks);
        let settings = nocturne_store::Settings::at(dir.path().join("settings.toml"));
        let session = match nocturne_engine::Session::new(&notebooks, settings) {
            Ok(session) => session,
            Err(err) => {
                assert_eq!(err.to_string(), "", "session creation failed");
                return;
            }
        };
        let mut app = nocturne_ui_egui::NocturneApp::new(session);
        let mut raw = egui::RawInput::default();

        let repaint = super::dispatch_pending_text_commands(&mut queue, true, &mut preedit, &mut app, &mut raw, || None);
        assert!(!repaint);
        assert!(!preedit);
        assert_eq!(raw.events, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None })]);
    }

    fn run_egui_text_edit_queue(
        ctx: &egui::Context,
        app: &mut nocturne_ui_egui::NocturneApp,
        buf: &mut String,
        preedit: &mut bool,
        queue: &mut std::collections::VecDeque<TextEditCommand>,
    ) {
        let mut focus_wait = 0u8;
        let id = egui::Id::unique("unit-text-edit");
        for _ in 0..32 {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(600.0, 400.0))),
                ..Default::default()
            };
            let repaint =
                super::dispatch_pending_text_commands_with_context(Some(ctx), &mut focus_wait, queue, true, preedit, app, &mut raw, || None);
            let mut out = ctx.run_ui(raw, |ui| {
                let resp = ui.add(egui::TextEdit::multiline(buf).id(id));
                if !ui.memory(|m| m.has_focus(id)) {
                    resp.request_focus();
                }
            });
            out.textures_delta.clear();
            if queue.is_empty() && !repaint {
                break;
            }
        }
        assert!(queue.is_empty(), "egui TextEdit queue did not drain: {queue:?}");
    }

    #[test]
    fn replace_range_with_active_marked_case_a_exact_and_superset_in_egui_and_doc() {
        let dir = nocturne_store::testing::TempDir::new("ios-unit-marked-case-a");
        let notebooks = dir.path().join("notebooks");
        let _ = std::fs::create_dir_all(&notebooks);
        let settings = nocturne_store::Settings::at(dir.path().join("settings.toml"));
        let Ok(session) = nocturne_engine::Session::new(&notebooks, settings) else {
            assert_eq!("session", "ok");
            return;
        };
        let mut app = nocturne_ui_egui::NocturneApp::new(session);

        // 1. Exact marked range replacement: "hello" + marked "ni" -> replace(5, 7, "你") => "hello你"
        let mut doc = TextDocumentModel::new();
        let mut queue = std::collections::VecDeque::new();
        let mut preedit = false;
        let mut buf = String::new();
        let ctx = egui::Context::default();
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);

        queue.extend(doc.insert_text("hello"));
        queue.push_back(doc.set_marked_text(Some("ni"), (2, 2)));
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);
        assert!(preedit);

        let cmds = doc.replace_range(5, 7, "你");
        assert_eq!(cmds, vec![TextEditCommand::UnmarkText, TextEditCommand::InsertText("你".to_owned())]);
        assert_eq!(doc.text(), "hello你");
        assert_eq!(doc.marked_range_utf16(), None);
        assert_eq!(doc.selection_utf16(), (6, 6));

        queue.extend(cmds);
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);
        assert_eq!(buf, "hello你");
        assert!(!preedit);

        // 2. Strict superset replacement covering committed prefix/suffix + marked span.
        let mut doc2 = TextDocumentModel::new();
        let mut buf2 = String::new();
        let mut preedit2 = false;
        let ctx2 = egui::Context::default();
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);
        queue.extend(doc2.insert_text("hello world"));
        doc2.set_selection_utf16(5, 5);
        queue.push_back(TextEditCommand::MoveCursor(-6));
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);

        queue.push_back(doc2.set_marked_text(Some("ni"), (2, 2)));
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);
        let cmds2 = doc2.replace_range(3, 9, "X");
        assert_eq!(
            cmds2,
            vec![
                TextEditCommand::UnmarkText,
                TextEditCommand::EditAction(TextEditAction::Delete),
                TextEditCommand::EditAction(TextEditAction::Delete),
                TextEditCommand::DeleteBackward,
                TextEditCommand::DeleteBackward,
                TextEditCommand::InsertText("X".to_owned()),
            ]
        );
        assert_eq!(doc2.text(), "helXorld");
        assert_eq!(doc2.marked_range_utf16(), None);
        queue.extend(cmds2);
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);
        assert_eq!(buf2, "helXorld");
        assert!(!preedit2);

        // 3. Empty replacement & surrogate-pair emoji marked replacement.
        let mut doc3 = TextDocumentModel::new();
        doc3.insert_text("a😀");
        let _ = doc3.set_marked_text(Some("🎉"), (2, 2));
        assert_eq!(
            doc3.replace_range(1, 5, "Z"),
            vec![TextEditCommand::UnmarkText, TextEditCommand::DeleteBackward, TextEditCommand::InsertText("Z".to_owned())]
        );
        assert_eq!(doc3.text(), "aZ");
        assert_eq!(doc3.marked_range_utf16(), None);

        let mut doc4 = TextDocumentModel::new();
        doc4.insert_text("hello");
        let _ = doc4.set_marked_text(Some("ni"), (2, 2));
        assert_eq!(doc4.replace_range(5, 7, ""), vec![TextEditCommand::UnmarkText]);
        assert_eq!(doc4.text(), "hello");
        assert_eq!(doc4.marked_range_utf16(), None);
    }

    #[test]
    fn replace_range_with_active_marked_case_b_disjoint_and_partial_in_egui_and_doc() {
        let dir = nocturne_store::testing::TempDir::new("ios-unit-marked-case-b");
        let notebooks = dir.path().join("notebooks");
        let _ = std::fs::create_dir_all(&notebooks);
        let settings = nocturne_store::Settings::at(dir.path().join("settings.toml"));
        let Ok(session) = nocturne_engine::Session::new(&notebooks, settings) else {
            assert_eq!("session", "ok");
            return;
        };
        let mut app = nocturne_ui_egui::NocturneApp::new(session);

        // 1. Disjoint preceding range: "hello" + marked "ni" -> replace(0, 2, "HE") => "HElloni"
        let mut doc = TextDocumentModel::new();
        let mut queue = std::collections::VecDeque::new();
        let mut preedit = false;
        let mut buf = String::new();
        let ctx = egui::Context::default();
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);

        queue.extend(doc.insert_text("hello"));
        queue.push_back(doc.set_marked_text(Some("ni"), (2, 2)));
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);
        assert!(preedit);

        let cmds = doc.replace_range(0, 2, "HE");
        assert_eq!(
            cmds,
            vec![
                TextEditCommand::CommitText("ni".to_owned()),
                TextEditCommand::MoveCursor(-5),
                TextEditCommand::DeleteBackward,
                TextEditCommand::DeleteBackward,
                TextEditCommand::InsertText("HE".to_owned()),
            ]
        );
        assert_eq!(doc.text(), "HElloni");
        assert_eq!(doc.marked_range_utf16(), None);
        assert_eq!(doc.selection_utf16(), (2, 2));

        queue.extend(cmds);
        run_egui_text_edit_queue(&ctx, &mut app, &mut buf, &mut preedit, &mut queue);
        assert_eq!(buf, "HElloni");
        assert!(!preedit);

        // 2. Partial left overlap: "hello" + marked "ni" -> replace(4, 6, "X") => "hellXi"
        let mut doc2 = TextDocumentModel::new();
        let mut buf2 = String::new();
        let mut preedit2 = false;
        let ctx2 = egui::Context::default();
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);
        queue.extend(doc2.insert_text("hello"));
        queue.push_back(doc2.set_marked_text(Some("ni"), (2, 2)));
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);

        let cmds2 = doc2.replace_range(4, 6, "X");
        assert_eq!(doc2.text(), "hellXi");
        assert_eq!(doc2.marked_range_utf16(), None);
        queue.extend(cmds2);
        run_egui_text_edit_queue(&ctx2, &mut app, &mut buf2, &mut preedit2, &mut queue);
        assert_eq!(buf2, "hellXi");
        assert!(!preedit2);

        // 3. Strictly interior to marked: "hello" + marked "nihao" -> replace(7, 9, "X") => "helloniXo"
        let mut doc3 = TextDocumentModel::new();
        let mut buf3 = String::new();
        let mut preedit3 = false;
        let ctx3 = egui::Context::default();
        run_egui_text_edit_queue(&ctx3, &mut app, &mut buf3, &mut preedit3, &mut queue);
        queue.extend(doc3.insert_text("hello"));
        queue.push_back(doc3.set_marked_text(Some("nihao"), (5, 5)));
        run_egui_text_edit_queue(&ctx3, &mut app, &mut buf3, &mut preedit3, &mut queue);

        let cmds3 = doc3.replace_range(7, 9, "X");
        assert_eq!(doc3.text(), "helloniXo");
        assert_eq!(doc3.marked_range_utf16(), None);
        queue.extend(cmds3);
        run_egui_text_edit_queue(&ctx3, &mut app, &mut buf3, &mut preedit3, &mut queue);
        assert_eq!(buf3, "helloniXo");
        assert!(!preedit3);
    }

    // ---- iPad Studio UX v2, phase 1: the keyboard sees the whole field -------------------------

    #[test]
    fn mirrored_field_has_its_text_before_anything_is_typed_and_backspace_works() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_document("Lemon tare", (10, 10), false), "a newly focused field is news to the keyboard");
        assert!(doc.has_text(), "a field with a name in it is not empty");
        assert_eq!(doc.selection_utf16(), (10, 10));
        assert_eq!(doc.text_in_range(6, 10), "tare", "the keyboard can read the word before the caret");

        assert_eq!(doc.delete_backward(), vec![TextEditCommand::DeleteBackward]);
        assert_eq!(doc.text(), "Lemon tar");
        // The app has not drawn the frame that applies the delete yet: nothing to report.
        assert!(!doc.sync_document("Lemon tare", (10, 10), true));
        assert_eq!(doc.text(), "Lemon tar", "the app's older text does not undo what the keyboard just did");
        // The app caught up and agrees: still nothing to report.
        assert!(!doc.sync_document("Lemon tar", (9, 9), false));
    }

    #[test]
    fn typing_into_a_mirrored_field_never_reports_a_selection_change() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_document("Blue", (4, 4), false));
        let mut app_text = "Blue".to_string();
        let mut reports = 0;
        for typed in [" ", "L", "a", "g", "o", "o", "n", " ", "\u{1F600}"] {
            assert_eq!(doc.insert_text(typed), vec![TextEditCommand::InsertText(typed.to_string())]);
            let before = app_text.chars().count();
            reports += usize::from(doc.sync_document(&app_text, (before, before), true));
            app_text.push_str(typed);
            let after = app_text.chars().count();
            reports += usize::from(doc.sync_document(&app_text, (after, after), false));
            // The caret's place on screen moves with every letter; that is not a selection change.
            reports += usize::from(doc.sync_document(&app_text, (after, after), false));
        }
        assert_eq!(reports, 0, "selectionDidChange during typing throws away the keyboard's suggestions");
        assert_eq!(doc.text(), "Blue Lagoon \u{1F600}");
        let end = utf16_len(doc.text());
        assert_eq!(doc.selection_utf16(), (end, end), "characters outside the BMP count as two UTF-16 units");
    }

    #[test]
    fn a_caret_moved_in_the_app_is_reported_once_and_marked_text_is_left_alone() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_document("Lemon tare", (10, 10), false));
        assert!(doc.sync_document("Lemon tare", (5, 2), false), "the user dragged a selection in the field");
        assert_eq!(doc.selection_utf16(), (2, 5), "kept in order");
        assert!(!doc.sync_document("Lemon tare", (5, 2), false));

        let _ = doc.set_marked_text(Some("ni"), (2, 2));
        let marked = doc.text().to_string();
        assert!(!doc.sync_document("Lemon tare", (5, 5), false), "a composition in progress is the keyboard's");
        assert_eq!(doc.text(), marked);
    }

    #[test]
    fn moving_between_a_mirrored_field_and_the_page_caret_starts_afresh() {
        let mut doc = TextDocumentModel::new();
        assert!(doc.sync_document("Section name", (12, 12), false));
        // The dialog closes and the page's own caret takes the keyboard without a blur in between.
        assert!(doc.sync_from_platform("", true, true));
        assert_eq!(doc.text(), "", "the dialog's text is not the page's");
        // And back.
        assert!(doc.sync_document("Section name", (12, 12), false));
        assert_eq!(doc.text(), "Section name");
        // Blur empties it.
        assert!(doc.sync_from_platform("", false, true));
        assert!(!doc.has_text());
    }

    #[test]
    fn cursor_moves_for_an_egui_field_are_sent_in_one_frame() {
        let dir = nocturne_store::testing::TempDir::new("ios-unit-move-batch");
        let notebooks = dir.path().join("notebooks");
        let _ = std::fs::create_dir_all(&notebooks);
        let settings = nocturne_store::Settings::at(dir.path().join("settings.toml"));
        let Ok(session) = nocturne_engine::Session::new(&notebooks, settings) else {
            assert_eq!("session", "ok");
            return;
        };
        let mut app = nocturne_ui_egui::NocturneApp::new(session);
        // What `replaceRange:withText:` three letters back queues: moves, then the text, then moves.
        let mut queue = std::collections::VecDeque::from(vec![
            TextEditCommand::MoveCursor(-3),
            TextEditCommand::InsertText(" ".to_owned()),
            TextEditCommand::MoveCursor(2),
        ]);
        let mut preedit = false;
        let mut raw = egui::RawInput::default();
        let repaint = super::dispatch_pending_text_commands(&mut queue, true, &mut preedit, &mut app, &mut raw, || None);
        assert!(!repaint, "nothing is left for a later frame");
        assert!(queue.is_empty(), "the space is not held up behind the cursor moves: {queue:?}");
        let mut expected = Vec::new();
        for _ in 0..3 {
            super::push_key_press_release(&mut expected, egui::Key::ArrowLeft, egui::Modifiers::NONE);
        }
        expected.push(egui::Event::Text(" ".to_owned()));
        for _ in 0..2 {
            super::push_key_press_release(&mut expected, egui::Key::ArrowRight, egui::Modifiers::NONE);
        }
        assert_eq!(raw.events, expected);
    }

    #[test]
    fn the_page_title_is_typed_through_its_own_text_box() {
        let ctx = egui::Context::default();
        let page_rect = egui::Rect::from_min_max(egui::pos2(200.0, 120.0), egui::pos2(900.0, 720.0));
        let title_caret = egui::Rect::from_min_size(egui::pos2(240.0, 140.0), egui::vec2(2.0, 26.0));
        let ime_at_title = egui::Rect::from_min_size(egui::pos2(241.0, 141.0), egui::vec2(1.0, 24.0));
        // A block's caret in the same place still belongs to the page.
        assert!(!super::route_ime_to_egui(false, Some(ime_at_title), Some(title_caret), Some(page_rect), &ctx));
        // The title is an egui text box: it types its own letters, spaces included.
        assert!(super::route_ime_to_egui(true, Some(ime_at_title), Some(title_caret), Some(page_rect), &ctx));
        assert!(!super::route_ime_to_egui(true, None, Some(title_caret), Some(page_rect), &ctx), "no text box asked for the keyboard");
    }

    #[test]
    fn should_route_ime_to_egui_distinguishes_app_caret_from_external_widgets() {
        let ctx = egui::Context::default();
        let page_rect = egui::Rect::from_min_max(egui::pos2(200.0, 120.0), egui::pos2(900.0, 720.0));
        let caret_rect = egui::Rect::from_min_size(egui::pos2(240.0, 200.0), egui::vec2(2.0, 18.0));
        let ime_at_caret = egui::Rect::from_min_size(egui::pos2(241.0, 201.0), egui::vec2(1.0, 16.0));
        let ime_in_search = egui::Rect::from_min_size(egui::pos2(820.0, 24.0), egui::vec2(1.0, 16.0));
        let ime_in_dialog = egui::Rect::from_min_size(egui::pos2(500.0, 350.0), egui::vec2(1.0, 16.0));

        // 1. No IME output -> false.
        assert!(!super::should_route_ime_to_egui(None, Some(caret_rect), Some(page_rect), &ctx));

        // 2. No active NocturneApp caret -> true (dialogs / backstage / Draw / show_source).
        assert!(super::should_route_ime_to_egui(Some(ime_at_caret), None, Some(page_rect), &ctx));

        // 3. Steady-state NocturneApp block/title caret on Order::Background -> false.
        let mut out_bg = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1024.0, 768.0))), ..Default::default() },
            |ui| {
                let _ = ui.allocate_rect(page_rect, egui::Sense::click());
            },
        );
        out_bg.textures_delta.clear();
        assert!(!super::should_route_ime_to_egui(Some(ime_at_caret), Some(caret_rect), Some(page_rect), &ctx));

        // 4. Top-bar Search TextEdit outside page_rect even while ed.caret is still parked -> true.
        assert!(super::should_route_ime_to_egui(Some(ime_in_search), Some(caret_rect), Some(page_rect), &ctx));

        // 4b. Scrolled long page translates page_rect upward over the top bar; localized caret_rect
        // must still route Search/ribbon IME to egui instead of falling back to page_rect containment.
        let scrolled_page_rect = egui::Rect::from_min_max(egui::pos2(200.0, 5.0), egui::pos2(900.0, 605.0));
        let scrolled_caret = egui::Rect::from_min_size(egui::pos2(240.0, 80.0), egui::vec2(2.0, 18.0));
        assert!(super::should_route_ime_to_egui(Some(ime_in_search), Some(scrolled_caret), Some(scrolled_page_rect), &ctx));

        // 5. Fallback inside page_rect when caret_rect is page_rect -> false.
        assert!(!super::should_route_ime_to_egui(Some(ime_in_dialog), Some(page_rect), Some(page_rect), &ctx));

        // 6. Order::Middle dialog window covering ime_in_dialog -> true even if caret_rect is Some.
        for _ in 0..2 {
            let mut out_dlg = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1024.0, 768.0))),
                    ..Default::default()
                },
                |ui| {
                    egui::Window::new("Link").fixed_pos(egui::pos2(420.0, 300.0)).fixed_size(egui::vec2(260.0, 140.0)).show(ui.ctx(), |ui| {
                        ui.label("Dialog");
                    });
                },
            );
            out_dlg.textures_delta.clear();
        }
        assert!(super::should_route_ime_to_egui(Some(ime_in_dialog), Some(page_rect), Some(page_rect), &ctx));

        // 7. Unrelated non-text popup open elsewhere does not hijack localized caret on Order::Background,
        // while an Order::Foreground popup covering ime_rect still routes to egui.
        let popup_ctx = egui::Context::default();
        let popup_area_id = egui::Id::unique("test-menu-popup");
        let ime_in_popup = egui::Rect::from_min_size(egui::pos2(860.0, 44.0), egui::vec2(1.0, 16.0));
        let mut popup_rect = None;
        for frame in 0..4 {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1024.0, 768.0))),
                ..Default::default()
            };
            if frame == 1 {
                let btn_pos = egui::pos2(860.0, 16.0);
                raw.events.push(egui::Event::PointerMoved(btn_pos));
                raw.events.push(egui::Event::PointerButton {
                    pos: btn_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                raw.events.push(egui::Event::PointerButton {
                    pos: btn_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut out_pop = popup_ctx.run_ui(raw, |ui| {
                let _ = ui.allocate_rect(page_rect, egui::Sense::click());
                let btn = ui.put(egui::Rect::from_min_size(egui::pos2(840.0, 4.0), egui::vec2(60.0, 24.0)), egui::Button::new("Theme"));
                egui::Popup::menu(&btn).id(popup_area_id).show(|ui| {
                    let resp = ui.allocate_exact_size(egui::vec2(120.0, 80.0), egui::Sense::click());
                    popup_rect = Some(resp.0);
                });
            });
            out_pop.textures_delta.clear();
        }
        assert!(popup_ctx.any_popup_open());
        assert!(!super::should_route_ime_to_egui(Some(ime_at_caret), Some(caret_rect), Some(page_rect), &popup_ctx));
        let popup_center = popup_rect.expect("popup rect rendered").center();
        let ime_over_popup = egui::Rect::from_center_size(popup_center, egui::vec2(2.0, 16.0));
        let _ = ime_in_popup;
        assert!(super::should_route_ime_to_egui(Some(ime_over_popup), Some(caret_rect), Some(page_rect), &popup_ctx));
    }
}
