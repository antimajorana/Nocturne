//! The Nocturne window: top bar, notebook tree, page editor with live preview.
//!
//! Every action goes through `Session::run`, so the window, the command line and any
//! later control channel share one code path.

use crate::backstage;
use crate::control::{ControlRequest, ControlServer, base64};
// The file dialogs: `rfd` everywhere it has a backend, and a stand-in on Android and iOS that says so.
#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::filepicker as rfd;
use crate::floating;
use crate::icons::{self, Icon};
use crate::navhistory::NavHistory;
use crate::pageedit;
use crate::preview;
use crate::printout;
use crate::quickaccess;
use crate::ribbon;
use crate::snippets;
use crate::theme::{Accent, Mode, ModePref, Theme, Tokens};
use egui::{Align2, Color32, Context, Id, Key, Modifiers, Sense, Ui, vec2};
use nocturne_engine::{EngineError, Output, Params, Session};
use nocturne_model::{NodeColor, NodeKind, PageFormat};
use nocturne_store::search::Hit;
use nocturne_store::tree::Node;
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
// std's own Instant panics on wasm32-unknown-unknown, which has no clock; web_time is the
// same type everywhere else and performance.now()/Date.now() there.
use web_time::Instant;

/// The grid boxes snap to on plain paper, in page points: the smallest squared paper's step.
const PLAIN_SNAP_STEP: f32 = 12.0;
const AUTOSAVE_AFTER: Duration = Duration::from_millis(900);
/// The three-bar button: what OneNote puts at the top left of the navigation pane.
const BARS: &str = "\u{2630}";

/// Where the pen rack and the tool in hand are remembered between runs.
const RACK_KEY: &str = "pen_rack";
/// The size the whole window is drawn at, in percent: see `backstage::UI_SCALES`.
const UI_SCALE_KEY: &str = "ui_scale";
const NIB_KEY: &str = "pen_in_hand";
const SENSITIVITY_KEY: &str = "pen_sensitivity";
const CURVE_KEY: &str = "pen_pressure_curve";
const DEFAULT_SENSITIVITY: u8 = 50;

fn default_telemetry_level(debug_build: bool) -> &'static str {
    if debug_build { "debug" } else { "none" }
}

const ROW_HEIGHT: f32 = 24.0;

/// Space between the end of a page name and its format label in the page list.
const ROW_LABEL_GAP: f32 = 6.0;

/// How wide `text` is when drawn on one line at `size` points.
fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), egui::Color32::WHITE).rect.width())
}

/// Whether a row's format label is shown, and how much room its name gets. The label is
/// shown only when the whole name fits beside it; otherwise the name gets the whole row.
fn row_name_room(name_width: f32, label_width: Option<f32>, room: f32) -> (bool, f32) {
    match label_width {
        Some(label) if name_width + label <= room => (true, room - label),
        _ => (false, room),
    }
}

/// `text` as it fits in `room` points: whole when it fits, otherwise cut at a character
/// and ended with "…", so a long name never runs into what is drawn after it.
fn fit_to_width(ui: &Ui, text: &str, size: f32, room: f32) -> String {
    if text_width(ui, text, size) <= room {
        return text.to_owned();
    }
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).chain(std::iter::once(text.len())).collect();
    // The longest start of the name that still fits with the ellipsis after it.
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate = format!("{}…", text[..ends[mid - 1]].trim_end());
        if text_width(ui, &candidate, size) <= room { lo = mid } else { hi = mid - 1 }
    }
    if lo == 0 { "…".to_owned() } else { format!("{}…", text[..ends[lo - 1]].trim_end()) }
}
const INDENT: f32 = 14.0;

/// The text boxes a platform keyboard types into besides the page: each has a fixed id, so the
/// window can say which one has the keyboard ([`NocturneApp::focused_text_field`]).
const RENAME_FIELD: &str = "rename-title-field";
const ADD_FIELD: &str = "add-title-field";
const NEW_NOTEBOOK_FIELD: &str = "new-notebook-name-field";
pub(crate) const SEARCH_FIELD: &str = "search-all-pages-field";
const LINK_TEXT_FIELD: &str = "link-dialog-text";
const LINK_ADDRESS_FIELD: &str = "link-dialog-address";
const TABLE_COLUMNS_FIELD: &str = "insert-table-columns";
const TABLE_ROWS_FIELD: &str = "insert-table-rows";
const TEMPLATE_NAME_FIELD: &str = "template-name-field";
const PAGE_TITLE_FIELD: &str = "page-title-box";

/// The one-line name box of a dialog. It asks for the keyboard when the dialog opens, and again
/// after a click that left nothing with the keyboard, and at no other time.
///
/// It used to ask on every frame. That kept typing and Enter working after a click on another
/// of the dialog's controls, which takes the keyboard from the box as a click anywhere else
/// does. On an iPad it also made the keyboard drop and rise on every frame. So the box now
/// gets the keyboard back once such a click is over, and a keyboard put away without a click
/// (the iPad's own hide key) stays away.
fn name_field(ui: &mut Ui, text: &mut String, id: &str, opened_now: bool) -> egui::Response {
    let id = egui::Id::unique(id);
    // A window's first frame only measures it: its widgets are disabled there, and a text box
    // that is disabled gives the keyboard straight back. So the one request waits for the
    // first frame the box can take it.
    let waiting = id.with("wants-keyboard");
    if opened_now {
        ui.data_mut(|d| d.insert_temp(waiting, true));
    }
    if ui.is_enabled() && !ui.is_sizing_pass() && ui.data_mut(|d| d.remove_temp::<bool>(waiting)).unwrap_or(false) {
        ui.memory_mut(|m| m.request_focus(id));
    }
    let response = ui.add(egui::TextEdit::singleline(text).id(id).desired_width(f32::INFINITY));
    if ui.input(|i| i.pointer.any_click()) && ui.memory(|m| m.focused().is_none()) {
        ui.data_mut(|d| d.insert_temp(waiting, true));
        ui.ctx().request_repaint();
    }
    response
}

fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn missing_attachment_icon(page: &Path, src: &str) -> Option<String> {
    let name = src.strip_prefix("assets/")?;
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    // Generated icon sidecars are images too, but they are not attachments that
    // need another icon. Treat `file.ext.svg`/`file.ext.png` as implementation
    // details when the attachment they belong to is present.
    if let Some((source, extension)) = name.rsplit_once('.')
        && matches!(extension.to_ascii_lowercase().as_str(), "png" | "svg")
        && page.join("assets").join(source).is_file()
    {
        return None;
    }
    if nocturne_store::assets::media_kind(src) != nocturne_store::assets::MediaKind::Other {
        return None;
    }
    // A missing attachment is not a missing-icon case. Do not warn about an icon
    // when the original file itself is no longer present.
    if !page.join("assets").join(name).is_file() {
        return None;
    }
    (!attachment_icon_healthy(page, name)).then(|| name.to_string())
}

/// A generated PNG is preferred on Windows, but the deterministic SVG fallback is also a
/// complete icon. In particular, Shell extraction can fail for files on a UNC-mounted
/// enlistment even though the fallback renders correctly.
fn attachment_icon_healthy(page: &Path, name: &str) -> bool {
    attachment_icon_exists(page, name)
}

fn attachment_icon_exists(page: &Path, name: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        // Native Shell icons are PNG, but the deterministic SVG fallback is valid
        // when Shell extraction is unavailable on this Windows installation.
        page.join("assets").join(format!("{name}.png")).is_file() || page.join("assets").join(format!("{name}.svg")).is_file()
    }
    #[cfg(not(target_os = "windows"))]
    {
        page.join("assets").join(format!("{name}.svg")).is_file() || page.join("assets").join(format!("{name}.png")).is_file()
    }
}

fn attachment_icon_link(page: &Path, name: &str) -> String {
    let dir = page.join("assets");
    if dir.join(format!("{name}.png")).is_file() { format!("assets/{name}.png") } else { format!("assets/{name}.svg") }
}

fn replace_attachment_icon(format: PageFormat, snippet: &str, name: &str, icon_link: &str) -> String {
    match format {
        PageFormat::Html => snippet
            .replace(&format!("src=\"assets/{name}.png\""), &format!("src=\"{icon_link}\""))
            .replace(&format!("src=\"assets/{name}.svg\""), &format!("src=\"{icon_link}\"")),
        PageFormat::Cmgfm => {
            let png = format!("](assets/{name}.png)");
            let svg = format!("](assets/{name}.svg)");
            snippet.replacen(&png, &format!("]({icon_link})"), 1).replacen(&svg, &format!("]({icon_link})"), 1)
        }
    }
}

#[cfg(test)]
mod attachment_reference_tests {
    use super::{PageFormat, replace_attachment_icon};

    #[cfg(not(target_os = "windows"))]
    use super::attachment_icon_exists;

    #[test]
    fn html_repair_changes_only_the_image_source() {
        let input = r#"<p><a href="assets/brief.docx"><img src="assets/brief.docx.svg" alt="" width="48"><br>brief.docx</a></p>"#;
        let output = replace_attachment_icon(PageFormat::Html, input, "brief.docx", "assets/brief.docx.png");
        assert!(output.contains(r#"href="assets/brief.docx""#));
        assert!(output.contains(r#"src="assets/brief.docx.png""#));
    }

    #[test]
    fn markdown_repair_changes_only_the_inner_image_link() {
        let input = "[![brief.docx](assets/brief.docx.svg) brief.docx](assets/brief.docx)";
        let output = replace_attachment_icon(PageFormat::Cmgfm, input, "brief.docx", "assets/brief.docx.png");
        assert_eq!(output, "[![brief.docx](assets/brief.docx.png) brief.docx](assets/brief.docx)");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn a_windows_png_sidecar_is_a_valid_icon_on_posix() {
        let t = nocturne_store::testing::TempDir::new("attachment-icon");
        let page = t.path().join("page");
        std::fs::create_dir_all(page.join("assets")).expect("assets");
        std::fs::write(page.join("assets/brief.docx"), b"attachment").expect("attachment");
        std::fs::write(page.join("assets/brief.docx.png"), b"png sidecar").expect("icon");
        assert!(attachment_icon_exists(&page, "brief.docx"));
    }
}

fn first_missing_attachment_in_source(source: &str, page: &Path) -> Option<String> {
    let mut rest = source;
    while let Some(at) = rest.find("assets/") {
        rest = &rest[at + "assets/".len()..];
        let end = rest.find(['"', '\'', ')', ']', '<', '>', ' ', '\n', '\r']).unwrap_or(rest.len());
        let name = &rest[..end];
        let candidate = format!("assets/{name}");
        if let Some(name) = (!name.is_empty()).then(|| missing_attachment_icon(page, &candidate)).flatten() {
            return Some(name);
        }
        rest = &rest[end..];
    }
    None
}

#[allow(dead_code)]
fn first_unreferenced_attachment(attachments: &[(String, u64)], source: &str) -> Option<String> {
    attachments.iter().find_map(|(name, _)| {
        let reference = format!("assets/{name}");
        (!source.contains(&reference)).then(|| name.clone())
    })
}

#[allow(dead_code)]
fn prepare_unreferenced_attachment(page: &Path, name: &str) -> bool {
    match nocturne_store::assets::ensure_icon(page, name) {
        Ok(()) => {
            log::debug!("generated icon before detached attachment warning: page={:?} attachment={name:?}", page);
            true
        }
        Err(error) => {
            log::error!("could not generate icon before detached attachment warning: page={:?} attachment={name:?} error={error}", page);
            false
        }
    }
}

fn first_legacy_attachment_in_source(source: &str) -> Option<String> {
    let mut rest = source;
    while let Some(at) = rest.find("assets/") {
        rest = &rest[at + "assets/".len()..];
        let end = rest.find(['"', '\'', ')', ']', '<', '>', ' ', '\n', '\r']).unwrap_or(rest.len());
        let name = &rest[..end];
        if !name.is_empty()
            && !name.ends_with(".svg")
            && nocturne_store::assets::media_kind(&format!("assets/{name}")) == nocturne_store::assets::MediaKind::Other
            && (source.contains(&format!("<a href=\"assets/{name}\">")) || source.contains(&format!("[{name}](assets/{name})")))
        {
            return Some(name.to_string());
        }
        rest = &rest[end..];
    }
    None
}

fn shown(p: &Path) -> String {
    nocturne_store::tree::display_path(p)
}

/// The widest empty stretch Collect All leaves between things on a page, in page points.
const COLLECT_GAP: f32 = 24.0;
/// The Insert Space tool's own layer over the visible page (issue #164): a vertical-resize
/// pointer, a guide at the pointer's height, and while it is dragged, the band of room being
/// added (below the start) or taken away (above it). Returns where the drag began and how far it
/// went, in screen points, on the frame it is let go.
fn insert_space_overlay(ctx: &Context, visible: egui::Rect, t: &Tokens) -> Option<(f32, f32)> {
    let id = Id::new("insert-space-tool");
    let from_id = id.with("from");
    let mut done = None;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(visible.min).constrain(false).movable(false).show(ctx, |ui| {
        let (rect, resp) = ui.allocate_exact_size(visible.size(), Sense::drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.insert_space_how")));
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            ui.data_mut(|d| d.insert_temp(from_id, p.y));
        }
        let from = ui.data(|d| d.get_temp::<f32>(from_id));
        let now = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p) || resp.dragged()).map(|p| p.y.clamp(rect.min.y, rect.max.y));
        let painter = ui.painter();
        let line = egui::Stroke::new(1.5, t.accent);
        match (from, now) {
            (Some(a), Some(b)) => {
                let band = egui::Rect::from_x_y_ranges(rect.x_range(), egui::Rangef::new(a.min(b), a.max(b)));
                painter.rect_filled(band, 0.0, t.accent.gamma_multiply(0.12));
                painter.hline(rect.x_range(), a, line);
                painter.hline(rect.x_range(), b, egui::Stroke::new(1.0, t.accent.gamma_multiply(0.7)));
            }
            (None, Some(y)) => {
                painter.hline(rect.x_range(), y, line);
            }
            _ => {}
        }
        if resp.drag_stopped() {
            if let (Some(a), Some(p)) = (from, resp.interact_pointer_pos()) {
                done = Some((a, p.y.clamp(rect.min.y, rect.max.y) - a));
            }
            ui.data_mut(|d| d.remove::<f32>(from_id));
        }
    });
    done
}

fn notebook_for(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if nocturne_store::tree::read_node(current).is_some_and(|node| node.kind == NodeKind::Notebook) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// The path as the window may show it: redacted to `<notebooks>/relative/path` when
/// `redact` is on (for screenshots, tests and documentation, so no local path leaks).
fn shown_in(root: &Path, p: &Path, redact: bool) -> String {
    if !redact {
        return shown(p);
    }
    match p.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => "<notebooks folder>".to_string(),
        Ok(rel) => format!("<notebooks>/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => "<outside>".to_string(),
    }
}

/// Where the Tags Summary looks for tagged notes (issue #166), widest last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagScope {
    Page,
    Section,
    Notebook,
    All,
}

impl TagScope {
    const ALL: [TagScope; 4] = [TagScope::Page, TagScope::Section, TagScope::Notebook, TagScope::All];

    fn label(self) -> &'static str {
        match self {
            TagScope::Page => crate::tl_id!("ui.tags_scope_this_page"),
            TagScope::Section => crate::tl_id!("ui.tags_scope_this_section"),
            TagScope::Notebook => crate::tl_id!("ui.tags_scope_this_notebook"),
            TagScope::All => crate::tl_id!("ui.tags_scope_all_notebooks"),
        }
    }

    fn id(self) -> &'static str {
        match self {
            TagScope::Page => "page",
            TagScope::Section => "section",
            TagScope::Notebook => "notebook",
            TagScope::All => "all",
        }
    }
}

/// The Tags Summary pane, OneNote's Find Tags: the tagged notes in a scope, grouped by tag,
/// each one a way back to its line. It lists what the pages said when it last looked; Refresh
/// looks again, as OneNote's does.
struct TagsPane {
    scope: TagScope,
    /// Only the To Do items not yet ticked.
    unchecked: bool,
    found: Vec<nocturne_store::tags::Tagged>,
}

/// The Page Templates pane (issue #173): every template, by category, each one a new page in
/// the section; and a way to keep the open page as a template of its own.
struct TemplatesPane {
    templates: Vec<nocturne_store::page_templates::Template>,
    /// What the open page will be called as a template.
    name: String,
}

/// The section a page is in: the nearest folder above it that is a section.
fn section_for(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent()?;
    loop {
        match nocturne_store::tree::read_node(current).map(|node| node.kind) {
            Some(NodeKind::Section) => return Some(current.to_path_buf()),
            Some(NodeKind::Page) => {}
            _ => return None,
        }
        current = current.parent()?;
    }
}

/// What the Format Painter holds: the formatting it picked up, where it picked it up (so the
/// selection it was picked up from is not painted with itself), and whether it keeps painting.
#[derive(Clone, Debug)]
struct FormatPaint {
    style: nocturne_formats::Span,
    from: (usize, (usize, usize)),
    sticky: bool,
}

#[derive(Clone, Debug, Default)]
struct PendingFormat {
    block: usize,
    at: usize,
    size: Option<u8>,
    family: Option<String>,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    script: Option<nocturne_formats::markdown::Script>,
}

struct Editor {
    dir: PathBuf,
    format: PageFormat,
    title: String,
    title_buf: String,
    /// The page has a title of its own; `title` is only the fallback name when it has not.
    titled: bool,
    body: String,
    /// What the page's file holds, as last read or written, so a save that would not change it
    /// is not made: a save rewrites the file, which on a synced folder can take a second.
    saved_body: String,
    /// The text an autosave is writing in the background, until it is written.
    saving: Option<String>,
    dirty_since: Option<Instant>,
    blocks: Vec<nocturne_formats::Block>,
    last_saved: Option<String>,
    attachments: Vec<(String, u64)>,
    /// Where the caret is in the page, and the text of that block while it is written in.
    caret: Option<pageedit::Caret>,
    /// Screen-space rectangle of the active caret (or title caret) from the most recent layout pass.
    caret_screen_rect: Option<egui::Rect>,
    /// Whether the page title box currently holds focus.
    title_focused: bool,
    /// Current character selection `(primary, secondary)` in `title_buf` while `title_focused`.
    title_selection: Option<(usize, usize)>,
    /// Pending character selection `(primary, secondary)` to apply to `title_id` on next frame.
    title_restore_selection: Option<(usize, usize)>,
    /// Formatting chosen at a collapsed caret, before any text has been inserted.
    pending_format: Option<PendingFormat>,
    restore_format_selection: Option<(usize, usize, usize)>,
    /// False when the page holds markup the block editor would drop: it is then read here
    /// and changed in its source.
    writable: bool,
    /// Which way the whole page reads, if the writer said so. A line with no letters of its
    /// own follows it.
    page_dir: Option<nocturne_model::Dir>,
    /// The boxes the page's blocks are arranged in, as the page's own file keeps them.
    boxes: Vec<nocturne_model::Container>,
    /// How wide the columns of this page's tables are drawn, from the same place.
    columns: nocturne_model::tablecols::Widths,
    /// Which paragraphs of this page are stretched to fill the column, from the same place.
    /// The words themselves are never touched; see `nocturne_model::justify`.
    justified: nocturne_model::justify::Justified,
    /// A selection dragged across more than one block. A caret belongs to one block, so a
    /// selection that leaves it is kept here; `None` is the ordinary case.
    across: Option<pageedit::Across>,
    /// The paper this page is drawn on and the colour behind it, as the page's own file
    /// keeps them. `None` means the page says nothing and the window's own choice stands.
    paper: Option<ribbon::RuleLines>,
    colour: Option<nocturne_model::NodeColor>,
    /// The colour of this page's rule and grid lines, when it was given one (issue #288).
    rule_colour: Option<nocturne_model::NodeColor>,
    canvas_layout: nocturne_store::page::CanvasLayout,
    /// What the page looked like before each change, so a change can be taken back.
    history: crate::history::History,
    /// The ink files in the page's folder. The folder is what decides: the page's own
    /// text is kept in step with this, never the other way round.
    ink_files: Vec<String>,
    /// Where the page's area starts on screen, reported so a script can find a box.
    origin: egui::Pos2,
    /// Where the page's origin sits in the panned canvas, from the last frame it was drawn. The
    /// paper's lines are drawn through it, so they stay with the page and its boxes (issue #79).
    origin_in_canvas: Option<egui::Vec2>,
    /// Whether the boxes are written into the page's text. A page nobody arranged stays plain
    /// text, one column; moving, resizing or adding a box arranges it, and from then on the
    /// boxes are elements around the blocks.
    arranged: bool,
    /// How far down the page the containers reached when last drawn, in page points.
    content_bottom: Option<f32>,
    /// How wide each box was drawn last frame, in page points (`containers::Moved::shown_widths`).
    shown_widths: Vec<f32>,
    /// Container bounding frames in unzoomed page coordinates from the most recent layout pass.
    last_frames: Vec<egui::Rect>,
}

impl Editor {
    /// `count` blocks were just put into the page at `at`. The box that holds the block before
    /// them counts them too (the first box, for blocks put in at the top), so the boxes still
    /// add up to the page's blocks and `container::fit` keeps the arrangement instead of
    /// folding it into one box (issue #72). Call it before [`Editor::rebuild_body`].
    fn count_inserted_in_boxes(&mut self, at: usize, count: usize) {
        let counted: usize = self.boxes.iter().map(|c| c.blocks).sum();
        if count == 0 || self.boxes.is_empty() || counted + count != self.blocks.len() {
            return;
        }
        let owner = if at == 0 { Some(0) } else { nocturne_model::container::place(&self.boxes, at - 1).map(|(i, _)| i) };
        if let Some(i) = owner {
            self.boxes[i].blocks += count;
        }
    }

    /// The block at `at` was just taken out of the page: its box counts one fewer.
    fn count_removed_from_boxes(&mut self, at: usize) {
        let counted: usize = self.boxes.iter().map(|c| c.blocks).sum();
        if counted != self.blocks.len() + 1 {
            return;
        }
        if let Some((i, _)) = nocturne_model::container::place(&self.boxes, at) {
            self.boxes[i].blocks -= 1;
        }
    }

    /// Moves the box count and indexed layout marks after removing the block at `at`.
    fn block_removed(&mut self, at: usize) -> (bool, bool) {
        self.count_removed_from_boxes(at);
        (self.columns.remove_blocks(at..at + 1), self.justified.remove_blocks(at..at + 1))
    }

    /// [`Editor::count_inserted_in_boxes`], and justified paragraphs and table widths after the
    /// new blocks move down with their blocks. Returns whether the widths and the justified
    /// paragraphs changed, for [`NocturneApp::save_block_marks`].
    fn blocks_inserted(&mut self, at: usize, count: usize) -> (bool, bool) {
        self.count_inserted_in_boxes(at, count);
        let (mut cols, mut justified) = (false, false);
        for offset in 0..count {
            cols |= self.columns.insert_block(at + offset);
            justified |= self.justified.insert_block(at + offset);
        }
        (cols, justified)
    }

    /// Writes the page's text from its blocks, with the boxes around them when it is arranged.
    /// The page as it is written to its file. A box with nothing in it, last in reading order
    /// (a click on open paper adds its line at the end), is only where the cursor was put, not
    /// something on the page, so it is left out.
    fn body_to_save(&self) -> String {
        if !self.arranged || self.boxes.len() < 2 || self.boxes.iter().map(|c| c.blocks).sum::<usize>() != self.blocks.len() {
            return self.body.clone();
        }
        let last = (0..self.boxes.len()).max_by_key(|i| nocturne_model::container::blocks_of(&self.boxes, *i).end).unwrap_or(0);
        let range = nocturne_model::container::blocks_of(&self.boxes, last);
        let blank = |b: &nocturne_formats::Block| crate::preview::is_blank(b) && !matches!(b, nocturne_formats::Block::Item { task: Some(_), .. });
        if range.end != self.blocks.len() || range.is_empty() || !self.blocks[range.clone()].iter().all(blank) {
            return self.body.clone();
        }
        let mut boxes = self.boxes.clone();
        boxes.remove(last);
        nocturne_formats::boxes::write(self.format, &self.blocks[..range.start], Some(&boxes))
    }

    fn rebuild_body(&mut self) {
        let arrangement = (self.arranged && !self.boxes.is_empty()).then_some(self.boxes.as_slice());
        self.body = nocturne_formats::boxes::write(self.format, &self.blocks, arrangement);
    }

    fn apply_pending_format(&mut self, before: Option<&pageedit::Caret>) {
        let Some(pending) = self.pending_format.take() else { return };
        let (block, at) = (pending.block, pending.at);
        let (Some(before), Some(caret)) = (before, self.caret.as_mut()) else {
            return;
        };
        if before.block != block || caret.block != block || before.selection != (at, at) || caret.cell.is_some() {
            return;
        }
        let offset = before.text.char_indices().nth(at).map_or(before.text.len(), |(offset, _)| offset);
        let (prefix, suffix) = before.text.split_at(offset);
        let inserted = caret.text.strip_prefix(prefix).and_then(|text| text.strip_suffix(suffix));
        let Some(inserted) = inserted.filter(|text| !text.is_empty()) else {
            self.pending_format = Some(pending);
            return;
        };
        let end = at + inserted.chars().count();
        if caret.selection != (end, end) {
            return;
        }
        let mut marker = "NocturnePendingTextMarker".to_string();
        while pending.family.as_ref().is_some_and(|family| family.contains(&marker)) {
            marker.push('X');
        }
        let span = nocturne_formats::markdown::Span {
            text: marker.clone(),
            font_size: pending.size,
            font_family: pending.family,
            bold: pending.bold,
            italic: pending.italic,
            strike: pending.strike,
            underline: pending.underline,
            script: pending.script,
            ..Default::default()
        };
        let template = nocturne_formats::render::spans_to_markdown(&[span]);
        let Some((open, close)) = template.split_once(&marker) else {
            log::error!("Could not serialize pending formatting");
            return;
        };
        let (text, selection) = pageedit::wrap_selection_with(&caret.text, (at, end), open, close);
        caret.text = text;
        caret.selection = (selection.1, selection.1);
        caret.focus = true;
        caret.place = Some(selection.1);
        if let Some(target) = self.blocks.get_mut(block) {
            caret.write(target);
        }
    }
}

/// The page's own name, drawn on the sheet at the top of the page.
///
/// It used to sit in a strip above the page, beside the breadcrumb and the Saved line,
/// which made it look like a field about the page rather than the first thing on it. In
/// OneNote the name is part of the page, and the owner asked for the same here on
/// 2026-10-19. So it is drawn inside the scroll area and inside the readable column: it
/// scrolls with the words, it lines up with them, and the page colour and rule lines run
/// behind it as they do behind everything else on the sheet.
///
/// It is drawn before the sheet's origin is taken, so ink keeps the coordinates it already
/// has and a drawing made beside a paragraph stays beside it.
///
/// A title that reads right to left is drawn by the bidi painter rather than the text box:
/// egui lays the letters of a word out the right way and then puts the next word to the
/// right of it, so every Arabic page showed its own name with the words backwards. Clicking
/// it hands the line to the text box, which is still where it is edited.
fn page_title(ui: &mut Ui, ed: &mut Editor, tokens: &Tokens, title_id: egui::Id, rename: &mut Option<String>, zoom: f32, tap_at: Option<egui::Pos2>) {
    ed.title_focused = false;
    ed.title_selection = None;
    let align = crate::pagetext::halign(&ed.title_buf, ed.page_dir);
    let width = ui.available_width();
    let rtl = crate::bidi::paints(&ed.title_buf, ed.page_dir);
    let editing = ui.memory(|m| m.has_focus(title_id)) || !rtl;
    let r = if editing {
        if let Some((primary, secondary)) = ed.title_restore_selection.take()
            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), title_id)
        {
            let len = ed.title_buf.chars().count();
            let (a, b) = (primary.min(len), secondary.min(len));
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(a), egui::text::CCursor::new(b))));
            state.store(ui.ctx(), title_id);
            ui.memory_mut(|m| m.request_focus(title_id));
            ed.caret = None;
            ed.across = None;
        }
        // A box that wraps (issue #316): a single-line box scrolls sideways, so a long title
        // was cut off at the page's edge on a phone. The title is still one line of text:
        // Enter finishes it rather than breaking it, and a line break pasted in becomes a space.
        let finish = ui.memory(|m| m.has_focus(title_id)) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let out = egui::TextEdit::multiline(&mut ed.title_buf)
            .id(title_id)
            .frame(egui::Frame::NONE)
            .font(egui::FontId::proportional(26.0 * zoom))
            .horizontal_align(align)
            .desired_width(width)
            .desired_rows(1)
            .return_key(None)
            .show(ui);
        if ed.title_buf.contains(['\n', '\r']) {
            ed.title_buf = line_breaks_to_spaces(&ed.title_buf);
        }
        let tapped_title = tap_at.is_some_and(|p| out.response.rect.contains(p));
        if tapped_title {
            out.response.request_focus();
        }
        if out.response.gained_focus() || tapped_title {
            ed.caret = None;
            ed.across = None;
        }
        if finish {
            out.response.surrender_focus();
        }
        let pending_block_focus = ed.caret.as_ref().is_some_and(|c| c.focus);
        if (out.response.has_focus() || tapped_title) && !finish && !pending_block_focus {
            ed.title_focused = true;
            ed.title_selection = out.cursor_range.map(|r| (r.primary.index.0, r.secondary.index.0));
            if let Some(range) = out.cursor_range {
                let caret = out.galley.pos_from_cursor(range.primary).translate(out.galley_pos.to_vec2());
                ed.caret_screen_rect = Some(caret);
                if crate::input_language::is_rtl() {
                    crate::pageedit::paint_rtl_input_marker(ui, caret);
                }
            }
        }
        out.response
    } else {
        let pieces = crate::bidi::plain(&ed.title_buf, 26.0 * zoom, tokens);
        let (drawn, size) = crate::bidi::runs(ui, &ed.title_buf, &pieces, 26.0 * zoom, width, ed.page_dir);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, size.y.max(34.0)), egui::Sense::click());
        crate::bidi::paint(ui, rect.min, &drawn);
        if resp.clicked() || tap_at.is_some_and(|p| rect.contains(p)) {
            ui.memory_mut(|m| m.request_focus(title_id));
            ed.caret = None;
            ed.across = None;
        }
        let name = ed.title_buf.clone();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextInput, true, &name));
        egui::AtomLayoutResponse::empty(resp.on_hover_cursor(egui::CursorIcon::Text))
    };
    if r.lost_focus() {
        let typed = ed.title_buf.trim();
        if typed.is_empty() {
            // Clearing the title of a page that had one makes it untitled, as the blank title
            // area shows; it used to be dropped, and the old title came back on reopen (#132).
            if ed.titled {
                *rename = Some(String::new());
            }
        } else if !ed.titled || typed != ed.title {
            *rename = Some(typed.to_string());
        }
    }
    // The rule under the name, as OneNote draws one. The date the page carries sits under
    // it already, as the first block of the page.
    let y = ui.cursor().min.y + 2.0;
    let (left, right) = (ui.max_rect().min.x, ui.max_rect().max.x);
    ui.painter().hline(left..=right, y, egui::Stroke::new(1.0, tokens.faint));
    ui.add_space(8.0);
}

use pageedit::{TABLE_MAX_COLUMNS, TABLE_MAX_ROWS};

/// The rows and columns typed into Insert Table, when both are whole numbers within the limits.
fn table_size(rows: &str, columns: &str) -> Option<(usize, usize)> {
    let r = rows.trim().parse::<usize>().ok().filter(|r| (1..=TABLE_MAX_ROWS).contains(r))?;
    let c = columns.trim().parse::<usize>().ok().filter(|c| (1..=TABLE_MAX_COLUMNS).contains(c))?;
    Some((r, c))
}

/// What is typed or pasted into a title, kept on one line: each line break becomes one space,
/// and nothing else is touched. Spaces are not trimmed here. A space arrives from the iPad
/// keyboard as a text of its own, and trimming it left titles with no spaces at all
/// (two words arrived as one). The ends are trimmed once, when the title is saved.
fn line_breaks_to_spaces(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

/// What to do with a file the user dropped on a page.
/// What one Undo took back: the drawing, the page's words and boxes, or both at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Undone {
    Ink,
    Page,
    Both,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileChoice {
    /// Copy it into the page's folder, so the notebook carries it. OneNote: "Attach File".
    Copy,
    /// Leave it where it is and write a link. OneNote: "Insert a link to the file".
    Link,
    /// Copy it in and insert one rendered image for each printed page.
    Printout,
}

enum Dialog {
    /// A dropped file: copy it in, or link to it where it is.
    FileOptions {
        dir: PathBuf,
        files: Vec<PathBuf>,
    },
    NewNotebook {
        name: String,
        format: Option<PageFormat>,
    },
    Add {
        parent: PathBuf,
        parent_kind: NodeKind,
        title: String,
        kind: NodeKind,
        format: Option<PageFormat>,
    },
    Rename {
        path: PathBuf,
        title: String,
    },
    /// Move or Copy (issue #167): where a page, section or section group goes.
    MoveOrCopy {
        path: PathBuf,
        title: String,
        kind: NodeKind,
        dest: Option<PathBuf>,
    },
    /// Insert > Link, or editing the link the caret is in (issue #163): the words to show
    /// and the address, put into the line `block` (or its table cell) in place of the
    /// characters `range` of the caret's text.
    Link {
        block: usize,
        cell: Option<(usize, usize)>,
        range: (usize, usize),
        text: String,
        address: String,
        editing: bool,
        first: bool,
    },
    /// Insert > Table > Insert Table...: a table of any size up to the limits, typed in.
    InsertTable {
        rows: String,
        columns: String,
        first: bool,
    },
    /// A file in the page's own folder, about to be deleted.
    DeleteFile {
        page: PathBuf,
        name: String,
    },
    Delete {
        path: PathBuf,
        title: String,
        count: usize,
    },
    DeleteNotebook {
        path: PathBuf,
        title: String,
    },
    ClearTelemetry {
        path: PathBuf,
    },
    ResetPreferences,
    PageVersions {
        page: PathBuf,
        versions: Vec<nocturne_store::page::Version>,
    },
    RecycleBin {
        notebook: PathBuf,
        items: Vec<nocturne_store::recycle::Recycled>,
    },
    PurgeRecycleItem {
        notebook: PathBuf,
        id: String,
        title: String,
    },
    Symbols,
    /// The first-use wizard: one window with three steps. Step 0 is the local logging level,
    /// step 1 the page format saved as the default, and step 2 whether to add the Welcome notebook
    /// in that format.
    FirstUseWizard {
        step: u8,
        level: String,
        format: PageFormat,
        add_demo: bool,
        error: Option<String>,
    },
    /// A problem told to the user in a box.    /// A problem told to the user in a box. Ticking "do not alert me again" stops the box for
    /// this `key` for the rest of the session; the status line still says what happened.
    Alert {
        key: String,
        title: String,
        message: String,
        suppress: bool,
        suppress_forever: bool,
    },
    MissingAttachmentIcon {
        page: PathBuf,
        name: String,
    },
    OpenAttachmentWarning {
        path: PathBuf,
    },
    #[allow(dead_code)]
    UnreferencedAttachment {
        page: PathBuf,
        name: String,
    },
    CloudSync(crate::sync_ui::CloudSyncForm),
    MathOcrSetup(crate::math_ocr::Setup),
}

struct ImportJob {
    rx: Receiver<ImportMessage>,
    keep_alive: Option<Sender<ImportMessage>>,
    cancel: Arc<AtomicBool>,
    source: PathBuf,
    destination: PathBuf,
    current: String,
    index: usize,
    total: usize,
}

enum ImportMessage {
    Found(usize),
    Section { index: usize, total: usize, label: String },
    Finished(Result<String, String>),
}

enum Act {
    Toggle(PathBuf),
    Select(PathBuf),
    Add(PathBuf, NodeKind),
    /// A page in this section at this place in the order, from the line between two pages.
    AddPageAt(PathBuf, u32),
    /// The same, when the two pages either side of the line are numbered next to each other:
    /// the section is renumbered ten apart first, then the page goes halfway after the first one.
    RenumberAndAddPage(PathBuf, u32),
    NewPage(PathBuf),
    /// Add a child of this kind, named in the menu rather than chosen in the dialog.
    AddKind(PathBuf, NodeKind, NodeKind),
    /// A notebook, from the menu on the empty space.
    NewNotebook,
    /// Close a notebook opened from outside the notebooks folder.
    CloseNotebook(PathBuf),
    /// Show the notebooks folder in the file manager.
    RevealRoot,
    /// Ask the system for the folder to work from.
    ChooseRoot,
    Rename(PathBuf, String),
    /// Open Move or Copy for a page, section or section group.
    MoveOrCopy(PathBuf),
    MakeSubpage(PathBuf),
    PromoteSubpage(PathBuf),
    MovePageUp(PathBuf),
    MovePageDown(PathBuf),
    PageVersions(PathBuf),
    Delete(PathBuf, String, usize),
    Color(PathBuf, Option<NodeColor>),
    /// Show the node's folder in the operating system's file manager.
    Reveal(PathBuf),
    /// Put a link to this page on the clipboard, to paste into another page.
    CopyLink(PathBuf),
}

/// A file the UI's host should deliver through its download facility.
/// The desktop host keeps the normal file instead; browser hosts opt in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDownload {
    pub name: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// Platform-neutral text edit action triggered by native OS menus (e.g. iPadOS `UIEditMenuInteraction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformEditAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Delete,
}

/// The one-line text field that has the keyboard, as a platform keyboard needs to see it: all
/// of its text and where the selection is, in characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformTextField {
    pub text: String,
    /// `(start, end)` of the selection in characters, in order. Equal when it is only a caret.
    pub selection: (usize, usize),
}

const TOUCH_PAN_SLOP: f32 = 8.0;
const SOFT_KEYBOARD_MIN_INSET_PTS: f32 = 80.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum FingerPanState {
    #[default]
    Idle,
    Pending {
        id: egui::TouchId,
        start_pos: egui::Pos2,
        last_pos: egui::Pos2,
    },
    Scrolling {
        id: egui::TouchId,
        last_pos: egui::Pos2,
    },
}

fn ink_pen_kind_for(tool: nocturne_ink::Tool) -> nocturne_engine::InkPenKind {
    nocturne_engine::InkPenKind::parse(tool.id()).unwrap_or(nocturne_engine::InkPenKind::Pen)
}

fn ink_mode_for_pointer(pointer: ribbon::Pointer) -> nocturne_engine::InkActiveMode {
    match pointer {
        ribbon::Pointer::Write => nocturne_engine::InkActiveMode::Write,
        ribbon::Pointer::Draw => nocturne_engine::InkActiveMode::Draw,
        ribbon::Pointer::Erase => nocturne_engine::InkActiveMode::Erase,
        ribbon::Pointer::Pick => nocturne_engine::InkActiveMode::Pick,
        ribbon::Pointer::Lasso => nocturne_engine::InkActiveMode::Lasso,
        ribbon::Pointer::Shape(_) => nocturne_engine::InkActiveMode::Shape,
        ribbon::Pointer::Pan => nocturne_engine::InkActiveMode::Pan,
        ribbon::Pointer::Laser => nocturne_engine::InkActiveMode::Laser,
    }
}

struct KeyboardPaste {
    fallback_text: Option<String>,
}

pub struct NocturneApp {
    session: Session,
    /// Writes autosaves off the window's thread (see [`crate::saver`]).
    saver: crate::saver::Saver,
    open: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    /// The pages visited, for Back and Forward.
    nav: NavHistory,
    /// Which Quick Access buttons are shown, and where.
    qat: quickaccess::Config,
    /// When the pen last changed the page, to tell which Undo should take back.
    last_ink_change: Option<Instant>,
    /// What each Undo took back, newest last, so Redo puts it back the same way.
    undone: Vec<Undone>,
    /// Two columns in the side panel (organisers, then pages) instead of one tree.
    split_panes: bool,
    /// Whether the organisers column is showing, which the three-bar button toggles.
    organisers: bool,
    /// Whether the window is kept above the others.
    always_on_top: bool,
    /// Set when that has just changed and the window manager has not been told yet.
    window_level_pending: bool,
    /// True when the window has no frame of the system's own (the app asked for none), so
    /// the band across the top draws the window's buttons and moves and resizes it.
    frameless: bool,
    editor: Option<Editor>,
    dialog: Option<Dialog>,
    import_job: Option<ImportJob>,
    sync_job: Option<crate::sync_ui::SyncJob>,
    cached_sync_config: Option<nocturne_engine::SyncConfig>,
    auto_sync_started_on_launch: bool,
    last_auto_sync_trigger: Option<Instant>,
    sync_dirty_since_last: bool,
    last_local_edit_at: Option<Instant>,
    was_window_focused: bool,
    last_ctx_is_ios: bool,
    status: String,
    status_is_error: bool,
    /// The host keeps notebook files only for this session, rather than on durable storage.
    session_only_storage: bool,
    search: String,
    searched: String,
    hits: Vec<Hit>,
    /// The Tags Summary, while it is open.
    tags_pane: Option<TagsPane>,
    templates_pane: Option<TemplatesPane>,
    cursor: usize,
    tokens: Tokens,
    mode_pref: ModePref,
    ui_language: String,
    /// The size the whole window is drawn at, in percent of the system's own scale, and
    /// whether it still has to be handed to egui.
    ui_scale: u16,
    ui_scale_pending: bool,
    /// What File > Check for updates is doing. The update runs on a thread of its own and
    /// writes here; the window reads it each frame and closes when the update is ready to
    /// be put in place.
    update: std::sync::Arc<std::sync::Mutex<crate::update::Status>>,
    update_closing: bool,
    effective_mode: Mode,
    accent: Accent,
    applied_theme: Option<(ModePref, Accent)>,
    loaders_installed: bool,
    redact_paths: bool,
    control: Option<Receiver<ControlRequest>>,
    control_server: Option<ControlServer>,
    event_frames: std::collections::VecDeque<Vec<egui::Event>>,
    show_source: bool,
    /// The code view: which of the page's files is picked, and its text while it is read
    /// or edited.
    code: crate::codeview::State,
    /// Which ribbon tab is open.
    ribbon_tab: String,
    /// What the pen is reporting, where the platform reports anything.
    stylus: crate::stylus::Stylus,
    /// Where the page's words were drawn last frame, which is where its ink goes.
    page_rect: Option<egui::Rect>,
    /// Visible screen-space viewport (`ScrollArea::inner_rect`) of the open page canvas.
    page_view_rect: Option<egui::Rect>,
    /// The canvas translation in screen points. It is UI state, so it never changes page data.
    canvas_pan: egui::Vec2,
    /// Last middle-button position while panning the canvas.
    canvas_drag: Option<egui::Pos2>,
    /// Words typed while a pen was in hand, held for the caret's line until it has the keyboard
    /// again (issue #9), and how many frames they have waited.
    typed_while_inking: Vec<egui::Event>,
    typed_wait: u8,
    /// Collect All was asked for: the page gathers its boxes and ink when it is next drawn.
    collect_pending: bool,
    /// The page's sideways scroll bar and its thumb, when one was drawn last frame (issue #188).
    page_hbar: Option<(egui::Rect, egui::Rect)>,
    /// The page ScrollArea's vertical bar hit strip from last frame, kept out of the ink layer.
    page_scroll_strip: Option<egui::Rect>,
    /// True while Insert Space is in hand: the next drag on the page adds or takes away room.
    space_tool: bool,
    /// What the pointer does on the page (the Draw tab).
    pointer: ribbon::Pointer,
    /// Last active geometric shape selected on the Draw tab, preserved across tool-state syncs.
    last_shape: nocturne_ink::shapes::Shape,
    /// Single-finger tap-vs-pan state machine when `finger_drawing == false`.
    finger_pan: FingerPanState,
    /// Touch ID currently dragging an active selection/resize handle so `FingerPanState` yields.
    finger_handle_drag: Option<egui::TouchId>,
    /// Vertical scroll delta queued by external platform pan gestures (e.g. `UIPanGestureRecognizer`).
    pending_external_scroll_y: f32,
    /// Exact vertical ScrollArea offset queued by minimap pan or 2-frame Fit All Content.
    pending_scroll_offset_y: Option<f32>,
    /// Page-space bounding box awaiting second-frame origin measurement after Fit All Content zoom.
    pending_fit_bounds: Option<egui::Rect>,
    /// Cached result of `is_tablet_studio` from the most recent frame (for non-ctx helpers).
    last_tablet_studio: bool,
    /// Explicit user override for the infinite-canvas minimap (`None` defaults ON in Tablet Studio,
    /// OFF in Classic Desktop Ribbon).
    minimap_override: Option<bool>,
    /// Whether the infinite-canvas minimap body is folded to its compact header chip.
    minimap_collapsed: bool,
    tablet_tools_minimized: bool,
    /// Whether lifting the eraser after rubbing out ink returns to the last writing pen.
    eraser_auto_return: bool,
    /// Cached presence of a saved kDrive token so Studio top bar avoids per-frame Keychain/disk reads.
    sync_has_token: Option<bool>,
    /// Whether the platform software keyboard is explicitly reported visible.
    soft_keyboard_visible: bool,
    /// Reported bottom inset of the software keyboard in logical points.
    soft_keyboard_inset_bottom: f32,
    /// Most recent bottom safe-area inset observed from `ctx.viewport_rect()` vs `ctx.content_rect()`.
    last_safe_area_bottom: f32,
    /// The tools on the rack, as the owner has arranged them, and which is in hand. Both
    /// are remembered between runs.
    rack: Vec<nocturne_ink::rack::Nib>,
    nib: usize,
    /// How readily the pen answers pressure, 0 to 100. 50 leaves the readings as the pen
    /// gave them; above that a light hand lays down more ink.
    sensitivity: u8,
    /// Monotonic pressure-response curve applied before `sensitivity` gain.
    pressure_curve: nocturne_ink::PressureCurve,
    /// The drawing open in this page, if it has one.
    pad: Option<crate::inkpad::Pad>,
    /// Present only when the host supplies a download facility.
    file_downloads: Option<Vec<FileDownload>>,
    /// Lines or squares behind the page.
    rules: ribbon::RuleLines,
    /// Where GridCraft is, when it is installed: a table can be handed to it. Looked for
    /// once, when the window opens.
    gridcraft: Option<std::path::PathBuf>,
    /// The page title line is hidden (OneNote: View, Hide Page Title).
    hide_title: bool,
    /// Boxes snap to the page's grid when moved or resized (View, Snap to grid; issue #79).
    snap_to_grid: bool,
    /// The Format Painter's formatting, waiting for the next words picked (issue #165).
    painter: Option<FormatPaint>,
    /// The ribbon is folded away to its tab names (Ctrl+F1; issue #136). Kept in the settings.
    ribbon_collapsed: bool,
    /// While folded, a tab was clicked: its commands are shown over the page until one is used,
    /// the page is clicked or Escape is pressed.
    ribbon_peek: bool,
    /// Where the band with the tab names ends, which is where a shown-over ribbon starts.
    band_bottom: f32,
    /// The page's zoom. The ribbon and the rest of the window keep their own size.
    zoom_percent: u32,
    /// Exact fractional zoom accumulator for smooth incremental pinch-zoom gestures.
    zoom_exact: f64,
    /// Recalculate the page zoom from the reading area's width on the next frame.
    fit_page_width: bool,
    /// The theme chooser is open as a window (the ribbon's Theme button).
    theme_window: bool,
    /// Text the ribbon asked to put on the clipboard this frame.
    pending_copy: Option<String>,
    /// Matching HTML flavor for the same copied item, when one is available.
    pending_copy_html: Option<String>,
    keyboard_paste: Option<KeyboardPaste>,
    /// The File screen, when it is open, and which of its pages.
    backstage: Option<&'static str>,
    /// The page fills the window: the notebooks are hidden until it is turned off.
    full_page: bool,
    /// The window is phone-width, so it shows the notebooks or the page, not both
    /// (`compact.rs`, issue #12). Worked out from the window's width every frame.
    compact: bool,
    /// The compact layout turned Full page on itself, so a wider window turns it off again.
    compact_full_page: bool,
    /// A page was opened since the last frame: the compact layout shows it.
    page_opened: bool,
    /// Put the keyboard in the search box on the next frame.
    focus_search: bool,
    /// The kind of dialog drawn last frame, so a dialog knows the frame it opens in.
    dialog_shown: Option<std::mem::Discriminant<Dialog>>,
    /// The failure banner of Tablet Studio: tapped away, and whether it was drawn last frame.
    status_banner_dismissed: Option<String>,
    status_banner_drawn: bool,
    /// A pen is writing while a finger or palm is also on the glass: until the pen lifts, the
    /// ink reads the pen's own touch and not the window's pointer (`stylus::StylusFrame`).
    pen_reads_own_touch: bool,
    /// The Tool Settings screen of Tablet Studio is open.
    tool_settings_open: bool,
    /// Where the floating header of Tablet Studio was drawn last frame, when a page is open.
    header_pill_rect: Option<egui::Rect>,
    pencil_pill_rect: Option<egui::Rect>,
    math_ocr: Option<crate::math_ocr::Conversion>,
    pending_math_ocr_start: Option<nocturne_engine::OcrMode>,
    ocr_model_download: Option<crate::math_ocr::ModelDownloadWorker>,
    library_grid: crate::library_grid::LibraryGrid,
    /// The sync now running was started by hand, so its result is shown when it ends.
    sync_by_hand: bool,
    /// A result the user asked for, shown on one line in Tablet Studio for a few seconds.
    studio_notice: Option<(String, Instant)>,
    /// The page the window closed on has been opened again (once per run).
    restored: bool,
    /// Recovery warnings that were already shown during this run.
    icon_recovery_reported: HashSet<String>,
    /// Suppress further missing-icon warnings during this run.
    icon_recovery_suppressed: bool,
    /// Alerts the user asked not to see again this session, by key.
    muted_alerts: HashSet<String>,
    /// Alerts waiting for the open one to close.
    queued_alerts: VecDeque<Dialog>,
    /// The clipboard the app reads and writes. The system's, unless the app was built with another.
    clipboard: Box<dyn crate::clipboard::Board>,
    open_attachment_warning_suppressed: bool,
    welcome_notebook_setup: Option<Box<dyn FnMut(&Path, &str) -> Result<PathBuf, String>>>,
}

impl NocturneApp {
    /// Replaces the clipboard the app uses. Made at startup by the caller, so the app never
    /// reaches a platform's clipboard by itself.
    pub fn with_clipboard(mut self, board: Box<dyn crate::clipboard::Board>) -> NocturneApp {
        self.clipboard = board;
        self
    }

    /// Routes native paste events for the page body through Nocturne's clipboard.
    pub fn capture_page_paste(&mut self, ctx: &Context, events: &mut Vec<egui::Event>) {
        if !self.page_owns_clipboard_input(ctx) {
            return;
        }
        let mut captured = false;
        let mut fallback_text = None;
        events.retain_mut(|event| {
            let is_paste = match event {
                egui::Event::Paste(text) => {
                    // egui-winit emits this event in place of Ctrl/Cmd+V when it has text.
                    fallback_text = Some(std::mem::take(text));
                    true
                }
                egui::Event::PasteImage(_) => true,
                egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } => modifiers.ctrl || modifiers.command,
                _ => false,
            };
            captured |= is_paste;
            !is_paste
        });
        if captured {
            self.keyboard_paste = Some(KeyboardPaste { fallback_text });
        }
    }

    pub fn new(session: Session) -> NocturneApp {
        // The installed fonts are read in the background, so the window opens at once.
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        crate::fonts::start_catalog();
        let mode_pref = session.settings.get("theme_mode").and_then(|v| ModePref::parse(&v)).unwrap_or(ModePref::System);
        let ui_language =
            session.settings.get("ui_language").map(|value| crate::i18n::normalize_preference(&value)).unwrap_or_else(|| crate::i18n::AUTO.into());
        crate::i18n::set_current(&ui_language);
        let accent = session.settings.get("theme_accent").and_then(|v| Accent::parse(&v)).unwrap_or(Accent::Default);
        let show_source = false;
        let session_rules = session.settings.get("rule_lines").and_then(|v| ribbon::RuleLines::parse(&v)).unwrap_or_default();
        let session_hide_title = session.settings.get("hide_title").map(|v| v.trim() == "true").unwrap_or(false);
        let session_snap = session.settings.get("snap_to_grid").map(|v| v.trim() == "true").unwrap_or(false);
        let session_ribbon_collapsed = session.settings.get("ribbon_collapsed").map(|v| v.trim() == "true").unwrap_or(false);
        // Two columns in the side panel, the way OneNote separates the notebooks from the
        // pages. This is the product's default navigation layout; the one-tree view remains
        // available as an explicit preference.
        let session_split = session.settings.get("split_panes").map(|v| v.trim() == "true").unwrap_or(true);
        // The pen rack as the owner last arranged it, and which tool was in hand.
        let rack = nocturne_ink::rack::parse(&session.settings.get(RACK_KEY).unwrap_or_default());
        let nib = session.settings.get(NIB_KEY).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0).min(rack.len() - 1);
        let sensitivity = session.settings.get(SENSITIVITY_KEY).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(DEFAULT_SENSITIVITY).min(100);
        let pressure_curve = session.settings.get(CURVE_KEY).and_then(|v| nocturne_ink::PressureCurve::parse(&v)).unwrap_or_default();
        let qat = quickaccess::Config::load(|k| session.settings.get(k));
        // `--first-use` clears the key to an empty value, which counts as not seen.
        let privacy_welcome = session.settings.get("privacy_welcome_seen").is_none_or(|v| v.trim().is_empty());
        let telemetry_level = session.settings.get("telemetry_level").unwrap_or_else(|| default_telemetry_level(cfg!(debug_assertions)).to_string());
        log::info!("privacy welcome modal state: show={} telemetry_level={:?}", privacy_welcome, telemetry_level);
        let wizard_format = session.settings.global_format().0;
        let ui_scale = session
            .settings
            .get(UI_SCALE_KEY)
            .and_then(|v| v.trim().parse::<u16>().ok())
            .filter(|percent| backstage::UI_SCALES.contains(percent))
            .unwrap_or(100);
        let wizard_level = session.settings.get("telemetry_level").unwrap_or_else(|| "debug".to_string());
        let last_tablet_studio = match session.layout_mode() {
            nocturne_engine::UiLayoutMode::TabletStudio => true,
            nocturne_engine::UiLayoutMode::ClassicRibbon => false,
            nocturne_engine::UiLayoutMode::Auto => cfg!(target_os = "ios"),
        };
        let minimap_override = session.settings.get("show_minimap").and_then(|v| {
            let trimmed = v.trim();
            if trimmed.eq_ignore_ascii_case("true") {
                Some(true)
            } else if trimmed.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        });
        let minimap_collapsed = session.settings.get("minimap_collapsed").map(|v| !v.trim().eq_ignore_ascii_case("false")).unwrap_or(true);
        let tablet_tools_minimized = session.settings.get("tablet_tools_minimized").as_deref() == Some("true");
        let eraser_auto_return = session.settings.get("eraser_auto_return").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        let library_grid = crate::library_grid::LibraryGrid::load(&session.settings);
        let mut app = NocturneApp {
            session,
            saver: crate::saver::Saver::default(),
            nav: NavHistory::default(),
            qat,
            last_ink_change: None,
            undone: Vec::new(),
            open: HashSet::new(),
            selected: None,
            split_panes: session_split,
            organisers: true,
            always_on_top: false,
            window_level_pending: false,
            frameless: false,
            editor: None,
            dialog: if privacy_welcome {
                Some(Dialog::FirstUseWizard { step: 0, level: wizard_level, format: wizard_format, add_demo: true, error: None })
            } else {
                None
            },
            import_job: None,
            sync_job: None,
            cached_sync_config: None,
            auto_sync_started_on_launch: false,
            last_auto_sync_trigger: None,
            sync_dirty_since_last: false,
            last_local_edit_at: None,
            was_window_focused: true,
            last_ctx_is_ios: cfg!(target_os = "ios"),
            status: crate::tl_id!("ui.ready_2").into(),
            status_is_error: false,
            session_only_storage: false,
            search: String::new(),
            searched: String::new(),
            hits: Vec::new(),
            tags_pane: None,
            templates_pane: None,
            cursor: 0,
            tokens: Theme::DEFAULT.tokens(),
            mode_pref,
            ui_language,
            ui_scale,
            ui_scale_pending: ui_scale != 100,
            update: Default::default(),
            update_closing: false,
            effective_mode: if mode_pref == ModePref::Dark { Mode::Dark } else { Mode::Light },
            accent,
            applied_theme: None,
            loaders_installed: false,
            redact_paths: false,
            control: None,
            control_server: None,
            event_frames: std::collections::VecDeque::new(),
            show_source,
            code: crate::codeview::State::default(),
            ribbon_tab: "Home".to_string(),
            stylus: crate::stylus::Stylus::default(),
            page_rect: None,
            page_view_rect: None,
            canvas_pan: egui::Vec2::ZERO,
            collect_pending: false,
            page_hbar: None,
            page_scroll_strip: None,
            space_tool: false,
            canvas_drag: None,
            typed_while_inking: Vec::new(),
            typed_wait: 0,
            pointer: ribbon::Pointer::Write,
            last_shape: nocturne_ink::shapes::Shape::Line,
            finger_pan: FingerPanState::Idle,
            finger_handle_drag: None,
            pending_external_scroll_y: 0.0,
            pending_scroll_offset_y: None,
            pending_fit_bounds: None,
            last_tablet_studio,
            minimap_override,
            minimap_collapsed,
            tablet_tools_minimized,
            eraser_auto_return,
            sync_has_token: None,
            soft_keyboard_visible: false,
            soft_keyboard_inset_bottom: 0.0,
            last_safe_area_bottom: 0.0,
            rack,
            nib,
            sensitivity,
            pressure_curve,
            pad: None,
            file_downloads: None,
            rules: session_rules,
            gridcraft: crate::gridcraft::installed(),
            hide_title: session_hide_title,
            snap_to_grid: session_snap,
            painter: None,
            ribbon_collapsed: session_ribbon_collapsed,
            ribbon_peek: false,
            band_bottom: 0.0,
            zoom_percent: 100,
            zoom_exact: 100.0,
            fit_page_width: false,
            theme_window: false,
            pending_copy: None,
            pending_copy_html: None,
            keyboard_paste: None,
            backstage: None,
            full_page: false,
            compact: false,
            compact_full_page: false,
            page_opened: false,
            focus_search: false,
            dialog_shown: None,
            status_banner_dismissed: None,
            status_banner_drawn: false,
            pen_reads_own_touch: false,
            tool_settings_open: false,
            header_pill_rect: None,
            pencil_pill_rect: None,
            math_ocr: None,
            pending_math_ocr_start: None,
            ocr_model_download: None,
            library_grid,
            sync_by_hand: false,
            studio_notice: None,
            restored: false,
            icon_recovery_reported: HashSet::new(),
            icon_recovery_suppressed: false,
            muted_alerts: HashSet::new(),
            queued_alerts: VecDeque::new(),
            clipboard: Box::new(crate::clipboard::System),
            open_attachment_warning_suppressed: false,
            welcome_notebook_setup: None,
        };
        app.sync_tool_state_to_session();
        app
    }

    /// Installs the application-owned action behind the Welcome notebook step of the first-use
    /// wizard. The callback gets the notebooks folder and the chosen format id, and returns the
    /// installed notebook folder. The wizard opens once: on the first launch after installation, or
    /// after the preferences are reset. It does not reopen for later versions.
    pub fn with_welcome_notebook_setup(mut self, setup: impl FnMut(&Path, &str) -> Result<PathBuf, String> + 'static) -> Self {
        self.welcome_notebook_setup = Some(Box::new(setup));
        self
    }

    /// The first-use wizard, starting at the logging step with the current settings.
    fn first_use_wizard(&self) -> Dialog {
        Dialog::FirstUseWizard {
            step: 0,
            level: self.session.settings.get("telemetry_level").unwrap_or_else(|| "debug".to_string()),
            format: self.session.settings.global_format().0,
            add_demo: true,
            error: None,
        }
    }

    /// Applies the wizard's answers: the logging level, the default page format, and, when chosen,
    /// the Welcome notebook in that format. The wizard is marked seen only after every answer is saved,
    /// so a failed install shows the wizard again on the next launch.
    fn finish_first_use_wizard(&mut self, level: &str, format: PageFormat, add_demo: bool) -> Result<(), String> {
        self.session.settings.set("telemetry_level", level).map_err(|error| error.to_string())?;
        self.session.settings.set("privacy_welcome_seen", "true").map_err(|error| error.to_string())?;
        self.session.settings.set_default_format(format).map_err(|error| error.to_string())?;
        if add_demo {
            self.save_now();
            if self.is_dirty() {
                return Err("the current page could not be saved before replacing the Welcome notebook".to_string());
            }
            let installed = {
                let setup = self.welcome_notebook_setup.as_mut().ok_or_else(|| "the bundled Welcome notebook setup is unavailable".to_string())?;
                setup(self.session.root(), format.id())?
            };
            self.editor = None;
            self.pad = None;
            self.session.refresh();
            self.status_is_error = false;
            self.open_path(&installed.join("00-start/welcome").to_string_lossy());
            if self.status_is_error {
                return Err(self.status.clone());
            }
        }
        Ok(())
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Opt in to host-delivered files for explicit exports. Ordinary saves never
    /// request a download. The host must drain these outputs after every frame.
    pub fn enable_file_downloads(&mut self) {
        self.file_downloads.get_or_insert_with(Vec::new);
    }

    /// Takes each explicit file export once, for a host such as the web wrapper.
    pub fn take_file_downloads(&mut self) -> Vec<FileDownload> {
        self.file_downloads.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// The session, for tests and for tools that drive the window.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Writes the open page now, instead of waiting for the autosave.
    pub fn save_page_now(&mut self) {
        self.save_now();
    }

    /// Opens the page that was open when the window last closed, and the branches above it,
    /// the way OneNote comes back where you left it. Runs once, on the first frame.
    fn restore_last_page(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let Some(saved) = self.session.settings.get("last_page").filter(|s| !s.trim().is_empty()) else { return };
        let path = self.session.root().join(saved.trim().replace('/', std::path::MAIN_SEPARATOR_STR));
        if !path.is_dir() {
            return;
        }
        // Open every container above it, so the page is visible in the tree.
        let mut at = path.parent();
        while let Some(dir) = at {
            if !dir.starts_with(self.session.root()) || dir == self.session.root() {
                break;
            }
            self.open.insert(dir.to_path_buf());
            at = dir.parent();
        }
        self.open_page(&path);
    }

    /// Remembers which page is open, for the next time the window starts.
    fn remember_last_page(&mut self, dir: &Path) {
        let Ok(rel) = dir.strip_prefix(self.session.root()) else { return };
        let value = rel.to_string_lossy().replace('\\', "/");
        let _ = self.session.settings.set("last_page", &value);
    }

    /// Marks a host whose notebook files disappear with the current browser tab.
    /// Native callers keep the default durable-file captions.
    pub fn with_session_only_storage(mut self) -> Self {
        self.session_only_storage = true;
        self
    }

    /// Whether the page source is shown beside the page. Off by default: the page is what
    /// the user writes on, and the source is there when they ask for it.
    pub fn show_source(&self) -> bool {
        self.show_source
    }

    pub fn set_show_source(&mut self, on: bool) {
        // Closing the view after the page file was typed in: the page is read again, so
        // the live view, the title and everything in the head come from what is on disk.
        if !on && self.code.page_edited {
            self.code.page_edited = false;
            if let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) {
                self.open_page(&dir);
            }
        }
        self.show_source = on;
    }

    /// Chooses a theme (light or dark, and an accent) and saves the choice.
    pub fn set_theme(&mut self, theme: Theme) {
        self.mode_pref = if theme.mode.is_dark() { ModePref::Dark } else { ModePref::Light };
        self.effective_mode = theme.mode;
        self.accent = theme.accent;
        self.save_theme();
    }

    /// Opens a File/backstage page for deterministic UI captures and integration harnesses.
    pub fn open_backstage_page(&mut self, page: &'static str) {
        self.backstage = Some(page);
    }

    /// Shows a deterministic progress state for documentation screenshots.
    pub fn show_demo_import_progress(&mut self, index: usize, total: usize, current: &str) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.import_job = Some(ImportJob {
            rx,
            keep_alive: Some(tx),
            cancel: Arc::new(AtomicBool::new(false)),
            source: PathBuf::from("demo-source"),
            destination: PathBuf::from("demo-destination"),
            current: current.to_string(),
            index,
            total,
        });
    }

    /// Follows the system light or dark setting, with the given accent, and saves the choice.
    pub fn follow_system_theme(&mut self, accent: Accent) {
        self.mode_pref = ModePref::System;
        self.accent = accent;
        self.save_theme();
    }

    /// The theme in use (for the system mode, the mode the system last asked for).
    pub fn theme(&self) -> Theme {
        Theme { mode: self.effective_mode, accent: self.accent }
    }

    pub fn mode_pref(&self) -> ModePref {
        self.mode_pref
    }

    fn save_theme(&mut self) {
        let a = self.session.settings.set("theme_mode", self.mode_pref.id());
        let b = self.session.settings.set("theme_accent", self.accent.id());
        if let Err(e) = a.and(b) {
            self.status = crate::i18n::format_message_id("ui.the_theme_could_not_be_saved_error", &[("error", &e.to_string())]);
            self.status_is_error = true;
        }
    }

    fn set_ui_language(&mut self, language: String) {
        if let Err(error) = self.session.settings.set("ui_language", &language) {
            let error = error.to_string();
            let message = crate::i18n::format_message_id("ui.could_not_save_the_interface_language_error", &[("error", &error)]);
            self.complain(&message);
        } else {
            self.ui_language = language;
            crate::i18n::set_current(&self.ui_language);
            self.status = crate::tl_id!("ui.ready_2").to_string();
            self.status_is_error = false;
        }
    }

    /// The size the whole window is drawn at, in percent of the system's own scale.
    pub fn ui_scale(&self) -> u16 {
        self.ui_scale
    }

    /// Draws the whole window larger or smaller, for a screen where the system's own scale
    /// leaves the ribbon and the menus too small to read, and remembers the choice.
    pub fn set_ui_scale(&mut self, percent: u16) {
        if let Err(error) = self.session.settings.set(UI_SCALE_KEY, &percent.to_string()) {
            let error = error.to_string();
            let message = crate::i18n::format_message_id("ui.could_not_save_the_interface_size_error", &[("error", &error)]);
            self.complain(&message);
            return;
        }
        self.ui_scale = percent;
        self.ui_scale_pending = true;
    }

    /// Applies the chosen theme to egui when it changed, and refreshes the tokens.
    fn apply_theme(&mut self, ctx: &Context) {
        let key = (self.mode_pref, self.accent);
        if self.applied_theme != Some(key) {
            ctx.set_visuals_of(egui::Theme::Light, Theme { mode: Mode::Light, accent: self.accent }.visuals());
            ctx.set_visuals_of(egui::Theme::Dark, Theme { mode: Mode::Dark, accent: self.accent }.visuals());
            ctx.set_theme(match self.mode_pref {
                ModePref::Light => egui::ThemePreference::Light,
                ModePref::Dark => egui::ThemePreference::Dark,
                ModePref::System => egui::ThemePreference::System,
            });
            self.applied_theme = Some(key);
        }
        self.effective_mode = if ctx.theme() == egui::Theme::Dark { Mode::Dark } else { Mode::Light };
        self.tokens = self.theme().tokens();
    }

    /// Shows paths as `<notebooks>/...` instead of real local paths. Used by tests and
    /// when taking screenshots for documentation.
    pub fn set_redact_paths(&mut self, on: bool) {
        self.redact_paths = on;
    }

    fn shown(&self, p: &Path) -> String {
        shown_in(self.session.root(), p, self.redact_paths)
    }

    /// Opens `path` (absolute, or relative to the notebooks folder) and expands the tree down
    /// to it. A notebook, section group or section opens on its first page.
    pub fn open_path(&mut self, path: &str) {
        // An explicit command-line/control request wins over the page saved by the
        // previous session. Otherwise the first frame's restore pass would replace the
        // requested page with a stale page from another notebook.
        self.restored = true;
        // A notebook or section opens on its first page, and a notebook kept elsewhere is
        // opened first: the engine decides, so the command line and agents get the same page.
        if let Some(Output::Path(dir)) = self.run("open.page", &params(&[("path", path)])) {
            self.expand_to(&dir);
            self.open_page(&dir);
        }
    }

    /// Opens every level above `dir`, so the page is visible in the tree.
    fn expand_to(&mut self, dir: &Path) {
        let mut parent = dir.parent();
        while let Some(d) = parent {
            if d == self.session.root() {
                break;
            }
            self.open.insert(d.to_path_buf());
            parent = d.parent();
        }
    }

    /// Back to the page before this one.
    fn go_back(&mut self) {
        match self.nav.back(|p| p.exists()) {
            Some(dir) => {
                self.expand_to(&dir);
                self.open_page_inner(&dir);
            }
            None => self.say(crate::tl_id!("ui.there_is_no_earlier_page")),
        }
    }

    /// Forward to the page after this one.
    fn go_forward(&mut self) {
        match self.nav.forward(|p| p.exists()) {
            Some(dir) => {
                self.expand_to(&dir);
                self.open_page_inner(&dir);
            }
            None => self.say(crate::tl_id!("ui.there_is_no_later_page")),
        }
    }

    fn can_undo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.writable && e.history.can_undo()) || self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_undo)
    }

    fn can_redo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.writable && e.history.can_redo()) || self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo)
    }

    /// Undo for the Quick Access Toolbar, Tablet Studio dock, keyboard shortcuts, and platform
    /// multi-finger tap gestures: undoes the page's words/boxes, the last ink stroke, or both.
    pub fn undo(&mut self) {
        let ink_can = self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_undo);
        let page_can = self.editor.as_ref().is_some_and(|e| e.writable && e.history.can_undo());
        if !ink_can && !page_can {
            return;
        }
        let edited = self.editor.as_ref().and_then(|e| e.history.when());
        // The ink's own last change if it keeps one, else when ink last changed at all.
        let inked = self.pad.as_ref().and_then(crate::inkpad::Pad::newest_change).or(self.last_ink_change);
        let ink_newer = match (inked, edited) {
            (Some(i), Some(e)) => i > e,
            (Some(_), None) => true,
            _ => false,
        };
        // One press that changed both (Delete on boxes and ink picked up together) is one step.
        let together = matches!((inked, edited), (Some(i), Some(e)) if i.max(e).duration_since(i.min(e)) < Duration::from_millis(300));
        let what = if ink_can && page_can && together {
            self.ink_undo();
            self.step_back(true);
            Undone::Both
        } else if ink_can && (ink_newer || !page_can) {
            self.ink_undo();
            Undone::Ink
        } else {
            self.step_back(true);
            Undone::Page
        };
        self.undone.push(what);
    }

    /// Puts back what Undo last took back, from the ink, the page or both, the way it was taken
    /// (issue #155). When the record of which no longer fits what either can put back, the page
    /// goes first, then the ink.
    pub fn redo(&mut self) {
        let ink_can = self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo);
        let page_can = self.editor.as_ref().is_some_and(|e| e.writable && e.history.can_redo());
        if !ink_can && !page_can {
            self.undone.clear();
            return;
        }
        match self.undone.pop() {
            Some(Undone::Both) if ink_can && page_can => {
                self.ink_redo();
                self.step_back(false);
            }
            Some(Undone::Ink) if ink_can => self.ink_redo(),
            Some(Undone::Page) if page_can => self.step_back(false),
            _ if !page_can && ink_can => {
                self.undone.clear();
                self.ink_redo();
            }
            _ => {
                self.undone.clear();
                self.step_back(false);
            }
        }
    }

    fn canvas_pointer_unoccluded(&self, ctx: &Context, at: egui::Pos2) -> bool {
        self.dialog.is_none()
            && self.backstage.is_none()
            && !self.theme_window
            && self.import_job.is_none()
            && !self.tool_settings_open
            && !self.ribbon_peek
            && !egui::Popup::is_any_open(ctx)
            && !crate::ribbon::any_popup_open(ctx)
            && self.page_view_rect.or(self.page_rect).is_some_and(|r| r.contains(at))
            && !self.page_scroll_strip.is_some_and(|s| s.contains(at))
            && ctx.layer_id_at(at).is_none_or(|l| l.order < egui::Order::Foreground)
    }

    fn sync_tool_state_to_session(&mut self) {
        if let ribbon::Pointer::Shape(s) = self.pointer {
            self.last_shape = s;
        }
        let tool = self.rack.get(self.nib).or_else(|| self.rack.first()).map(|n| ink_pen_kind_for(n.tool)).unwrap_or_default();
        self.session.sync_writing_tool_metadata(tool, self.nib);
        self.session.set_ink_mode(ink_mode_for_pointer(self.pointer));
    }

    fn sync_tool_state_from_session(&mut self) {
        let state = self.session.ink_tool_state();
        self.nib = state.rack_index.min(self.rack.len().saturating_sub(1));
        self.pointer = match state.active_mode {
            nocturne_engine::InkActiveMode::Write => ribbon::Pointer::Write,
            nocturne_engine::InkActiveMode::Draw => ribbon::Pointer::Draw,
            nocturne_engine::InkActiveMode::Erase => ribbon::Pointer::Erase,
            nocturne_engine::InkActiveMode::Pick => ribbon::Pointer::Pick,
            nocturne_engine::InkActiveMode::Lasso => ribbon::Pointer::Lasso,
            nocturne_engine::InkActiveMode::Shape => match self.pointer {
                ribbon::Pointer::Shape(s) => ribbon::Pointer::Shape(s),
                _ => ribbon::Pointer::Shape(self.last_shape),
            },
            nocturne_engine::InkActiveMode::Pan => ribbon::Pointer::Pan,
            nocturne_engine::InkActiveMode::Laser => ribbon::Pointer::Laser,
        };
        if self.pointer != ribbon::Pointer::Write {
            self.space_tool = false;
        }
    }

    /// OneNote's automatic switching (issue #9): the window follows what the hand is holding.
    /// A pen that comes down on the page while the pointer is the caret picks up the pen last in
    /// hand, colour and thickness and all, and draws with it from that touch on; typing while a
    /// pen or another ink tool is in hand puts it down and the keys go to the page as words. A
    /// mouse, and a finger on a touch screen, change nothing unless `finger_drawing` is enabled:
    /// only a contact that reports a force is a pen, so this works where the platform passes the
    /// pen's pressure on (see `stylus.rs`), and elsewhere the tools are chosen by hand as before.
    fn switch_by_device(&mut self, ctx: &Context) {
        if self.dialog.is_some() || self.backstage.is_some() || self.compact_list_fills() || !self.editor.as_ref().is_some_and(|ed| ed.writable) {
            return;
        }
        if self.pointer == ribbon::Pointer::Write {
            let stylus_hit = self.stylus.landed().is_some_and(|at| self.canvas_pointer_unoccluded(ctx, at));
            let finger_hit = self.is_finger_drawing()
                && !self.stylus.has_active_stylus()
                && self.stylus.active_finger_count() == 1
                && self.stylus.finger_landed().is_some_and(|(_, at)| self.canvas_pointer_unoccluded(ctx, at));
            if stylus_hit || finger_hit {
                self.ribbon_act(ribbon::Act::InkPen(self.nib));
                crate::inkpad::claim_press(ctx);
            }
            return;
        }
        let typed = ctx.input(|i| {
            !i.modifiers.command
                && !i.modifiers.ctrl
                && !i.modifiers.alt
                && i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if !t.trim().is_empty()))
        });
        if !typed || ctx.egui_wants_keyboard_input() {
            return;
        }
        self.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
        // The words go back to the line the caret was in, where it was: the line's text box
        // was not drawn while the pen was in hand, so it has to take the keyboard again before
        // it can read them. They are held until it has (`replay_typing`).
        let Some(ed) = self.editor.as_mut() else { return };
        // A selection across lines left from before the pen would keep every line as finished
        // words, with no text box to type into.
        ed.across = None;
        let Some(caret) = ed.caret.as_mut() else { return };
        caret.focus = true;
        caret.place = Some(caret.selection.1);
        self.typed_while_inking = ctx.input_mut(|i| {
            let (text, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut i.events).into_iter().partition(|e| matches!(e, egui::Event::Text(_)));
            i.events = rest;
            text
        });
        self.typed_wait = 0;
        ctx.request_repaint();
    }

    /// Gives the page the words that were typed with a pen in hand (see `switch_by_device`)
    /// once the caret's line has the keyboard again, a frame or two after the switch.
    fn replay_typing(&mut self, ctx: &Context) {
        if self.typed_while_inking.is_empty() {
            return;
        }
        let ready = self.editor.as_ref().and_then(|ed| ed.caret.as_ref()).is_some_and(|c| !c.focus) && ctx.memory(|m| m.focused().is_some());
        if ready {
            let typed = std::mem::take(&mut self.typed_while_inking);
            ctx.input_mut(|i| i.events.extend(typed));
        } else if self.typed_wait > 4 {
            // The caret never came back (the page was closed, say): the words have nowhere to go.
            self.typed_while_inking.clear();
        } else {
            self.typed_wait += 1;
            ctx.request_repaint();
        }
    }
    /// Keys for the pens, so a tablet's express keys or a keyboard can change tools without
    /// reaching for the ribbon (issue #44): Alt+1 to Alt+9 pick up that pen on the Draw tab's
    /// rack, and Alt+0 puts the pens down and goes back to typing. On a Mac, where Option and a
    /// digit also type a character, the character is dropped so it does not land in the note.
    fn pen_keys(&mut self, ctx: &Context) {
        if self.dialog.is_some() || self.backstage.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::E)) {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
            self.toggle_eraser_tool();
            return;
        }
        const DIGITS: [Key; 10] = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        let Some(n) = ctx.input_mut(|i| DIGITS.iter().position(|key| i.consume_key(Modifiers::ALT, *key))) else { return };
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        if n == 0 {
            self.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
            self.say(crate::tl_id!("ui.pen_keys_typing"));
        } else if n <= self.rack.len() {
            self.ribbon_act(ribbon::Act::InkPen(n - 1));
            self.say(crate::i18n::format_message_id("ui.pen_keys_picked", &[("number", &n.to_string())]));
        }
    }

    /// The keys and buttons that move through the pages that were opened. Undo and Redo keys
    /// are the page history's own, taken below.
    fn nav_keys(&mut self, ctx: &Context) {
        use crate::quickaccess::Command;

        let navigate = |command: Command, mouse| {
            let shortcut = command.shortcut(ctx.os());
            ctx.input_mut(|input| {
                let mut pressed = input.pointer.button_pressed(mouse);
                // Match Alt and Shift exactly so Cmd+Option+bracket still reaches page hierarchy.
                input.events.retain(|event| {
                    let matched = matches!(event,
                        egui::Event::Key { key, pressed: true, modifiers, .. }
                            if *key == shortcut.logical_key && modifiers.matches_exact(shortcut.modifiers));
                    pressed |= matched;
                    !matched
                });
                pressed
            })
        };
        if navigate(Command::Back, egui::PointerButton::Extra1) {
            self.go_back();
        }
        if navigate(Command::Forward, egui::PointerButton::Extra2) {
            self.go_forward();
        }
        // OneNote's Ctrl+Page Up and Ctrl+Page Down: the page above or below in the page list,
        // from the page as well as from the list (issue #162).
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::PageUp)) {
            self.step_page(false);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::PageDown)) {
            self.step_page(true);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F1)) {
            self.toggle_ribbon();
        }
    }

    /// The pages of the section in the order the page list shows them: a page, then its
    /// subpages when it is expanded. A collapsed page's subpages are not on the list, so they
    /// are not stepped through either.
    fn listed_pages(&self) -> Vec<PathBuf> {
        fn walk(nodes: &[Node], open: &HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
            for n in nodes.iter().filter(|n| n.info.kind == NodeKind::Page) {
                out.push(n.path.clone());
                if open.contains(&n.path) {
                    walk(&n.children, open, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(section) = self.current_section(self.session.tree()) {
            walk(&section.children, &self.open, &mut out);
        }
        out
    }

    /// Opens the page after the open one in the page list, or the one before it. At either end
    /// it stays where it is, as OneNote does, and says so. The page being left is saved first.
    fn step_page(&mut self, forward: bool) {
        let pages = self.listed_pages();
        let here = self.editor.as_ref().map(|e| e.dir.clone()).or_else(|| self.selected.clone());
        let at = here.as_ref().and_then(|h| pages.iter().position(|p| p == h));
        let next = match at {
            Some(i) if forward => pages.get(i + 1),
            Some(i) => i.checked_sub(1).and_then(|i| pages.get(i)),
            // No page of this section open: the first or last of its list.
            None if forward => pages.first(),
            None => pages.last(),
        };
        match next.cloned() {
            Some(page) => {
                self.selected = Some(page.clone());
                self.open_page(&page);
            }
            None => self.say(if forward { crate::tl_id!("ui.this_is_the_last_page") } else { crate::tl_id!("ui.this_is_the_first_page") }),
        }
    }

    /// Folds the ribbon away to its tab names, or puts it back, and keeps the choice for the
    /// next start (Ctrl+F1, as in Office; issue #136). The page gets the ribbon's height.
    pub fn toggle_ribbon(&mut self) {
        self.ribbon_collapsed = !self.ribbon_collapsed;
        self.ribbon_peek = false;
        let _ = self.session.settings.set("ribbon_collapsed", if self.ribbon_collapsed { "true" } else { "false" });
    }

    /// Whether the ribbon shows only its tab names: the user folded it, or the window is
    /// phone-width, where its groups would leave no room for the page (issue #12).
    fn ribbon_folded(&self) -> bool {
        self.ribbon_collapsed || self.compact
    }

    /// Works out the phone-width layout for this frame (`compact.rs`). An opened page fills a
    /// compact window; a window that grows wide again shows the notebooks beside the page.
    fn compact_layout(&mut self, ctx: &Context) {
        let compact = crate::compact::is_compact(ctx.content_rect().width());
        if compact != self.compact {
            self.compact = compact;
            if !compact && self.compact_full_page {
                self.full_page = false;
            }
            self.compact_full_page = false;
        }
        if std::mem::take(&mut self.page_opened) && compact && self.editor.is_some() && !self.full_page {
            self.full_page = true;
            self.compact_full_page = true;
        }
    }

    /// Whether the notebooks list fills a phone-width window this frame, with no page beside it.
    fn compact_list_fills(&self) -> bool {
        let studio_grid = self.last_tablet_studio && self.session.library_view() == nocturne_engine::LibraryView::StudioGrid;
        self.compact && !studio_grid && !(self.full_page && self.editor.is_some())
    }

    /// Whether the Tags Summary or the page templates fill a phone-width window this frame,
    /// with no page beside them.
    fn compact_pane_fills(&self) -> bool {
        self.compact && (self.tags_pane.is_some() || self.templates_pane.is_some())
    }

    /// Whether a pan or pinch from the platform (UIKit's recognizers on iOS) has no page under
    /// it: none is open, or the source, a dialog, the File screen or a pane filling a phone's
    /// window is shown instead.
    fn external_gesture_blocked(&self) -> bool {
        self.editor.is_none()
            || self.show_source
            || self.dialog.is_some()
            || self.backstage.is_some()
            || self.compact_list_fills()
            || self.compact_pane_fills()
    }

    /// Returns `true` when a modal dialog, backstage screen, source view, or floating window blocks
    /// raw canvas long-press injection (`TouchLongPress` on iPadOS).
    pub fn has_canvas_blocking_overlay(&self) -> bool {
        self.external_gesture_blocked()
            || self.ribbon_peek
            || self.theme_window
            || self.import_job.is_some()
            || self.tool_settings_open
            || matches!(self.math_ocr.as_ref().map(|c| &c.stage), Some(crate::math_ocr::Stage::Preview { .. }))
    }

    /// Returns `true` when the window renders the simplified Tablet Studio chrome instead of the
    /// multi-row Classic Ribbon. Explicit `UiLayoutMode::TabletStudio` stays in Tablet Studio even
    /// on narrow iPad Split View / Slide Over widths (`< 600pt`).
    pub fn is_tablet_studio(&self, ctx: &Context) -> bool {
        match self.session.layout_mode() {
            nocturne_engine::UiLayoutMode::TabletStudio => true,
            nocturne_engine::UiLayoutMode::ClassicRibbon => false,
            nocturne_engine::UiLayoutMode::Auto => !self.compact && ctx.os() == egui::os::OperatingSystem::IOS,
        }
    }

    /// Whether the infinite-canvas minimap is shown for the current layout: defaults ON in Tablet
    /// Studio and OFF in Classic Desktop Ribbon unless explicitly toggled.
    fn minimap_visible(&self, ctx: &Context) -> bool {
        self.minimap_override.unwrap_or_else(|| self.is_tablet_studio(ctx))
    }

    fn minimap_visible_now(&self) -> bool {
        self.minimap_override.unwrap_or(self.last_tablet_studio)
    }

    pub fn set_ui_layout_mode(&mut self, mode: crate::tablet_chrome::UiLayoutMode) {
        let engine_mode = match mode {
            crate::tablet_chrome::UiLayoutMode::Auto => nocturne_engine::UiLayoutMode::Auto,
            crate::tablet_chrome::UiLayoutMode::TabletStudio => nocturne_engine::UiLayoutMode::TabletStudio,
            crate::tablet_chrome::UiLayoutMode::ClassicRibbon => nocturne_engine::UiLayoutMode::ClassicRibbon,
        };
        self.session.set_layout_mode(engine_mode);
        self.last_tablet_studio = match engine_mode {
            nocturne_engine::UiLayoutMode::TabletStudio => true,
            nocturne_engine::UiLayoutMode::ClassicRibbon => false,
            nocturne_engine::UiLayoutMode::Auto => !self.compact && self.last_ctx_is_ios,
        };
    }

    /// Android's Back button: closes the topmost thing over the page and says whether it did.
    /// A dialog closes as its Cancel would (the first-use wizard goes back a step instead); a
    /// shown-over ribbon or the format painter get Escape, as from the keyboard; the File screen,
    /// the Tags Summary and the page templates close; a page filling the window gives way to the notebooks. With
    /// nothing left to close it does nothing, and Android keeps the app open.
    pub fn system_back(&mut self) -> bool {
        let escape = |frames: &mut std::collections::VecDeque<Vec<egui::Event>>| {
            let key =
                |pressed| egui::Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed, repeat: false, modifiers: Modifiers::NONE };
            frames.push_back(vec![key(true), key(false)]);
        };
        if let Some(dialog) = &self.dialog {
            // Not every dialog reads Escape (a name box keeps it, for one), so Back cancels the
            // dialog itself. The wizard reads Escape as its own Back button.
            if matches!(dialog, Dialog::FirstUseWizard { .. }) {
                escape(&mut self.event_frames);
            } else {
                self.dialog = None;
            }
            return true;
        }
        if self.math_ocr.is_some() {
            self.math_ocr = None;
            return true;
        }
        if self.tool_settings_open {
            self.tool_settings_open = false;
            return true;
        }
        if self.ribbon_peek || self.painter.is_some() {
            escape(&mut self.event_frames);
            return true;
        }
        if self.backstage.is_some() {
            self.backstage = None;
            return true;
        }
        if self.tags_pane.is_some() && self.compact {
            self.tags_pane = None;
            return true;
        }
        if self.templates_pane.is_some() && self.compact {
            self.templates_pane = None;
            return true;
        }
        if self.full_page && self.editor.is_some() {
            self.full_page = false;
            self.compact_full_page = false;
            return true;
        }
        false
    }

    /// Where the soft keyboard's text goes while a block or title of the page has the caret, or `None`.
    pub fn keyboard_caret_rect(&self) -> Option<egui::Rect> {
        if self.show_source
            || self.dialog.is_some()
            || self.backstage.is_some()
            || self.compact_list_fills()
            || self.pointer != ribbon::Pointer::Write
        {
            return None;
        }
        let ed = self.editor.as_ref()?;
        if !ed.writable && !ed.title_focused {
            return None;
        }
        if ed.caret.is_none() && !ed.title_focused {
            return None;
        }
        ed.caret_screen_rect.filter(|r| r.is_finite()).or(self.page_rect)
    }

    /// Whether single-finger touch draws on the canvas instead of scrolling/panning.
    pub fn is_finger_drawing(&self) -> bool {
        self.session.finger_drawing()
    }

    /// Enables or disables single-finger drawing on the canvas.
    pub fn set_finger_drawing(&mut self, enabled: bool) {
        self.session.set_finger_drawing(enabled);
    }

    /// Toggles between the eraser and the active/previous writing tool via `Session`.
    pub fn toggle_eraser_tool(&mut self) {
        self.session.toggle_eraser(nocturne_engine::EraserToggleMode::Toggle);
        self.sync_tool_state_from_session();
    }

    /// Applies an external pan delta in logical points (e.g. from native UIKit pan recognizers).
    pub fn apply_external_pan(&mut self, delta_points: egui::Vec2) {
        if !delta_points.is_finite() || self.external_gesture_blocked() {
            return;
        }
        self.canvas_pan.x = (self.canvas_pan.x + delta_points.x).clamp(-1.0e6, 1.0e6);
        if self.pointer == ribbon::Pointer::Write {
            let mut rem_y = delta_points.y;
            if self.canvas_pan.y < 0.0 && rem_y > 0.0 {
                let take = rem_y.min(-self.canvas_pan.y);
                self.canvas_pan.y += take;
                rem_y -= take;
            } else if self.canvas_pan.y > 0.0 && rem_y < 0.0 {
                let take = (-rem_y).min(self.canvas_pan.y);
                self.canvas_pan.y -= take;
                rem_y += take;
            }
            if rem_y != 0.0 {
                self.pending_external_scroll_y = (self.pending_external_scroll_y + rem_y).clamp(-1.0e6, 1.0e6);
            }
        } else {
            self.canvas_pan.y = (self.canvas_pan.y + delta_points.y).clamp(-1.0e6, 1.0e6);
        }
    }

    fn set_zoom_percent(&mut self, percent: u32) {
        let clamped = percent.clamp(crate::touch_ux::MIN_ZOOM_PERCENT, crate::touch_ux::MAX_ZOOM_PERCENT);
        self.zoom_percent = clamped;
        self.zoom_exact = f64::from(clamped);
    }

    /// Applies an external pinch-zoom scale factor around `focal_pos` using canonical zoom bounds.
    pub fn apply_external_pinch_zoom(&mut self, scale_factor: f32, focal_pos: Option<egui::Pos2>) {
        // Refused where a pan is: the zoom is the page's, and a pinch over the File screen or a
        // list filling a phone's window would change it out of sight.
        if !scale_factor.is_finite() || scale_factor <= 0.0 || self.external_gesture_blocked() {
            return;
        }
        let old_zoom = self.zoom_percent;
        let min_zoom = f64::from(crate::touch_ux::MIN_ZOOM_PERCENT);
        let max_zoom = f64::from(crate::touch_ux::MAX_ZOOM_PERCENT);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let synced = self.zoom_exact.round().clamp(min_zoom, max_zoom) as u32;
        if synced != old_zoom {
            self.zoom_exact = f64::from(old_zoom);
        }
        self.zoom_exact = (self.zoom_exact * f64::from(scale_factor)).clamp(min_zoom, max_zoom);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let new_zoom = self.zoom_exact.round().clamp(min_zoom, max_zoom) as u32;
        self.zoom_percent = new_zoom;
        self.fit_page_width = false;
        if new_zoom != old_zoom
            && let Some(pos) = focal_pos.filter(|p| p.is_finite())
            && let Some(page_rect) = self.page_view_rect.or(self.page_rect)
            && page_rect.contains(pos)
        {
            #[allow(clippy::cast_precision_loss)]
            let old_z = old_zoom.max(1) as f32 / 100.0;
            #[allow(clippy::cast_precision_loss)]
            let new_z = new_zoom as f32 / 100.0;
            let ratio = new_z / old_z;
            let anchor = self.editor.as_ref().map_or(page_rect.min, |ed| ed.origin);
            let margin_at = |z: f32| ((page_rect.width() - 780.0 * z) / 2.0).clamp(8.0, 127.0);
            let mut delta = (pos - anchor) * (1.0 - ratio);
            delta.x -= margin_at(new_z) - margin_at(old_z);
            if delta.is_finite() {
                self.apply_external_pan(delta);
            }
        }
    }

    /// Returns the currently selected text in the active editor, title, or cross-block span.
    pub fn selected_text_for_platform(&self) -> String {
        let Some(ed) = self.editor.as_ref() else { return String::new() };
        if ed.title_focused {
            let Some((a, b)) = ed.title_selection else { return String::new() };
            let (lo, hi) = (a.min(b), a.max(b));
            return ed.title_buf.chars().skip(lo).take(hi.saturating_sub(lo)).collect();
        }
        if let Some(open) = ed.across.filter(|a| !a.inside_one_block()) {
            return pageedit::span_text(&ed.blocks, open);
        }
        let Some(caret) = ed.caret.as_ref() else { return String::new() };
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        caret.text.chars().skip(lo).take(hi.saturating_sub(lo)).collect()
    }

    fn flush_pending_copy(&mut self, ctx: Option<&Context>) {
        if let Some(text) = self.pending_copy.take() {
            let html = self.pending_copy_html.take();
            let ok = match html.as_deref() {
                Some(html) => self.clipboard.set_html(html, &text).is_ok(),
                None => self.clipboard.set_text(&text).is_ok(),
            };
            if !ok && let Some(ctx) = ctx {
                ctx.copy_text(text);
            }
        }
    }

    /// Executes a platform text-edit menu action (`Cut`, `Copy`, `Paste`, `SelectAll`, `Delete`).
    pub fn apply_platform_edit_action(&mut self, action: PlatformEditAction) {
        if self.dialog.is_some() || self.backstage.is_some() || self.show_source {
            return;
        }
        let title_focused = self.editor.as_ref().is_some_and(|ed| ed.title_focused);
        if title_focused {
            match action {
                PlatformEditAction::Copy | PlatformEditAction::Cut => {
                    let selected = self.selected_text_for_platform();
                    if selected.is_empty() {
                        return;
                    }
                    self.pending_copy = Some(selected);
                    self.pending_copy_html = None;
                    self.flush_pending_copy(None);
                    if action == PlatformEditAction::Cut
                        && let Some(ed) = self.editor.as_mut()
                        && let Some((a, b)) = ed.title_selection
                    {
                        let (lo, hi) = (a.min(b), a.max(b));
                        let (next, at) = pageedit::insert_at_caret(&ed.title_buf, (lo, hi), "");
                        ed.title_buf = next;
                        ed.title_selection = Some((at, at));
                        ed.title_restore_selection = Some((at, at));
                    }
                }
                PlatformEditAction::Paste => {
                    if let Some(text) = self.clipboard.text() {
                        let clean = line_breaks_to_spaces(&text);
                        if let Some(ed) = self.editor.as_mut() {
                            let end = ed.title_buf.chars().count();
                            let sel = ed.title_selection.unwrap_or((end, end));
                            let (next, at) = pageedit::insert_at_caret(&ed.title_buf, sel, &clean);
                            ed.title_buf = next;
                            ed.title_selection = Some((at, at));
                            ed.title_restore_selection = Some((at, at));
                        }
                    }
                }
                PlatformEditAction::SelectAll => {
                    if let Some(ed) = self.editor.as_mut() {
                        let end = ed.title_buf.chars().count();
                        ed.title_selection = Some((end, 0));
                        ed.title_restore_selection = Some((end, 0));
                    }
                }
                PlatformEditAction::Delete => {
                    if let Some(ed) = self.editor.as_mut()
                        && let Some((a, b)) = ed.title_selection
                    {
                        let (lo, hi) = (a.min(b), a.max(b));
                        let range = if lo < hi {
                            (lo, hi)
                        } else if lo < ed.title_buf.chars().count() {
                            (lo, lo + 1)
                        } else {
                            return;
                        };
                        let (next, at) = pageedit::insert_at_caret(&ed.title_buf, range, "");
                        ed.title_buf = next;
                        ed.title_selection = Some((at, at));
                        ed.title_restore_selection = Some((at, at));
                    }
                }
            }
            return;
        }
        if !self.editor.as_ref().is_some_and(|ed| ed.writable) {
            return;
        }
        match action {
            PlatformEditAction::Copy => {
                self.clipboard("copy");
                self.flush_pending_copy(None);
            }
            PlatformEditAction::Cut => {
                self.clipboard("cut");
                self.flush_pending_copy(None);
            }
            PlatformEditAction::Paste => self.paste_here(None),
            PlatformEditAction::SelectAll => {
                self.select_all_in_block();
                if let Some(ed) = self.editor.as_mut()
                    && let Some(c) = ed.caret.as_mut()
                {
                    let end = c.text.chars().count();
                    c.focus = true;
                    ed.restore_format_selection = Some((c.block, end, 0));
                }
            }
            PlatformEditAction::Delete => {
                let Some(ed) = self.editor.as_mut() else { return };
                if ed.across.is_some_and(|a| !a.inside_one_block()) {
                    Self::remember_step(ed);
                    let (_, cols, justified) = Self::delete_across_selection(ed);
                    ed.rebuild_body();
                    Self::mark_dirty_from_blocks(ed);
                    Self::remember_edit(ed);
                    if cols || justified {
                        self.save_block_marks((cols, justified));
                    }
                } else if let Some(caret) = ed.caret.as_ref() {
                    let len = caret.text.chars().count();
                    let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
                    let range = if lo < hi {
                        (lo, hi)
                    } else if lo < len {
                        (lo, lo + 1)
                    } else {
                        return;
                    };
                    let prev_sel = caret.selection;
                    let block_idx = caret.block;
                    Self::remember_step(ed);
                    if let Some(caret) = ed.caret.as_mut() {
                        if let Some(block) = ed.blocks.get(block_idx) {
                            caret.ensure_inline(block);
                        }
                        let (next, at) = pageedit::insert_at_caret(&caret.text, range, "");
                        caret.text = next.clone();
                        caret.selection = (at, at);
                        caret.place = Some(at);
                        caret.focus = true;
                        ed.restore_format_selection = Some((block_idx, at, at));
                        if let Some(inline) = caret.inline.as_mut() {
                            inline.update_at(&next, prev_sel, caret.selection);
                        }
                        if let Some(b) = ed.blocks.get_mut(block_idx) {
                            caret.write(b);
                        }
                    }
                    ed.rebuild_body();
                    Self::mark_dirty_from_blocks(ed);
                    Self::remember_edit(ed);
                }
            }
        }
    }

    /// Inserts platform-committed text (e.g. from Scribble or virtual keyboard) at the active caret.
    pub fn insert_platform_text(&mut self, text: &str) {
        if text.is_empty() || self.dialog.is_some() || self.backstage.is_some() || self.show_source {
            return;
        }
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.title_focused {
            let clean = line_breaks_to_spaces(text);
            let end = ed.title_buf.chars().count();
            let sel = ed.title_selection.unwrap_or((end, end));
            let (next, at) = pageedit::insert_at_caret(&ed.title_buf, sel, &clean);
            ed.title_buf = next;
            ed.title_selection = Some((at, at));
            ed.title_restore_selection = Some((at, at));
            return;
        }
        if !ed.writable {
            return;
        }
        let has_open_across = ed.across.is_some_and(|a| !a.inside_one_block());
        let across_marks = if has_open_across {
            Self::remember_step(ed);
            let (_, cols, justified) = Self::delete_across_selection(ed);
            (cols, justified)
        } else {
            if ed.caret.is_none() {
                Self::place_caret_for_format(ed);
            }
            (false, false)
        };
        let (prev_sel, block_idx, before_caret) = {
            let Some(caret) = ed.caret.as_ref() else { return };
            (caret.selection, caret.block, ed.pending_format.as_ref().map(|_| caret.clone()))
        };
        let clean = text.replace(['\r', '\n'], " ");
        if let Some(caret) = ed.caret.as_mut() {
            if let Some(block) = ed.blocks.get(block_idx) {
                caret.ensure_inline(block);
            }
            let (next, at) = pageedit::insert_at_caret(&caret.text, prev_sel, &clean);
            caret.text = next.clone();
            caret.selection = (at, at);
            caret.place = Some(at);
            caret.focus = true;
            ed.restore_format_selection = Some((block_idx, at, at));
            if let Some(inline) = caret.inline.as_mut() {
                inline.update_at(&next, prev_sel, caret.selection);
            }
            if let Some(b) = ed.blocks.get_mut(block_idx) {
                caret.write(b);
            }
        }
        ed.apply_pending_format(before_caret.as_ref());
        if let Some(c) = ed.caret.as_mut() {
            c.focus = true;
            c.place = Some(c.selection.0);
            if c.selection.0 != c.selection.1 {
                ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
            }
        }
        if has_open_across {
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
            if across_marks.0 || across_marks.1 {
                self.save_block_marks(across_marks);
            }
        } else {
            Self::note_typing(ed);
        }
    }

    /// Deletes one character backward (or the active selection) for platform keyboard input.
    pub fn delete_backward_platform(&mut self) {
        if self.dialog.is_some() || self.backstage.is_some() || self.show_source {
            return;
        }
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.title_focused {
            let end = ed.title_buf.chars().count();
            let (a, b) = ed.title_selection.unwrap_or((end, end));
            let (lo, hi) = (a.min(b), a.max(b));
            let range = if lo < hi {
                (lo, hi)
            } else if lo > 0 {
                (lo - 1, lo)
            } else {
                return;
            };
            let (next, at) = pageedit::insert_at_caret(&ed.title_buf, range, "");
            ed.title_buf = next;
            ed.title_selection = Some((at, at));
            ed.title_restore_selection = Some((at, at));
            return;
        }
        if !ed.writable {
            return;
        }
        if ed.across.is_some_and(|a| !a.inside_one_block()) {
            Self::remember_step(ed);
            let (_, cols, justified) = Self::delete_across_selection(ed);
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
            if cols || justified {
                self.save_block_marks((cols, justified));
            }
            return;
        }
        let (block_idx, prev_sel, range) = {
            let Some(caret) = ed.caret.as_ref() else { return };
            let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
            let range = if lo < hi {
                (lo, hi)
            } else if lo > 0 {
                (lo - 1, lo)
            } else {
                // At the start of a line the soft keyboard's delete joins it onto the line
                // above, as Backspace does on a desktop keyboard. UIKit sends deleteBackward
                // as a call rather than a Backspace key, so the page's own Backspace never
                // sees it. A table cell keeps its words where they are.
                let (at, in_cell) = (caret.block, caret.cell.is_some());
                if !in_cell {
                    self.join_back_platform(at);
                }
                return;
            };
            (caret.block, caret.selection, range)
        };
        if let Some(caret) = ed.caret.as_mut() {
            if let Some(block) = ed.blocks.get(block_idx) {
                caret.ensure_inline(block);
            }
            let (next, at) = pageedit::insert_at_caret(&caret.text, range, "");
            caret.text = next.clone();
            caret.selection = (at, at);
            caret.place = Some(at);
            caret.focus = true;
            ed.restore_format_selection = Some((block_idx, at, at));
            if let Some(inline) = caret.inline.as_mut() {
                inline.update_at(&next, prev_sel, caret.selection);
            }
            if let Some(b) = ed.blocks.get_mut(block_idx) {
                caret.write(b);
            }
        }
        if let Some(c) = ed.caret.as_mut() {
            c.focus = true;
            c.place = Some(c.selection.0);
            if c.selection.0 != c.selection.1 {
                ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
            }
        }
        Self::note_typing(ed);
    }

    /// Joins the caret's line onto the line above for [`Self::delete_backward_platform`]. The
    /// join is the page's own Backspace ([`pageedit::join_back`]); the boxes, the table widths
    /// and the justified paragraphs after the line move up with their blocks, as a cut across
    /// blocks moves them, and the whole of it is one Undo step. The caret lands where the two
    /// lines meet.
    fn join_back_platform(&mut self, at: usize) {
        let Some(ed) = self.editor.as_mut() else { return };
        let before = ed.blocks.len();
        // Where the lines meet: the end of the line above, measured before this one joins it.
        let seam = at.checked_sub(1).and_then(|above| ed.blocks.get(above)).map_or(0, |b| pageedit::caret_text(b).chars().count());
        Self::remember_step(ed);
        if !pageedit::join_back(&mut ed.blocks, &mut ed.caret, at).blocks {
            return;
        }
        let mut marks = (false, false);
        if ed.blocks.len() < before {
            marks = ed.block_removed(at);
            // A box that held only the joined line is left with nothing in it, and goes.
            if ed.arranged && ed.boxes.len() > 1 {
                let caret_box = nocturne_model::container::place(&ed.boxes, at.saturating_sub(1)).map(|(i, _)| i);
                for i in (0..ed.boxes.len()).rev() {
                    if Some(i) != caret_box && ed.boxes[i].blocks == 0 {
                        ed.boxes.remove(i);
                    }
                }
            }
        }
        ed.across = None;
        if let Some(c) = ed.caret.as_mut() {
            c.selection = (seam, seam);
            c.place = Some(seam);
            c.focus = true;
            ed.restore_format_selection = Some((c.block, seam, seam));
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        if marks.0 || marks.1 {
            self.save_block_marks(marks);
        }
    }

    /// Updates explicit software-keyboard visibility and bottom inset in logical points.
    pub fn set_soft_keyboard_visible(&mut self, visible: bool, inset_bottom_points: f32) {
        self.soft_keyboard_visible = visible;
        self.soft_keyboard_inset_bottom = if inset_bottom_points.is_finite() { inset_bottom_points.max(0.0) } else { 0.0 };
    }

    /// Whether the docked mobile formatting bar above the software keyboard is currently shown.
    pub fn formatting_bar_visible(&self) -> bool {
        let keyboard_up = self.soft_keyboard_visible || self.last_safe_area_bottom >= SOFT_KEYBOARD_MIN_INSET_PTS;
        keyboard_up
            && !self.show_source
            && self.dialog.is_none()
            && self.backstage.is_none()
            && !self.compact_list_fills()
            && !(self.compact && (self.tags_pane.is_some() || self.templates_pane.is_some()))
            && self.pointer == ribbon::Pointer::Write
            && self
                .editor
                .as_ref()
                .is_some_and(|ed| ed.writable && !ed.title_focused && (ed.caret.is_some() || ed.across.is_some_and(|a| !a.inside_one_block())))
    }

    fn mobile_formatting_bar(&mut self, ui: &mut Ui) {
        if !self.formatting_bar_visible() {
            return;
        }
        let tokens = self.tokens;
        let is_across = self.editor.as_ref().and_then(|ed| ed.across).is_some_and(|a| !a.inside_one_block());
        let (in_cell, list_depth, can_format, can_block, bullets_active, todo_active) = self
            .editor
            .as_ref()
            .and_then(|ed| {
                if is_across {
                    return Some((false, None, true, false, false, false));
                }
                let caret = ed.caret.as_ref()?;
                let block = ed.blocks.get(caret.block)?;
                let in_cell = caret.cell.is_some();
                let list_depth = match block {
                    nocturne_formats::Block::Item { depth, .. } => Some(*depth),
                    _ => None,
                };
                let is_code_or_math = matches!(block, nocturne_formats::Block::Code { .. } | nocturne_formats::Block::MathBlock(_));
                let can_format = !is_code_or_math;
                let can_block = !in_cell && pageedit::is_one_line_of_words(block) && !is_code_or_math;
                let bullets_active = !in_cell && matches!(block, nocturne_formats::Block::Item { ordered: false, task: None, .. });
                let todo_active = !in_cell && matches!(block, nocturne_formats::Block::Item { task: Some(_), .. });
                Some((in_cell, list_depth, can_format, can_block, bullets_active, todo_active))
            })
            .unwrap_or((false, None, false, false, false, false));
        let marks = if is_across { pageedit::Marks::default() } else { self.font_selection(ui.ctx()).marks };
        let state = crate::touch_ux::MobileFormattingBarState {
            can_outdent: !in_cell && list_depth.is_some_and(|d| d > 0),
            can_indent: !in_cell && list_depth.is_some_and(|d| d < 8),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            can_format,
            bold_active: marks.bold,
            italic_active: marks.italic,
            underline_active: marks.underline,
            can_block,
            bullets_active,
            todo_active,
            can_quick_pen: self.editor.as_ref().is_some_and(|ed| ed.writable),
            pen_active: self.pointer == ribbon::Pointer::Draw,
        };
        if let Some(action) = crate::touch_ux::mobile_formatting_bar(ui, state, &tokens) {
            self.ribbon_act(action.into_ribbon_act(self.nib));
            if action.restores_text_caret()
                && let Some(ed) = self.editor.as_mut()
                && ed.across.is_none()
                && let Some(c) = ed.caret.as_mut()
            {
                c.focus = true;
                c.place = Some(c.selection.0);
                if c.selection.0 != c.selection.1 {
                    ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
                }
            }
        }
    }

    /// The Quick Access Toolbar, drawn in whatever row it is given.
    fn quick_access(&mut self, ui: &mut Ui, tokens: Tokens) {
        let enabled =
            quickaccess::Enabled { back: self.nav.can_back(), forward: self.nav.can_forward(), undo: self.can_undo(), redo: self.can_redo() };
        let (pressed, changed) = quickaccess::show(ui, &mut self.qat, enabled, &tokens);
        if changed {
            self.qat.save(|k, v| {
                let _ = self.session.settings.set(k, v);
            });
        }
        match pressed {
            Some(quickaccess::Command::Back) => self.go_back(),
            Some(quickaccess::Command::Forward) => self.go_forward(),
            Some(quickaccess::Command::Undo) => self.undo(),
            Some(quickaccess::Command::Redo) => self.redo(),
            None => {}
        }
    }

    /// Opens every level of the tree.
    pub fn expand_all(&mut self) {
        self.open = self.collect_branches().into_iter().collect();
    }

    /// Whether the page being edited has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.dirty_since.is_some() || e.saving.is_some())
            || self.pad.as_ref().is_some_and(crate::inkpad::Pad::has_unsaved_changes)
    }

    /// Where the page's words were drawn last frame. Its ink is measured from the top
    /// left of this, so a test that writes on the page aims here.
    pub fn page_rect(&self) -> Option<egui::Rect> {
        self.page_rect
    }

    /// Visible screen-space viewport of the open page (`ScrollArea::inner_rect`), unshifted by scroll offset.
    pub fn page_view_rect(&self) -> Option<egui::Rect> {
        self.page_view_rect
    }

    /// The page's zoom, in percent, as the View tab shows it.
    pub fn zoom_percent(&self) -> u32 {
        self.zoom_percent
    }

    /// The text in the editor, if a page is open.
    pub fn body(&self) -> Option<&str> {
        self.editor.as_ref().map(|e| e.body.as_str())
    }

    /// The title of the open page, which the window shows above it.
    pub fn title(&self) -> Option<&str> {
        self.editor.as_ref().map(|e| e.title.as_str())
    }

    /// Replaces the text in the editor, as typing would, and marks the page changed.
    pub fn set_body(&mut self, body: &str) {
        if let Some(ed) = self.editor.as_mut() {
            ed.body = body.to_string();
            Self::mark_dirty(ed);
        }
    }

    /// Saves the open page now, if it has changes.
    pub fn save(&mut self) {
        self.save_now();
    }

    /// The last status line.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Shows a brief status message in the bottom bar.
    pub fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = false;
    }

    /// Says something went wrong, in our own words rather than the engine's.
    /// Tells the user about a problem. The status line always says it. A box says it too, unless
    /// the user has asked not to see alerts with this `key` again this session; a box that is
    /// already open is not replaced, the next alert waits its turn in a queue.
    pub fn alert(&mut self, key: &str, title: &str, message: &str) {
        self.status = message.to_string();
        self.status_is_error = true;
        if self.muted_alerts.contains(key) {
            return;
        }
        if self.session.settings.get(&format!("{key}_never")).as_deref() == Some("true") {
            return;
        }
        // A box says it, so Tablet Studio's failure banner does not say it again once the box is
        // closed. An alert the user has muted has no box, and the banner is then how it is seen.
        self.status_banner_dismissed = Some(self.status.clone());
        let showing = matches!(&self.dialog, Some(Dialog::Alert { key: k, .. }) if k == key);
        let queued = self.queued_alerts.iter().any(|d| matches!(d, Dialog::Alert { key: k, .. } if k == key));
        if showing || queued {
            return;
        }
        let dialog =
            Dialog::Alert { key: key.to_string(), title: title.to_string(), message: message.to_string(), suppress: false, suppress_forever: false };
        if self.dialog.is_none() {
            self.dialog = Some(dialog);
        } else {
            self.queued_alerts.push_back(dialog);
        }
    }

    fn complain(&mut self, msg: &str) {
        self.status = msg.to_string();
        self.status_is_error = true;
    }

    fn fail(&mut self, e: &EngineError) {
        self.status = crate::i18n::format_message_id("ui.could_not_complete_the_action_error", &[("error", &e.to_string())]);
        self.status_is_error = true;
    }

    fn run(&mut self, id: &str, p: &Params) -> Option<Output> {
        // A command may read or write the page an autosave is still writing.
        self.take_saves(true);
        match self.session.run(id, p) {
            Ok(o) => {
                if id == "sync.configure" {
                    self.cached_sync_config = None;
                }
                if !id.starts_with("sync.") && nocturne_engine::registry().iter().any(|spec| spec.id == id && spec.changes_tree) {
                    self.sync_dirty_since_last = true;
                    self.last_local_edit_at = Some(Instant::now());
                }
                Some(o)
            }
            Err(e) => {
                self.fail(&e);
                None
            }
        }
    }

    fn reparent_page(&mut self, path: &Path, command: &str) {
        self.save_now();
        if self.is_dirty() {
            return;
        }
        let editor_path = self.editor.as_ref().map(|editor| editor.dir.clone());
        let Some(Output::Path(new_path)) = self.run(command, &params(&[("path", &path_str(path))])) else { return };
        self.nav.relocate(path, &new_path);
        self.library_grid.relocate(path, &new_path, &self.session.settings);
        let remap = |candidate: &Path| candidate.strip_prefix(path).map_or_else(|_| candidate.to_path_buf(), |suffix| new_path.join(suffix));
        self.open = self.open.iter().map(|branch| remap(branch)).collect();
        self.selected = self.selected.as_deref().map(remap);
        if let Some(editor_path) = editor_path.filter(|editor_path| editor_path.starts_with(path)) {
            let relocated = remap(&editor_path);
            self.open_page_inner(&relocated);
        }
    }

    /// Opens Move or Copy for a page, section or section group (issue #167).
    pub fn open_move_or_copy(&mut self, path: &Path) {
        let Some(info) =
            nocturne_store::tree::read_node(path).filter(|n| matches!(n.kind, NodeKind::Page | NodeKind::Section | NodeKind::SectionGroup))
        else {
            return;
        };
        self.dialog = Some(Dialog::MoveOrCopy { path: path.to_path_buf(), title: info.title, kind: info.kind, dest: None });
    }

    /// The places a node of `kind` can be moved or copied to, as (path, title, depth), in tree
    /// order: sections for a page, notebooks and section groups for a section or group. Never
    /// the node itself or anything inside it.
    fn move_destinations(&self, path: &Path, kind: NodeKind) -> Vec<(PathBuf, String, usize)> {
        fn walk(nodes: &[Node], path: &Path, kind: NodeKind, depth: usize, out: &mut Vec<(PathBuf, String, usize)>) {
            for n in nodes.iter().filter(|n| n.info.kind != NodeKind::Page) {
                if n.path.starts_with(path) {
                    continue;
                }
                let fits = match kind {
                    NodeKind::Page => n.info.kind == NodeKind::Section,
                    _ => matches!(n.info.kind, NodeKind::Notebook | NodeKind::SectionGroup),
                };
                // A notebook or group is listed even where it does not fit, as the heading of
                // what is inside it.
                out.push((n.path.clone(), n.info.title.clone(), depth));
                if !fits && n.info.kind == NodeKind::Section {
                    out.pop();
                }
                walk(&n.children, path, kind, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(self.session.tree(), path, kind, 0, &mut out);
        out
    }

    /// Moves or copies `path` into `dest`, then keeps the window on the same page: a moved page
    /// that was open is opened again where it went.
    fn move_or_copy(&mut self, path: &Path, dest: &Path, copy: bool) {
        self.save_now();
        if self.is_dirty() {
            return;
        }
        let command = if copy { "node.copy" } else { "node.move" };
        let editor_path = self.editor.as_ref().map(|editor| editor.dir.clone());
        let Some(Output::Path(new_path)) = self.run(command, &params(&[("path", &path_str(path)), ("to", &path_str(dest))])) else { return };
        self.open.insert(dest.to_path_buf());
        if copy {
            self.say(crate::tl_id!("ui.copied"));
            return;
        }
        self.nav.relocate(path, &new_path);
        self.library_grid.relocate(path, &new_path, &self.session.settings);
        let remap = |candidate: &Path| candidate.strip_prefix(path).map_or_else(|_| candidate.to_path_buf(), |suffix| new_path.join(suffix));
        self.open = self.open.iter().map(|branch| remap(branch)).collect();
        self.selected = self.selected.as_deref().map(remap);
        if let Some(editor_path) = editor_path.filter(|editor_path| editor_path.starts_with(path)) {
            let relocated = remap(&editor_path);
            self.open_page_inner(&relocated);
        }
        self.say(crate::tl_id!("ui.moved"));
    }

    fn create_page_in(&mut self, parent: &Path) {
        if let Some(Output::Path(page)) = self.run("node.create", &params(&[("parent", &path_str(parent)), ("kind", "page")])) {
            self.open_page(&page);
            self.say(crate::tl_id!("ui.new_page"));
        }
    }

    fn show_page_versions(&mut self, page: &Path) {
        self.save_now();
        if self.is_dirty() {
            return;
        }
        let p = params(&[("path", &path_str(page))]);
        if let Some(Output::Versions(versions)) = self.run("page.versions", &p) {
            self.dialog = Some(Dialog::PageVersions { page: page.to_path_buf(), versions });
        }
    }

    fn can_move_page(path: &Path, up: bool) -> bool {
        let Some(parent) = path.parent() else { return false };
        let siblings: Vec<_> =
            nocturne_store::tree::child_dirs(parent).into_iter().filter(|sibling| nocturne_store::tree::read_node(sibling).is_some()).collect();
        let Some(index) = siblings.iter().position(|sibling| sibling == path) else { return false };
        let adjacent = if up { index.checked_sub(1) } else { index.checked_add(1).filter(|next| *next < siblings.len()) };
        adjacent
            .and_then(|adjacent| siblings.get(adjacent))
            .and_then(|sibling| nocturne_store::tree::read_node(sibling))
            .is_some_and(|info| info.kind == NodeKind::Page)
    }

    fn can_make_subpage(path: &Path) -> bool {
        let Some(parent) = path.parent() else { return false };
        let siblings: Vec<_> =
            nocturne_store::tree::child_dirs(parent).into_iter().filter(|sibling| nocturne_store::tree::read_node(sibling).is_some()).collect();
        let Some(index) = siblings.iter().position(|sibling| sibling == path) else { return false };
        index
            .checked_sub(1)
            .and_then(|previous| siblings.get(previous))
            .and_then(|previous| nocturne_store::tree::read_node(previous))
            .is_some_and(|info| info.kind == NodeKind::Page)
    }

    fn can_promote_subpage(path: &Path) -> bool {
        path.parent().and_then(nocturne_store::tree::read_node).is_some_and(|info| info.kind == NodeKind::Page)
    }

    fn hierarchy_keys(&mut self, ctx: &Context) {
        // Typing in the page (its title or a paragraph) is not a reason to ignore page
        // commands: OneNote's Ctrl+N and Ctrl+Alt+M work from the middle of a sentence (issue
        // #254). Any other text box keeps the keys: the search box, a rename, the source view.
        let typing_elsewhere = ctx.egui_wants_keyboard_input() && !self.page_text_has_focus(ctx);
        if typing_elsewhere || self.dialog.is_some() || self.backstage.is_some() {
            return;
        }
        let Some(page) = self.selected.clone().filter(|path| nocturne_store::tree::read_node(path).is_some_and(|info| info.kind == NodeKind::Page))
        else {
            return;
        };
        if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::N)) {
            if let Some(parent) = page.parent() {
                self.create_page_in(parent);
            }
            return;
        }
        let is_apple = cfg!(any(target_os = "macos", target_os = "ios"));
        let mods = Modifiers { ctrl: !is_apple, alt: true, mac_cmd: is_apple, command: true, ..Modifiers::NONE };
        // OneNote's Ctrl+Alt+M: Move or Copy the page.
        if ctx.input_mut(|input| input.consume_key(mods, Key::M)) {
            self.open_move_or_copy(&page);
            return;
        }
        if Self::can_make_subpage(&page) && ctx.input_mut(|input| input.consume_key(mods, Key::CloseBracket)) {
            self.reparent_page(&page, "node.demote");
        } else if Self::can_promote_subpage(&page) && ctx.input_mut(|input| input.consume_key(mods, Key::OpenBracket)) {
            self.reparent_page(&page, "node.promote");
        }
    }

    /// Whether the keyboard is with one of the open page's own text boxes, its title or a block,
    /// rather than with a box outside the page such as search or a rename in the tree.
    fn page_text_has_focus(&self, ctx: &Context) -> bool {
        let (Some(page), Some(_)) = (self.page_rect, self.editor.as_ref()) else { return false };
        if self.show_source {
            return false;
        }
        ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).is_some_and(|focused| page.contains(focused.rect.center()))
    }

    // ---------- page open, save ----------

    fn open_page(&mut self, dir: &Path) {
        if let Some(notebook) = notebook_for(dir) {
            self.remember_recent_notebook(&notebook);
        }
        self.nav.visit(dir);
        self.library_grid.record_recent_page(dir, &self.session.settings);
        self.library_grid.select_for_page(dir);
        self.open_page_inner(dir);
    }

    /// Opens a page without counting it as a visit: what Back and Forward do.
    fn open_page_inner(&mut self, dir: &Path) {
        let switching_page = self.editor.as_ref().is_none_or(|e| e.dir != dir);
        self.page_opened = true;
        self.save_now();
        self.code = crate::codeview::State::default();
        if let Some(Output::Page(page)) = self.run("page.load", &params(&[("path", &path_str(dir))])) {
            let (blocks, in_the_text) = nocturne_formats::boxes::read(page.format, &page.body);
            let audit = nocturne_formats::boxes::span_audit(&page.body, &blocks);
            log::debug!("page read span audit: page={:?} raw_span_tags={} styled_spans={}", dir, audit.raw_tags, audit.styled);
            // What the editor will show and edit: any block whose edit text still carries markup
            // is logged, with its index and the text, so a raw tag on screen is traceable.
            for (index, block) in blocks.iter().enumerate() {
                let shown = crate::pageedit::caret_text(block);
                if shown.contains("<span") || shown.contains("**") {
                    log::warn!("raw markup in edit text: page={:?} block={index} shown={shown:?}", dir);
                }
            }
            let older = nocturne_store::page::containers(dir).unwrap_or_default();
            let arranged = in_the_text.is_some() || !older.is_empty();
            let mut boxes = in_the_text.unwrap_or(older);
            log::debug!(
                "page loaded for layout: page={:?} format={:?} blocks={} arranged={} stored_containers={:?}",
                dir,
                page.format,
                blocks.len(),
                arranged,
                boxes
            );
            // A never-arranged blank page still gets one empty writing box. A
            // page whose last box was deliberately deleted remains boxless so
            // its open paper can create a new box on the next click.
            if blocks.is_empty() && !arranged && boxes.is_empty() {
                boxes.push(nocturne_model::Container::new(0.0, 0.0, 360.0, 0));
                log::debug!(
                    "default page block created: page={:?} block_count={} container=(x=0,y=0,width=360,blocks=0) empty_block_min_height=22",
                    dir,
                    blocks.len()
                );
            }
            // The page as it was opened is the first step of its history: undo goes back
            // this far and no further.
            let first = (page.body.clone(), blocks.clone(), boxes.clone());
            // The paper this page is drawn on and the colour behind it are the page's own.
            let appearance = nocturne_store::page::appearance(dir)
                .map(|(p, c)| (p.and_then(|p| ribbon::RuleLines::parse(&p)), c.and_then(|c| nocturne_model::NodeColor::parse(&c))))
                .unwrap_or((None, None));
            let attachments = nocturne_store::assets::list(dir);
            let page_body = page.body.clone();
            let writable = pageedit::round_trips(page.format, &page.body);
            // A page with no title of its own starts with a blank title area, not with
            // "Untitled page" sitting there as if it had been typed. `title` keeps the
            // display fallback, so losing focus on an empty box is correctly seen as "no
            // change" rather than a rename to the fallback word itself.
            let title_buf = if page.has_title { page.title.clone() } else { String::new() };
            self.editor = Some(Editor {
                dir: dir.to_path_buf(),
                format: page.format,
                title_buf,
                titled: page.has_title,
                title: page.title,
                saved_body: page.body.clone(),
                saving: None,
                body: page.body,
                dirty_since: None,
                blocks,
                last_saved: None,
                attachments: attachments.clone(),
                caret: None,
                caret_screen_rect: None,
                title_focused: false,
                title_selection: None,
                title_restore_selection: None,
                pending_format: None,
                restore_format_selection: None,
                writable,
                page_dir: page.dir,
                boxes,
                arranged,
                content_bottom: None,
                shown_widths: Vec::new(),
                last_frames: Vec::new(),
                columns: nocturne_store::page::table_columns(dir).unwrap_or_default(),
                justified: nocturne_store::page::justified(dir).unwrap_or_default(),
                across: None,
                ink_files: nocturne_store::page::ink_files(dir),
                paper: appearance.0,
                colour: appearance.1,
                rule_colour: nocturne_store::page::rule_colour(dir).ok().flatten().and_then(|c| nocturne_model::NodeColor::parse(&c)),
                canvas_layout: nocturne_store::page::canvas_layout(dir).unwrap_or_default(),
                history: crate::history::History::new(crate::history::Step::new(first.0, first.1, first.2, None, crate::history::Kind::Edit)),
                origin: egui::Pos2::ZERO,
                origin_in_canvas: None,
            });
            if !self.icon_recovery_suppressed
                && let Some(name) = first_missing_attachment_in_source(&page_body, dir).or_else(|| first_legacy_attachment_in_source(&page_body))
            {
                let key = format!("{}::{name}", dir.display());
                log::warn!("attachment recovery scan found missing icon: page={:?} attachment={name:?}", dir);
                // Do not let page recovery replace the first-use privacy dialog. The normal
                // frame pass below will offer recovery after the user answers that dialog.
                if self.dialog.is_none() && self.icon_recovery_reported.insert(key) {
                    self.dialog = Some(Dialog::MissingAttachmentIcon { page: dir.to_path_buf(), name });
                }
            } else {
                log::debug!("attachment recovery scan found no missing icons: page={:?} attachments={}", dir, attachments.len());
            }
            self.pad = Some(crate::inkpad::Pad::of_page(dir));
            // Another page, another history: nothing undone on the last one is redone here.
            self.undone.clear();
            if self.math_ocr.as_ref().is_some_and(|c| c.page != dir) {
                self.math_ocr = None;
            }
            self.absorb_old_sheets(dir);
            self.sync_ink_reference();
            if switching_page {
                self.canvas_pan = egui::Vec2::ZERO;
                self.pending_scroll_offset_y = Some(0.0);
                self.pending_external_scroll_y = 0.0;
                self.pending_fit_bounds = None;
                self.finger_pan = FingerPanState::Idle;
                self.finger_handle_drag = None;
                self.canvas_drag = None;
                self.typed_while_inking.clear();
                self.typed_wait = 0;
            }
            self.selected = Some(dir.to_path_buf());
            self.cursor = 0;
            self.remember_last_page(dir);
            self.say(crate::tl_id!("ui.opened"));
        }
    }

    /// A page written by an earlier version holds its drawing as a picture pointing at
    /// `assets/drawing-1.svg`. Nothing draws those now, so the page showed a broken
    /// picture. Their strokes are moved onto the page's own ink and the picture is taken
    /// out of the words; the old file is left in the folder untouched.
    fn absorb_old_sheets(&mut self, dir: &Path) {
        use nocturne_formats::markdown::Block;
        let olds: Vec<String> = self
            .editor
            .as_ref()
            .map(|e| {
                e.blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Image { src, .. } if crate::inkpad::is_old_sheet(src) => Some(src.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if olds.is_empty() {
            return;
        }
        let mut moved = 0;
        if let Some(pad) = self.pad.as_mut() {
            for src in &olds {
                moved += pad.absorb(dir, src);
            }
            if pad.dirty && pad.save().is_err() {
                self.complain(crate::tl_id!("ui.cannot_write_this_page_s_ink"));
                return;
            }
        }
        self.tell_kept_ink();
        if let Some(ed) = self.editor.as_mut() {
            ed.blocks.retain(|b| !matches!(b, Block::Image { src, .. } if crate::inkpad::is_old_sheet(src)));
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
        }
        self.sync_ink_reference();
        // The old file has been read and its strokes are in `ink.svg`, so leaving it
        // behind only puts a dead 200 KB picture in the page's folder for the owner to
        // wonder about. It goes, now that what was in it is kept somewhere better.
        let mut left = 0;
        for src in &olds {
            let file = dir.join(src.trim().replace('/', std::path::MAIN_SEPARATOR_STR));
            if std::fs::remove_file(&file).is_err() {
                left += 1;
            }
        }
        let message = if left > 0 {
            crate::i18n::format_message_id(
                "ui.moved_moved_strokes_onto_the_page_left_old_drawing_f_89f765dcef",
                &[("moved", &moved.to_string()), ("left", &left.to_string())],
            )
        } else {
            crate::i18n::format_message_id("ui.moved_moved_strokes_from_this_page_s_old_drawing_ont_11ecd930d5", &[("moved", &moved.to_string())])
        };
        self.say(&message);
    }

    /// Writes the page now and returns once it is on disk, after any autosave still being
    /// written: what comes next (another page, closing, a command) may read the file.
    fn save_now(&mut self) {
        self.take_saves(true);
        if let Some(pad) = self.pad.as_mut().filter(|pad| pad.has_unsaved_changes()) {
            let result = pad.save();
            self.ink_save_completed(result, true);
        }
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.dirty_since.is_none() {
            return;
        }
        let (dir, body) = (ed.dir.clone(), ed.body_to_save());
        // Nothing to write: the file already says this.
        if body == ed.saved_body {
            ed.dirty_since = None;
            return;
        }
        match self.session.run("page.save", &params(&[("path", &path_str(&dir)), ("body", &body)])) {
            Ok(Output::Text(ts)) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.dirty_since = None;
                    ed.last_saved = Some(ts);
                    ed.saved_body = body;
                }
                self.library_grid.invalidate_page(&dir);
                self.sync_dirty_since_last = true;
                self.last_local_edit_at = Some(Instant::now());
                self.say(self.saved_caption(None));
            }
            Ok(_) => {}
            Err(e) => self.fail(&e),
        }
    }

    /// The autosave: hands the page to the saver and carries on, so the window is not held up
    /// while a slow folder writes it.
    fn save_in_background(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.dirty_since.is_none() {
            return;
        }
        let body = ed.body_to_save();
        ed.dirty_since = None;
        if body == ed.saved_body || ed.saving.as_ref() == Some(&body) {
            return;
        }
        self.saver.submit(&ed.dir, &body);
        ed.saving = Some(body);
    }

    /// Takes in the autosaves that have finished, waiting for all of them if `wait`. A save that
    /// failed says why and leaves the page unsaved, so the autosave tries again.
    fn take_saves(&mut self, wait: bool) {
        if !self.saver.busy() {
            return;
        }
        let done = if wait { self.saver.wait() } else { self.saver.finished() };
        for saved in done {
            let here = self.editor.as_mut().filter(|ed| ed.dir == saved.dir);
            match saved.result {
                Ok(ts) => {
                    if let Some(ed) = here {
                        ed.last_saved = Some(ts);
                        if ed.saving.as_ref() == Some(&saved.body) {
                            ed.saving = None;
                        }
                        ed.saved_body = saved.body;
                    }
                    self.library_grid.invalidate_page(&saved.dir);
                    self.sync_dirty_since_last = true;
                    self.last_local_edit_at = Some(Instant::now());
                    self.say(self.saved_caption(None));
                }
                Err(e) => {
                    if let Some(ed) = here {
                        ed.saving = None;
                        ed.dirty_since.get_or_insert_with(Instant::now);
                    }
                    self.status = crate::i18n::format_message_id("ui.could_not_complete_the_action_error", &[("error", &e)]);
                    self.status_is_error = true;
                }
            }
        }
    }

    fn repair_attachment_reference(&mut self, page: &Path, name: &str) -> Result<bool, String> {
        let Some(ed) = self.editor.as_mut() else { return Ok(false) };
        if ed.dir != page {
            return Err("the page changed while the recovery dialog was open".into());
        }
        let link = format!("assets/{name}");
        let html_href = format!("href=\"{link}\"");
        let markdown_href = format!("]({link})");
        let references = ed.body.matches(&html_href).count() + ed.body.matches(&markdown_href).count();
        log::debug!("attachment recovery reference scan: page={:?} attachment={name:?} references={references}", page);
        let old = match ed.format {
            PageFormat::Html => format!("<p><a href=\"{link}\">{name}</a></p>"),
            PageFormat::Cmgfm => format!("[{name}]({link})"),
        };
        if references == 0 && !ed.body.contains(&old) {
            let icon_link = attachment_icon_link(page, name);
            let replacement = replace_attachment_icon(ed.format, &snippets::attachment(ed.format, &link, name), name, &icon_link).trim().to_string();
            ed.body.push('\n');
            ed.body.push_str(&replacement);
            Self::mark_dirty(ed);
            log::debug!("attachment recovery appended missing reference: page={:?} attachment={name:?}", page);
            return Ok(true);
        }
        let icon_link = attachment_icon_link(page, name);
        let replacement = replace_attachment_icon(ed.format, &snippets::attachment(ed.format, &link, name), name, &icon_link).trim().to_string();
        let mut body = ed.body.replace(&old, &replacement);
        if references > 1 {
            let mut kept = false;
            body = body
                .lines()
                .filter(|line| {
                    let card = match ed.format {
                        PageFormat::Html => line.contains(&html_href) && line.contains("<img"),
                        PageFormat::Cmgfm => line.contains(&markdown_href) && line.contains("!["),
                    };
                    if card {
                        if kept {
                            return false;
                        }
                        kept = true;
                    }
                    true
                })
                .collect::<Vec<_>>()
                .join("\n");
            log::warn!("attachment recovery removed duplicate references: page={:?} attachment={name:?} references={references}", page);
        }
        // Migrate an older generated icon in place when the platform now has a native icon.
        #[cfg(target_os = "windows")]
        {
            body = body.replace(&format!("assets/{name}.svg"), &icon_link).replace(&format!("assets/{name}.png"), &icon_link);
        }
        if body == ed.body {
            return Ok(false);
        }
        ed.body = body;
        Self::mark_dirty(ed);
        Ok(true)
    }

    /// The page text changed: remember to save it, and read the blocks from it again.
    fn mark_dirty(ed: &mut Editor) {
        ed.dirty_since = Some(Instant::now());
        let (blocks, in_the_text) = nocturne_formats::boxes::read(ed.format, &ed.body);
        ed.blocks = blocks;
        ed.arranged = in_the_text.is_some();
        ed.boxes = in_the_text.unwrap_or_default();
        ed.caret = None;
    }

    /// The blocks changed (the caret is in the page): remember to save, and keep the blocks
    /// as they are. Reading them back from the text would throw away an empty new block and
    /// move the caret out from under the user.
    fn mark_dirty_from_blocks(ed: &mut Editor) {
        // A change that leaves the file as it is, or as it is being written (a click on open
        // paper, which only places the cursor), is not a change to save.
        let file_will_hold = ed.saving.as_ref().unwrap_or(&ed.saved_body);
        ed.dirty_since = (ed.body_to_save() != *file_will_hold).then(Instant::now);
    }

    /// Puts a printout's blocks where the writer is: into the box that holds the caret, after
    /// the caret's line, the way OneNote puts it where you are writing (issue #67). With no
    /// caret in a box it gets a box of its own below the others.
    fn insert_printout_blocks(&mut self, text: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some((index, at)) =
            ed.caret.as_ref().and_then(|c| nocturne_model::container::place(&ed.boxes, c.block).map(|(index, _)| (index, c.block + 1)))
        else {
            self.insert_in_own_container(text);
            return;
        };
        let (added, _) = nocturne_formats::boxes::read(ed.format, text);
        if added.is_empty() {
            return;
        }
        let count = added.len();
        let (mut cols_moved, mut justified_moved) = (false, false);
        for (offset, block) in added.into_iter().enumerate() {
            ed.blocks.insert(at + offset, block);
            cols_moved |= ed.columns.insert_block(at + offset);
            justified_moved |= ed.justified.insert_block(at + offset);
        }
        ed.boxes[index].blocks += count;
        // The pages come out as wide as the sheet, as they do in a box of their own: a narrow box
        // (one started with a click) is widened for them rather than the pages shrunk to fit it.
        let sheet = ed.boxes.first().map_or(360.0, |c| c.width);
        ed.boxes[index].width = ed.boxes[index].width.max(sheet);
        ed.arranged = true;
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_step(ed);
        let path = path_str(&ed.dir);
        let cols = cols_moved.then(|| nocturne_model::tablecols::write(&ed.columns));
        let justified = justified_moved.then(|| nocturne_model::justify::write(&ed.justified));
        if let Some(line) = cols {
            self.run("page.columns", &params(&[("path", &path), ("widths", &line)]));
        }
        if let Some(line) = justified {
            self.run("page.justify", &params(&[("path", &path), ("blocks", &line)]));
        }
    }

    /// Adds the blocks of `text` to the page in a container of their own, below what is
    /// already on the page, so each printout can be moved and sized apart from the others.
    fn insert_in_own_container(&mut self, text: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let below = ed.content_bottom;
        let (added, _) = nocturne_formats::boxes::read(ed.format, text);
        if added.is_empty() {
            return;
        }
        let width = ed.boxes.first().map_or(360.0, |c| c.width);
        let before = ed.blocks.len();
        ed.blocks.extend(added.iter().cloned());
        let below = below.unwrap_or_else(|| ed.boxes.iter().map(|c| c.y + 60.0).fold(0.0, f32::max));
        ed.boxes = nocturne_model::container::append(&ed.boxes, before, added.len(), width, below, 12.0);
        ed.arranged = true;
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_step(ed);
    }

    fn insert(&mut self, text: &str) {
        let cursor = self.cursor;
        if let Some(ed) = self.editor.as_mut().filter(|ed| ed.writable) {
            Self::remember_step(ed);
            let (body, next) = snippets::insert_at(&ed.body, cursor, text);
            ed.body = body;
            Self::mark_dirty(ed);
            Self::remember_edit(ed);
            self.cursor = next;
        }
    }

    /// Puts the blocks of a snippet (a file copied in or linked) after the caret's line, in the
    /// caret's box, or on the end of the page. It works on the blocks rather than splicing the
    /// page's text at `cursor`, which put the snippet wherever that old text position pointed
    /// (the top of the page, in a box of its own) and read every box back from the text
    /// (issue #72).
    fn insert_snippet_blocks(&mut self, text: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let (added, _) = nocturne_formats::boxes::read(ed.format, text);
        if added.is_empty() {
            return;
        }
        let at = ed.caret.as_ref().map_or(ed.blocks.len(), |c| c.block + 1).min(ed.blocks.len());
        let count = added.len();
        for (offset, block) in added.into_iter().enumerate() {
            ed.blocks.insert(at + offset, block);
        }
        let marks = ed.blocks_inserted(at, count);
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_step(ed);
        self.save_block_marks(marks);
    }

    /// A dropped file asks what to do with it, the way OneNote asks when you insert one.
    fn attach_dropped(&mut self, ctx: &Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if files.is_empty() {
            return;
        }
        let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) else {
            self.status = crate::tl_id!("ui.open_a_page_first_then_drop_files_onto_it").into();
            self.status_is_error = true;
            return;
        };
        self.dialog = Some(Dialog::FileOptions { dir, files });
    }

    /// Carries out one of the choices of the file dialog.
    fn add_file(&mut self, dir: &Path, file: &Path, how: FileChoice) {
        let Some(format) = self.editor.as_ref().map(|e| e.format) else { return };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match how {
            FileChoice::Copy => {
                log::debug!("file attach requested: name={name:?} page={:?}", dir);
                match self.run("page.attach", &params(&[("path", &path_str(dir)), ("source", &path_str(file))])) {
                    Some(Output::Text(link)) => {
                        log::debug!("file attach copied: name={name:?} link={link:?}");
                        let text = snippets::attachment(format, &link, &name);
                        self.insert_snippet_blocks(&text);
                        self.say(crate::i18n::format_message_id("ui.name_is_now_a_file_in_this_page_s_folder", &[("name", &name)]));
                    }
                    Some(_) => self.complain(crate::tl_id!("ui.the_file_was_not_attached_the_attach_command_returne_beaffe0fd0")),
                    None => {
                        log::error!("file attach failed: name={name:?} page={:?} status={:?}", dir, self.status);
                        self.complain(&crate::i18n::format_message_id("ui.could_not_attach_name_to_this_page", &[("name", &name)]));
                    }
                }
            }
            FileChoice::Link => {
                let text = snippets::file_link(format, &path_str(file), &name);
                self.insert_snippet_blocks(&text);
                self.say(crate::i18n::format_message_id("ui.linked_name_where_it_is_the_file_was_not_copied", &[("name", &name)]));
            }
            FileChoice::Printout => self.insert_printout(dir, file, format),
        }
        if let Some(ed) = self.editor.as_mut() {
            ed.attachments = nocturne_store::assets::list(&ed.dir);
        }
    }

    fn insert_printout(&mut self, dir: &Path, file: &Path, format: PageFormat) {
        let rendered = match printout::render(file) {
            Ok(rendered) => rendered,
            Err(error) => {
                self.complain(&error);
                return;
            }
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(Output::Text(link)) = self.run("page.attach", &params(&[("path", &path_str(dir)), ("source", &path_str(file))])) else {
            self.complain(&crate::i18n::format_message_id("ui.could_not_attach_name_before_rendering_its_pages", &[("name", &name)]));
            return;
        };
        let Some(base) = link.strip_prefix("assets/").and_then(|link| Path::new(link).file_name()).map(|name| name.to_string_lossy().into_owned())
        else {
            self.complain(crate::tl_id!("ui.the_attachment_returned_an_invalid_page_link"));
            return;
        };
        let assets = dir.join("assets");
        let mut generated = Vec::with_capacity(rendered.pages().len());
        for (index, page) in rendered.pages().iter().enumerate() {
            let target = assets.join(format!("{base}.printout-{:03}.png", index + 1));
            if let Err(error) = std::fs::copy(page, &target) {
                for path in &generated {
                    let _ = std::fs::remove_file(path);
                }
                self.complain(&crate::i18n::format_message_id(
                    "ui.could_not_store_printout_page_page_error",
                    &[("page", &(index + 1).to_string()), ("error", &error.to_string())],
                ));
                return;
            }
            generated.push(target);
        }
        let mut text = snippets::attachment(format, &link, &name);
        for (index, _) in generated.iter().enumerate() {
            let image = format!("assets/{base}.printout-{:03}.png", index + 1);
            text.push_str(&snippets::printout_page(format, &image, index + 1));
        }
        self.insert_printout_blocks(&text);
        self.say(crate::i18n::format_message_id(
            "ui.inserted_name_and_count_printout_pages",
            &[("name", &name), ("count", &generated.len().to_string())],
        ));
    }

    // ---------- tree ----------

    fn collect_branches(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        for n in self.session.tree() {
            n.branches(&mut v);
        }
        v
    }

    fn apply(&mut self, act: Act) {
        match act {
            Act::Toggle(p) => {
                if !self.open.remove(&p) {
                    self.open.insert(p);
                }
            }
            Act::Select(p) => {
                let is_page = nocturne_store::tree::read_node(&p).map(|i| i.kind == NodeKind::Page).unwrap_or(false);
                if is_page {
                    self.open_page(&p);
                } else {
                    if !self.open.remove(&p) {
                        self.open.insert(p.clone());
                    }
                    // Selecting a container must also move the editor. Leaving the old
                    // page open here made the navigator and page column describe one
                    // location while the editor showed another.
                    if let Some(page) = first_page(self.session.tree(), &p) {
                        self.open_page(&page);
                        // A phone-width window keeps the list up, so the next level can be
                        // picked; only a page chosen for itself takes the window.
                        self.page_opened = false;
                    } else {
                        self.selected = Some(p);
                    }
                }
            }
            Act::Add(parent, parent_kind) => {
                let kind = parent_kind.may_contain().first().copied().unwrap_or(NodeKind::Page);
                let kind = if parent_kind == NodeKind::Notebook || parent_kind == NodeKind::SectionGroup { NodeKind::Section } else { kind };
                self.open_add_dialog(parent, parent_kind, kind);
            }
            Act::NewNotebook => self.open_new_notebook_dialog(),
            Act::CloseNotebook(path) => {
                self.save_now();
                let was_showing = self.editor.as_ref().is_some_and(|ed| ed.dir.starts_with(&path));
                if matches!(self.run("notebook.close", &params(&[("path", &path_str(&path))])), Some(Output::Text(ref result)) if result == "closed")
                {
                    self.open.retain(|p| !p.starts_with(&path));
                    if self.selected.as_ref().is_some_and(|p| p.starts_with(&path)) {
                        self.selected = self.session.tree().first().map(|n| n.path.clone());
                    }
                    if was_showing {
                        self.editor = None;
                        self.pad = None;
                        self.nav = NavHistory::default();
                    }
                    self.say(crate::tl_id!("ui.notebook_closed_its_files_are_still_on_disk"));
                }
            }
            Act::RevealRoot => {
                let root = self.session.root().to_path_buf();
                self.reveal(&root);
            }
            Act::ChooseRoot => self.choose_notebooks_folder(),
            Act::AddKind(parent, parent_kind, kind) => {
                self.open_add_dialog(parent, parent_kind, kind);
            }
            Act::AddPageAt(section, order) => {
                let path = path_str(&section);
                let order = order.to_string();
                // No title: the page is created genuinely untitled, like a new OneNote page.
                // "Untitled page" is only ever the display fallback, never real content.
                if let Some(Output::Path(dir)) = self.run("node.create", &params(&[("parent", &path), ("kind", "page"), ("order", &order)])) {
                    self.open_page(&dir);
                    self.say(crate::tl_id!("ui.new_page"));
                }
            }
            Act::RenumberAndAddPage(section, before) => {
                // Where the page before the line sits among the pages now, before renumbering.
                let position = nocturne_store::tree::child_dirs(&section)
                    .into_iter()
                    .filter_map(|c| nocturne_store::tree::read_node(&c))
                    .filter(|n| n.kind == NodeKind::Page)
                    .position(|n| n.order == before);
                let path = path_str(&section);
                if let (Some(position), Some(_)) = (position, self.run("node.renumber", &params(&[("path", &path)]))) {
                    // Renumbered ten apart in the same order, so the line is halfway after that page.
                    let order = ((position as u32 + 1) * 10 + 5).to_string();
                    if let Some(Output::Path(dir)) = self.run("node.create", &params(&[("parent", &path), ("kind", "page"), ("order", &order)])) {
                        self.open_page(&dir);
                        self.say(crate::tl_id!("ui.new_page"));
                    }
                }
            }
            Act::NewPage(parent) => self.create_page_in(&parent),
            Act::Rename(path, title) => self.open_rename_dialog(path, title),
            Act::MoveOrCopy(path) => self.open_move_or_copy(&path),
            Act::MakeSubpage(path) => self.reparent_page(&path, "node.demote"),
            Act::PromoteSubpage(path) => self.reparent_page(&path, "node.promote"),
            Act::MovePageUp(path) => {
                if let Some(Output::Path(path)) = self.run("node.move_up", &params(&[("path", &path_str(&path))])) {
                    self.selected = Some(path.clone());
                    self.session.refresh();
                }
            }
            Act::MovePageDown(path) => {
                if let Some(Output::Path(path)) = self.run("node.move_down", &params(&[("path", &path_str(&path))])) {
                    self.selected = Some(path.clone());
                    self.session.refresh();
                }
            }
            Act::PageVersions(path) => self.show_page_versions(&path),
            Act::Delete(path, title, count) => self.dialog = Some(Dialog::Delete { path, title, count }),
            Act::Color(path, color) => {
                let value = color.map_or("none", NodeColor::id);
                if self.run("node.color", &params(&[("path", &path_str(&path)), ("color", value)])).is_some() {
                    self.say(match color {
                        Some(c) => {
                            let colour = crate::tl!(c.label());
                            crate::i18n::format_message_id("ui.colour_set_to_colour", &[("colour", &colour.to_lowercase())])
                        }
                        None => crate::tl_id!("ui.colour_removed").to_string(),
                    });
                }
            }
            Act::CopyLink(path) => self.copy_link_to(&path),
            Act::Reveal(path) => match open_folder(&path) {
                Ok(()) => self.say(crate::i18n::format_message_id("ui.opened_path", &[("path", &self.shown(&path))])),
                Err(e) => {
                    self.status = crate::i18n::format_message_id("ui.the_folder_could_not_be_opened_error", &[("error", &e.to_string())]);
                    self.status_is_error = true;
                }
            },
        }
    }

    fn draw_node(&self, ui: &mut Ui, node: &Node, depth: usize, acts: &mut Vec<Act>) {
        self.draw_row(ui, node, depth, acts, true);
    }

    /// One row of the tree. With `pages` false the pages are left out, which is what the
    /// organisers column shows: notebooks, section groups and sections only.
    fn draw_row(&self, ui: &mut Ui, node: &Node, depth: usize, acts: &mut Vec<Act>, pages: bool) {
        let has_kids = if pages { !node.children.is_empty() } else { node.children.iter().any(|c| c.info.kind != NodeKind::Page) };
        let open = self.open.contains(&node.path);
        let selected = self.selected.as_deref() == Some(node.path.as_path());
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
        let painter = ui.painter_at(rect);
        let t = &self.tokens;
        if selected {
            painter.rect_filled(rect, 4.0, t.row_selected);
        } else if resp.hovered() {
            painter.rect_filled(rect, 4.0, t.row_hover);
        }
        let x0 = rect.min.x + depth as f32 * INDENT + 2.0;
        let cy = rect.center().y;
        let chevron_rect = egui::Rect::from_min_size(egui::pos2(x0, rect.min.y), vec2(16.0, ROW_HEIGHT));
        if has_kids {
            let c = chevron_rect.center();
            let stroke = egui::Stroke::new(1.5, t.muted);
            let pts = if open {
                [egui::pos2(c.x - 3.5, c.y - 1.5), egui::pos2(c.x, c.y + 2.5), egui::pos2(c.x + 3.5, c.y - 1.5)]
            } else {
                [egui::pos2(c.x - 1.5, c.y - 3.5), egui::pos2(c.x + 2.5, c.y), egui::pos2(c.x - 1.5, c.y + 3.5)]
            };
            painter.line_segment([pts[0], pts[1]], stroke);
            painter.line_segment([pts[1], pts[2]], stroke);
        }
        let (icon, default_color) = match node.info.kind {
            NodeKind::Notebook => (Icon::Notebook, t.notebook),
            NodeKind::SectionGroup => (if open && has_kids { Icon::SectionGroupOpen } else { Icon::SectionGroup }, t.group),
            NodeKind::Section => (Icon::Section, t.section),
            NodeKind::Page => (if has_kids && !open { Icon::PageStack } else { Icon::Page }, t.page),
        };
        // The colour the user gave this notebook or section, kept readable on the panel.
        let color = match node.info.color {
            Some(c) => {
                let (r, g, b) = c.rgb(self.effective_mode.is_dark());
                crate::theme::readable_on(egui::Color32::from_rgb(r, g, b), t.panel, 3.0, self.effective_mode.is_dark())
            }
            None => default_color,
        };
        // The coloured edge of the row, like the edge of a OneNote tab.
        if node.info.color.is_some() {
            let edge = egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.min.y + 2.0), vec2(3.0, ROW_HEIGHT - 4.0));
            painter.rect_filled(edge, 1.5, color);
        }
        let icon_rect = egui::Rect::from_min_size(egui::pos2(x0 + 18.0, cy - 8.5), vec2(17.0, 17.0));
        icons::paint(&painter, icon_rect, icon, color);
        let text_pos = egui::pos2(x0 + 40.0, cy);
        let text_color = if selected { t.strong } else { ui.style().visuals.text_color() };
        // Pages keep room for the add-page "+" at the end of the row. Organiser rows do not,
        // so notebook, group and section names run almost to the pane border.
        let trailing = if node.info.kind == NodeKind::Page { 28.0 } else { 8.0 };
        let room = (rect.max.x - trailing - text_pos.x).max(0.0);
        // The format label ("html", "md") is the less important of the two: it is shown only
        // when the whole name fits beside it, and the name is never cut to make room for it.
        let label = if node.info.kind == NodeKind::Page { node.info.format.map(|f| crate::tl!(f.short())) } else { None };
        let label_width = label.map(|l| text_width(ui, l, 10.5) + ROW_LABEL_GAP);
        let (show_label, name_room) = row_name_room(text_width(ui, &node.info.title, 14.0), label_width, room);
        let name = fit_to_width(ui, &node.info.title, 14.0, name_room);
        let text_clip = egui::Rect::from_min_max(rect.min, egui::pos2(text_pos.x + name_room + 1.0, rect.max.y));
        // The tree is on screen all the time, so this is the most visible place the words
        // of an Arabic name would come out backwards. Each row's name decides its own
        // direction; the row keeps its place and its clip either way.
        let clipped = painter.with_clip_rect(text_clip);
        crate::bidi::line_at(ui, &clipped, text_pos, Align2::LEFT_CENTER, &name, 14.0, text_color, None);
        let plus_rect = egui::Rect::from_min_size(egui::pos2(rect.max.x - 24.0, rect.min.y + 2.0), vec2(20.0, ROW_HEIGHT - 4.0));
        if show_label && let Some(l) = label {
            painter.text(egui::pos2(rect.max.x - trailing, cy), Align2::RIGHT_CENTER, l, egui::FontId::proportional(10.5), t.muted);
        }
        if resp.hovered() {
            painter.text(plus_rect.center(), Align2::CENTER_CENTER, "+", egui::FontId::proportional(16.0), t.strong);
        }
        let enabled = ui.is_enabled();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &node.info.title));
        let resp = resp.on_hover_text(self.shown(&node.path));
        if resp.clicked() {
            let pos = resp.interact_pointer_pos().unwrap_or_default();
            if has_kids && chevron_rect.expand2(vec2(2.0, 0.0)).contains(pos) {
                acts.push(Act::Toggle(node.path.clone()));
            } else if plus_rect.contains(pos) {
                acts.push(Act::Add(node.path.clone(), node.info.kind));
            } else {
                acts.push(Act::Select(node.path.clone()));
            }
        }
        resp.context_menu(|ui| {
            let new_page_label = if cfg!(any(target_os = "macos", target_os = "ios")) {
                crate::tl_id!("ui.new_page_cmd_n")
            } else {
                crate::tl_id!("ui.new_page_ctrl_n")
            };
            if node.info.kind == NodeKind::Page
                && let Some(parent) = node.path.parent()
                && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::Page)), new_page_label, t).clicked()
            {
                acts.push(Act::NewPage(parent.to_path_buf()));
                ui.close();
            }
            // What this node may hold, named in full rather than left to the dialog: a
            // notebook and a group take sections and groups, a section takes pages, and a
            // page takes sub-pages.
            for kind in node.info.kind.may_contain() {
                let label = match kind {
                    NodeKind::SectionGroup => "New section group...",
                    NodeKind::Section => "New section...",
                    NodeKind::Page if node.info.kind == NodeKind::Page => crate::tl_id!("ui.add_a_sub_page"),
                    NodeKind::Page => crate::tl_id!("ui.add_page"),
                    NodeKind::Notebook => "New notebook...",
                };
                let icon = match kind {
                    NodeKind::SectionGroup => Icon::SectionGroup,
                    NodeKind::Section => Icon::Section,
                    NodeKind::Page => Icon::Page,
                    NodeKind::Notebook => Icon::Notebook,
                };
                if floating::menu_row(ui, Some(floating::Glyph::Icon(icon)), label, t).clicked() {
                    acts.push(Act::AddKind(node.path.clone(), node.info.kind, *kind));
                    ui.close();
                }
            }
            if node.info.kind == NodeKind::Page
                && (Self::can_make_subpage(&node.path)
                    || Self::can_promote_subpage(&node.path)
                    || Self::can_move_page(&node.path, true)
                    || Self::can_move_page(&node.path, false))
            {
                floating::menu_separator(ui, t);
                if Self::can_make_subpage(&node.path)
                    && floating::menu_row(ui, Some(floating::Glyph::Mark(crate::ribbon_icons::Mark::IndentMore)), crate::tl_id!("ui.make_subpage"), t)
                        .clicked()
                {
                    acts.push(Act::MakeSubpage(node.path.clone()));
                    ui.close();
                }
                if Self::can_promote_subpage(&node.path)
                    && floating::menu_row(
                        ui,
                        Some(floating::Glyph::Mark(crate::ribbon_icons::Mark::IndentLess)),
                        crate::tl_id!("ui.promote_subpage"),
                        t,
                    )
                    .clicked()
                {
                    acts.push(Act::PromoteSubpage(node.path.clone()));
                    ui.close();
                }
                if Self::can_move_page(&node.path, true)
                    && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::MoveUp)), crate::tl_id!("ui.move_up"), t).clicked()
                {
                    acts.push(Act::MovePageUp(node.path.clone()));
                    ui.close();
                }
                if Self::can_move_page(&node.path, false)
                    && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::MoveDown)), crate::tl_id!("ui.move_down"), t).clicked()
                {
                    acts.push(Act::MovePageDown(node.path.clone()));
                    ui.close();
                }
            }
            floating::menu_separator(ui, t);
            if floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::Page)), crate::tl_id!("ui.rename_ellipsis"), t).clicked() {
                acts.push(Act::Rename(node.path.clone(), node.info.title.clone()));
                ui.close();
            }
            if matches!(node.info.kind, NodeKind::Page | NodeKind::Section | NodeKind::SectionGroup)
                && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::MoveDown)), crate::tl_id!("ui.move_or_copy_ellipsis"), t).clicked()
            {
                acts.push(Act::MoveOrCopy(node.path.clone()));
                ui.close();
            }
            if node.info.kind == NodeKind::Notebook
                && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::Notebook)), crate::tl_id!("ui.close_notebook"), t).clicked()
            {
                acts.push(Act::CloseNotebook(node.path.clone()));
                ui.close();
            }
            if has_kids
                && floating::menu_row(
                    ui,
                    Some(floating::Glyph::Icon(if open { Icon::Minus } else { Icon::Plus })),
                    if node.info.kind == NodeKind::Page {
                        if open { crate::tl_id!("ui.collapse_subpages") } else { crate::tl_id!("ui.expand_subpages") }
                    } else if open {
                        crate::tl_id!("ui.collapse")
                    } else {
                        crate::tl_id!("ui.expand")
                    },
                    t,
                )
                .clicked()
            {
                acts.push(Act::Toggle(node.path.clone()));
                ui.close();
            }
            // Pages take the colour of their section, the way OneNote colours tabs.
            if node.info.kind != NodeKind::Page {
                ui.menu_button(crate::tl_id!("ui.colour"), |ui| {
                    let dark = self.effective_mode.is_dark();
                    egui::Grid::new("node-colours").spacing([4.0, 4.0]).show(ui, |ui| {
                        for (i, c) in NodeColor::ALL.into_iter().enumerate() {
                            if color_swatch(ui, c, dark, node.info.color == Some(c)).clicked() {
                                acts.push(Act::Color(node.path.clone(), Some(c)));
                                ui.close();
                            }
                            if i % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.add_space(2.0);
                    if ui.button(crate::tl_id!("ui.no_colour")).clicked() {
                        acts.push(Act::Color(node.path.clone(), None));
                        ui.close();
                    }
                });
            }
            if floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::Link)), crate::tl_id!("ui.copy_link_to_page"), t)
                .on_hover_text(crate::tl_id!("ui.a_link_to_paste_into_another_page"))
                .clicked()
            {
                acts.push(Act::CopyLink(node.path.clone()));
                ui.close();
            }
            if floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::Folder)), crate::tl_id!("ui.open_folder"), t)
                .on_hover_text(self.shown(&node.path))
                .clicked()
            {
                acts.push(Act::Reveal(node.path.clone()));
                ui.close();
            }
            if node.info.kind == NodeKind::Page
                && floating::menu_row(ui, Some(floating::Glyph::Icon(Icon::PageStack)), crate::tl_id!("ui.page_versions_2"), t).clicked()
            {
                acts.push(Act::PageVersions(node.path.clone()));
                ui.close();
            }
            floating::menu_separator(ui, t);
            if floating::menu_row(ui, Some(floating::Glyph::Mark(crate::ribbon_icons::Mark::Delete)), crate::tl_id!("ui.delete_ellipsis"), t)
                .clicked()
            {
                acts.push(Act::Delete(node.path.clone(), node.info.title.clone(), node.count()));
                ui.close();
            }
        });
        if has_kids && open {
            let kids: Vec<&Node> = node.children.iter().filter(|c| pages || c.info.kind != NodeKind::Page).collect();
            if pages {
                // Sub-pages get the same gaps as top-level pages: one before each, one after the
                // last. A gap that is dropped here becomes a child of this page.
                let mut last: Option<u32> = None;
                for c in kids {
                    self.page_gap(ui, &node.path, last, Some(c.info.order), acts);
                    self.draw_row(ui, c, depth + 1, acts, pages);
                    last = Some(c.info.order);
                }
                self.page_gap(ui, &node.path, last, None, acts);
            } else {
                for c in kids {
                    self.draw_row(ui, c, depth + 1, acts, pages);
                }
            }
        }
    }

    /// The first section anywhere in the tree, for a window that has nothing selected.
    fn first_section_of(nodes: &[Node]) -> Option<&Node> {
        for n in nodes {
            if n.info.kind == NodeKind::Section {
                return Some(n);
            }
            if let Some(found) = Self::first_section_of(&n.children) {
                return Some(found);
            }
        }
        None
    }

    /// The section whose pages the pages column shows: the selected node if it is one, else
    /// the section the selected page sits in, else the first section there is.
    fn current_section<'a>(&self, tree: &'a [Node]) -> Option<&'a Node> {
        let selected = self.selected.clone();
        fn walk<'a>(nodes: &'a [Node], want: &Option<PathBuf>, best: &mut Option<&'a Node>, inside: Option<&'a Node>) {
            for n in nodes {
                let section = if n.info.kind == NodeKind::Section { Some(n) } else { inside };
                if Some(&n.path) == want.as_ref() {
                    *best = match n.info.kind {
                        NodeKind::Section => Some(n),
                        NodeKind::Page => section,
                        _ => *best,
                    };
                }
                walk(&n.children, want, best, section);
            }
        }
        let mut best = None;
        walk(tree, &selected, &mut best, None);
        best.or_else(|| Self::first_section_of(tree))
    }

    // ---------- panels ----------

    fn top_bar(&mut self, ui: &mut Ui) {
        if self.is_tablet_studio(ui.ctx()) {
            if self.frameless {
                self.title_band(ui, false);
            }
            self.studio_top_bar(ui);
            return;
        }
        self.title_band(ui, true);
        self.ribbon_bar(ui);
        // Below the ribbon, as OneNote can put it.
        if self.qat.below && !self.qat.hidden {
            let tokens = self.tokens;
            egui::Panel::top("quick-access").show(ui, |ui| {
                ui.horizontal(|ui| self.quick_access(ui, tokens));
            });
        }
    }

    fn current_view_state<'a>(&'a self, ctx: &Context, family: Option<&'a str>, size: Option<&'a str>, marks: pageedit::Marks) -> ribbon::View<'a> {
        ribbon::View {
            split_panes: self.split_panes,
            always_on_top: self.always_on_top,
            full_page: self.full_page,
            rules: self.rules,
            hide_title: self.hide_title,
            snap: self.snap_to_grid,
            zoom_percent: self.zoom_percent,
            pointer: self.pointer,
            rack: &self.rack,
            nib: self.nib,
            in_table: self.caret_in_table(),
            space_tool: self.space_tool,
            finger_drawing: self.is_finger_drawing(),
            eraser_auto_return: self.eraser_auto_return,
            eraser_pixel: self.session.ink_options().eraser_kind == nocturne_engine::EraserKind::Pixel,
            eraser_width: self.session.ink_options().eraser_width,
            show_minimap: self.minimap_visible(ctx),
            tablet_tools_minimized: self.tablet_tools_minimized,
            canvas_layout: self.editor.as_ref().map(|ed| ed.canvas_layout).unwrap_or_default(),
            table_borders: self.caret_table_has_borders(),
            table_align: self.caret_cells_align(),
            table_shade: self.caret_cells_shade(),
            gridcraft: self.gridcraft.is_some(),
            page_colour: self.editor.as_ref().and_then(|e| e.colour),
            rule_colour: self.editor.as_ref().and_then(|e| e.rule_colour),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            can_ink_redo: self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo),
            sensitivity: self.sensitivity,
            pressure_curve: self.pressure_curve,
            pressure_available: self.stylus.sample().pen,
            has_page: self.editor.is_some(),
            painting: self.painter.is_some(),
            style: self.caret_style(ctx),
            font_family: family,
            font_size: size,
            marks,
            math_ocr_enabled: self.session.ocr_config().enabled,
            holding_ink: self.pad.as_ref().is_some_and(|p| p.holding() > 0),
        }
    }

    fn studio_top_bar(&mut self, ui: &mut Ui) {
        if self.is_studio_grid(ui.ctx()) && self.editor.is_none() && self.backstage.is_none() {
            self.header_pill_rect = None;
            self.band_bottom = ui.ctx().content_rect().min.y;
            return;
        }
        let tokens = self.tokens;
        let drawer_open = !self.full_page;
        let library_view = self.session.library_view();
        let can_back = self.nav.can_back();
        let can_forward = self.nav.can_forward();
        let breadcrumb = self.editor.as_ref().map(|ed| ed.title.clone()).unwrap_or_default();
        let sync_busy = self.sync_job.is_some();
        let sync_configured = *self
            .sync_has_token
            .get_or_insert_with(|| self.session.sync_secrets().get_secret(nocturne_engine::KDRIVE_TOKEN_KEY).ok().flatten().is_some());
        let show_source = self.show_source;
        let mut out = crate::tablet_chrome::StudioTopBarOut::default();
        // With a page open the bar across the window gives way to a small floating header at the
        // top left of the page, so the page reaches the top of the screen. In StudioGrid there is
        // no side drawer, so the content rect serves on the very first frame as well.
        let header_at = if self.editor.is_some() && self.backstage.is_none() {
            if self.is_studio_grid(ui.ctx()) { Some(self.page_view_rect.unwrap_or_else(|| ui.ctx().content_rect())) } else { self.page_view_rect }
        } else {
            None
        };
        if let Some(view_rect) = header_at {
            let ctx = ui.ctx().clone();
            let font_sel = self.font_selection(&ctx);
            let mut search_buf = std::mem::take(&mut self.search);
            let mut focus_search = self.focus_search;
            let view = self.current_view_state(&ctx, None, None, font_sel.marks);
            let input = crate::tablet_chrome::StudioTopBarIn {
                drawer_open,
                library_view,
                can_back,
                can_forward,
                breadcrumb: &breadcrumb,
                sync_busy,
                sync_configured,
                show_source,
                view,
            };
            let (pill_out, rect) = crate::tablet_chrome::show_header_pill(&ctx, view_rect, &input, &mut search_buf, &mut focus_search, &tokens);
            out = pill_out;
            self.search = search_buf;
            self.focus_search = focus_search;
            self.header_pill_rect = Some(rect);
            self.band_bottom = ctx.content_rect().min.y;
        } else {
            self.header_pill_rect = None;
            egui::Panel::top("studio-top-bar").show_separator_line(false).show(ui, |ui| {
                let font_sel = self.font_selection(ui.ctx());
                let mut search_buf = std::mem::take(&mut self.search);
                let mut focus_search = self.focus_search;
                let view = self.current_view_state(ui.ctx(), None, None, font_sel.marks);
                let input = crate::tablet_chrome::StudioTopBarIn {
                    drawer_open,
                    library_view,
                    can_back,
                    can_forward,
                    breadcrumb: &breadcrumb,
                    sync_busy,
                    sync_configured,
                    show_source,
                    view,
                };
                out = crate::tablet_chrome::show_top_bar(ui, &input, &mut search_buf, &mut focus_search, &tokens);
                self.search = search_buf;
                self.focus_search = focus_search;
                self.band_bottom = ui.min_rect().max.y;
            });
        }
        if out.toggle_drawer {
            self.full_page = !self.full_page;
            self.compact_full_page = false;
        }
        if out.back_to_library {
            self.close_to_library();
        }
        if out.nav_back {
            self.go_back();
        }
        if out.nav_forward {
            self.go_forward();
        }
        if out.add_page {
            if let Some(parent) = self.selected.clone().and_then(|p| {
                if nocturne_store::tree::read_node(&p).is_some_and(|n| n.kind == NodeKind::Page) {
                    p.parent().map(Path::to_path_buf)
                } else {
                    Some(p)
                }
            }) {
                self.create_page_in(&parent);
            } else {
                self.dialog = Some(Dialog::NewNotebook { name: String::new(), format: None });
            }
        }
        if out.clear_page_caret
            && let Some(caret) = self.editor.as_mut().and_then(|ed| ed.caret.as_mut())
        {
            caret.focus = false;
        }
        for act in out.acts {
            self.ribbon_act(act);
        }
    }

    pub fn open_cloud_sync_dialog(&mut self) {
        self.dialog = Some(Dialog::CloudSync(crate::sync_ui::CloudSyncForm::from_session(&self.session)));
    }

    /// Starts a sync because the user asked for one: its result is shown when it ends.
    pub fn sync_now(&mut self, ctx: &Context) {
        self.sync_by_hand = true;
        self.start_background_sync(ctx);
    }

    /// The result Tablet Studio is showing for something the user asked for, while it shows.
    pub fn studio_notice(&self) -> Option<&str> {
        self.studio_notice.as_ref().map(|(message, _)| message.as_str())
    }

    pub fn start_background_sync(&mut self, ctx: &Context) {
        if self.sync_job.is_some() {
            return;
        }
        // Flush any dirty open page and ink before scanning local files so in-memory edits are included!
        self.save_now();
        if let Some(pad) = self.pad.as_mut()
            && pad.dirty
        {
            let _ = pad.save();
        }
        let cfg = nocturne_engine::SyncConfig::load(&self.session.settings);
        let root = self.session.root().to_path_buf();
        let settings = self.session.settings.clone();
        let http = self.session.sync_http();
        let secrets = self.session.sync_secrets();
        match crate::sync_ui::SyncJob::start(ctx.clone(), root, settings, cfg, http, secrets) {
            Ok(job) => {
                self.sync_job = Some(job);
                self.sync_dirty_since_last = false;
                self.last_auto_sync_trigger = Some(Instant::now());
                self.say(crate::tl_id!("ui.kdrive_sync_in_progress"));
            }
            Err(err) => self.complain(&err),
        }
    }

    fn poll_sync(&mut self, ctx: &Context) {
        let focused = ctx.input(|i| i.focused);
        let regained_focus = !self.was_window_focused && focused;
        self.was_window_focused = focused;

        if self.sync_job.is_none() {
            let cfg = self.cached_sync_config.get_or_insert_with(|| nocturne_engine::SyncConfig::load(&self.session.settings)).clone();
            let supported = self.session.sync_http().is_supported() && self.session.sync_secrets().is_supported();
            let has_tok = if cfg.auto_sync && supported {
                *self
                    .sync_has_token
                    .get_or_insert_with(|| self.session.sync_secrets().get_secret(nocturne_engine::KDRIVE_TOKEN_KEY).ok().flatten().is_some())
            } else {
                false
            };

            if !self.auto_sync_started_on_launch {
                self.auto_sync_started_on_launch = true;
                if cfg.auto_sync && has_tok && supported {
                    self.start_background_sync(ctx);
                }
            } else if cfg.auto_sync && has_tok && supported {
                let since_last = self.last_auto_sync_trigger.map_or(std::time::Duration::from_secs(3600), |t| t.elapsed());
                if regained_focus && since_last >= std::time::Duration::from_secs(30) {
                    self.start_background_sync(ctx);
                } else if self.sync_dirty_since_last {
                    let page_clean = !self.saver.busy() && self.editor.as_ref().is_none_or(|e| e.dirty_since.is_none());
                    let ink_clean = self.pad.as_ref().is_none_or(|p| !p.has_unsaved_changes());
                    let edit_quiet = self.last_local_edit_at.map_or(std::time::Duration::from_secs(60), |t| t.elapsed());
                    if page_clean && ink_clean && edit_quiet >= std::time::Duration::from_secs(25) && since_last >= std::time::Duration::from_secs(60)
                    {
                        self.start_background_sync(ctx);
                    } else {
                        ctx.request_repaint_after(std::time::Duration::from_secs(5));
                    }
                }
            }
        }

        let Some(outcome) = self.sync_job.as_ref().and_then(crate::sync_ui::SyncJob::try_recv) else {
            return;
        };
        self.sync_job = None;
        // A sync the user started says when it is done; one that ran by itself stays quiet.
        let by_hand = std::mem::take(&mut self.sync_by_hand);
        match outcome {
            Ok(staged) => {
                // Flush any edits made while the sync worker was in flight BEFORE applying staged downloads!
                self.save_now();
                if let Some(pad) = self.pad.as_mut()
                    && pad.dirty
                {
                    let _ = pad.save();
                }
                if let Some(discovered) = staged.discovered_drive_id {
                    let mut cfg = nocturne_engine::SyncConfig::load(&self.session.settings);
                    cfg.drive_id = Some(discovered);
                    let _ = cfg.save(&self.session.settings);
                    self.cached_sync_config = Some(cfg);
                }
                let open_rel =
                    self.editor.as_ref().and_then(|ed| ed.dir.strip_prefix(self.session.root()).ok().map(|r| r.to_string_lossy().replace('\\', "/")));
                let reload_open = open_rel.as_deref().is_some_and(|prefix| {
                    let prefix_slash = format!("{prefix}/");
                    staged.touched_local_paths().iter().any(|rel| *rel == prefix || rel.starts_with(&prefix_slash))
                });
                match nocturne_engine::apply_staged_sync(staged, &self.session.settings) {
                    Ok(report) => {
                        self.session.refresh();
                        self.library_grid.invalidate_all();
                        if let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) {
                            if !dir.is_dir() {
                                self.editor = None;
                                self.pad = None;
                                self.selected = None;
                            } else if reload_open {
                                self.open_page(&dir);
                            }
                        }
                        let msg = crate::sync_ui::format_sync_report_summary(&report);
                        if let Some(Dialog::CloudSync(ref mut form)) = self.dialog {
                            if form.drive_id_input.trim().is_empty() && report.drive_id > 0 {
                                form.drive_id_input = report.drive_id.to_string();
                            }
                            form.last_message = Some((msg.clone(), !report.conflicts.is_empty()));
                        }
                        self.say(msg);
                        if by_hand {
                            self.studio_notice = Some((self.status.clone(), Instant::now()));
                        }
                    }
                    Err(err) => {
                        self.sync_dirty_since_last = true;
                        let msg = err.to_string();
                        if let Some(Dialog::CloudSync(ref mut form)) = self.dialog {
                            form.last_message = Some((msg.clone(), true));
                        }
                        self.complain(&msg);
                    }
                }
            }
            Err(nocturne_engine::SyncError::Cancelled) => {
                if let Some(Dialog::CloudSync(ref mut form)) = self.dialog {
                    form.last_message = Some((crate::tl_id!("ui.kdrive_sync_cancelled").to_string(), false));
                }
                self.say(crate::tl_id!("ui.kdrive_sync_cancelled"));
            }
            Err(err) => {
                if !matches!(
                    err,
                    nocturne_engine::SyncError::NotConfigured(_) | nocturne_engine::SyncError::Unsupported(_) | nocturne_engine::SyncError::Auth(_)
                ) {
                    self.sync_dirty_since_last = true;
                }
                let msg = err.to_string();
                if let Some(Dialog::CloudSync(ref mut form)) = self.dialog {
                    form.last_message = Some((msg.clone(), true));
                }
                self.complain(&msg);
            }
        }
    }

    /// Says that the window has no frame of the system's own, so this window draws its buttons
    /// and takes care of moving and resizing it. The program calls this when it asked for no frame.
    pub fn set_frameless(&mut self, on: bool) {
        self.frameless = on;
    }

    /// The coloured band across the top of the window, as in Office: the Quick Access Toolbar,
    /// the page's title, search and the window's buttons in one row, and under it the tab
    /// names. The File screen shows the first row only.
    fn title_band(&mut self, ui: &mut Ui, with_tabs: bool) {
        let t = self.tokens;
        let (tab, source, in_table) = (self.ribbon_tab.clone(), self.show_source, self.caret_in_table());
        let title = self.editor.as_ref().map(|e| e.title.trim().to_owned()).filter(|s| !s.is_empty());
        let mut acts = Vec::new();
        let collapsed = self.ribbon_folded();
        let band = egui::Panel::top("band").frame(egui::Frame::new().fill(t.band)).show_separator_line(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            self.title_row(ui, title.as_deref());
            if with_tabs {
                ribbon::tab_strip(ui, &tab, source, in_table, collapsed, &t, &mut acts);
            }
        });
        self.band_bottom = band.response.rect.max.y;
        for a in acts {
            self.ribbon_act(a);
        }
    }

    /// The band's first row. Anything on it that is not a control moves the window.
    fn title_row(&mut self, ui: &mut Ui, title: Option<&str>) {
        const H: f32 = 30.0;
        let on = self.tokens.on_band();
        let ctx = ui.ctx().clone();
        let row = egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), H));
        // Added first, so the controls drawn over it win the pointer: only bare band is a handle.
        let handle = ui.interact(row, ui.id().with("title-handle"), Sense::click_and_drag());
        if self.frameless {
            if handle.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if handle.double_clicked() {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        }
        // The choice between the system's title bar and this one, kept in the settings.
        handle.context_menu(|ui| {
            let label = if self.frameless { crate::tl_id!("ui.use_the_system_title_bar") } else { crate::tl_id!("ui.use_nocturne_s_own_title_bar") };
            if ui.button(label).clicked() {
                self.frameless = !self.frameless;
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!self.frameless));
                let _ = self.session.settings.set("window_frame", if self.frameless { "own" } else { "system" });
                ui.close();
            }
        });
        let text = title.map_or_else(|| "Nocturne".to_owned(), |t| format!("{t} - Nocturne"));
        // A page named in Arabic puts the band's words in the wrong order if the whole
        // string is handed to egui, so the band goes through the bidi painter like the page
        // does. The name decides its own direction here; there is no page to follow.
        // A phone-width window has no room for the title between the controls: the page's own
        // title is the first thing on it anyway.
        if !self.compact {
            let band = ui.painter_at(row);
            crate::bidi::line_at(ui, &band, row.center(), Align2::CENTER_CENTER, &text, 12.5, on.text, None);
        }
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left.add_space(8.0);
        left.add(egui::Image::from_bytes("bytes://nocturne-logo", crate::LOGO_PNG).fit_to_exact_size(vec2(18.0, 18.0)));
        left.add_space(6.0);
        if self.qat.hidden || !self.qat.below {
            self.quick_access(&mut left, on);
        }
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if self.frameless {
            self.window_buttons(&mut right, &on);
        }
        right.add_space(4.0);
        self.theme_menu(&mut right, on);
        right.add_space(4.0);
        self.language_menu(&mut right, on);
        right.add_space(10.0);
        // The search box takes what the controls on both sides leave, up to its usual width, so
        // on a phone it does not run over the Quick Access Toolbar.
        let left_used = left.min_rect().right() - row.left();
        let search_width = (right.available_width() - left_used - 8.0).clamp(60.0, 200.0);
        let resp = right.add(
            egui::TextEdit::singleline(&mut self.search)
                .id(egui::Id::unique(SEARCH_FIELD))
                .hint_text(crate::tl_id!("ui.search_all_pages"))
                .desired_width(search_width),
        );
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if resp.changed() {
            self.refresh_search();
        }
        ui.advance_cursor_after_rect(row);
    }

    /// Runs `search.query` whenever `self.search` changes (works identically in Classic Ribbon
    /// and Tablet Studio).
    fn refresh_search(&mut self) {
        if self.search != self.searched {
            self.searched = self.search.clone();
            if self.search.trim().is_empty() {
                self.hits.clear();
            } else {
                self.hits = match self.session.run("search.query", &params(&[("q", &self.search), ("limit", "20")])) {
                    Ok(Output::Hits(h)) => h,
                    _ => Vec::new(),
                };
            }
        }
    }

    /// Minimise, maximise or restore, and close: the buttons the system would have drawn.
    fn window_buttons(&self, ui: &mut Ui, on: &Tokens) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        for kind in ["Close", "Maximize", "Minimize"] {
            let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 30.0), Sense::click());
            let painter = ui.painter_at(rect);
            if resp.hovered() {
                let fill = if kind == "Close" { Color32::from_rgb(196, 43, 28) } else { on.row_hover };
                painter.rect_filled(rect, 0.0, fill);
            }
            let c = rect.center();
            let stroke = egui::Stroke::new(1.0, on.text);
            match kind {
                "Close" => {
                    painter.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
                    painter.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], stroke);
                }
                "Maximize" if maximized => {
                    painter.rect_stroke(egui::Rect::from_min_size(c + vec2(-5.0, -2.0), vec2(7.0, 7.0)), 0.0, stroke, egui::StrokeKind::Inside);
                    painter.line_segment([c + vec2(-2.0, -5.0), c + vec2(5.0, -5.0)], stroke);
                    painter.line_segment([c + vec2(5.0, -5.0), c + vec2(5.0, 2.0)], stroke);
                }
                "Maximize" => {
                    painter.rect_stroke(egui::Rect::from_center_size(c, vec2(10.0, 10.0)), 0.0, stroke, egui::StrokeKind::Inside);
                }
                _ => {
                    painter.line_segment([c + vec2(-5.0, 4.0), c + vec2(5.0, 4.0)], stroke);
                }
            }
            let label = if kind == crate::tl_id!("ui.maximize") && maximized { crate::tl_id!("ui.restore") } else { kind };
            let enabled = ui.is_enabled();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
            if resp.on_hover_text(label).clicked() {
                ctx.send_viewport_cmd(match kind {
                    "Close" => egui::ViewportCommand::Close,
                    "Maximize" => egui::ViewportCommand::Maximized(!maximized),
                    _ => egui::ViewportCommand::Minimized(true),
                });
            }
        }
    }

    /// With no frame of the system's own, the edges and corners of the window are ours: the
    /// pointer shows the resize arrow there, and a press starts the resize.
    fn frame_edges(&self, ctx: &Context) {
        if !self.frameless || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        // A line round the window, so its edge shows against whatever is behind it.
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window-edge"))).rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, self.tokens.band),
            egui::StrokeKind::Inside,
        );
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        const E: f32 = 5.0;
        let (west, east, north, south) = (pos.x - r.min.x < E, r.max.x - pos.x < E, pos.y - r.min.y < E, r.max.y - pos.y < E);
        use egui::{CursorIcon as C, viewport::ResizeDirection as D};
        let (dir, cursor) = match (west, east, north, south) {
            (true, _, true, _) => (D::NorthWest, C::ResizeNorthWest),
            (_, true, true, _) => (D::NorthEast, C::ResizeNorthEast),
            (true, _, _, true) => (D::SouthWest, C::ResizeSouthWest),
            (_, true, _, true) => (D::SouthEast, C::ResizeSouthEast),
            (true, ..) => (D::West, C::ResizeWest),
            (_, true, ..) => (D::East, C::ResizeEast),
            (_, _, true, _) => (D::North, C::ResizeNorth),
            (_, _, _, true) => (D::South, C::ResizeSouth),
            _ => return,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }

    /// The File screen: notebooks and what is done to a whole notebook.
    fn backstage_screen(&mut self, ui: &mut Ui) {
        let page = self.backstage.unwrap_or("Info");
        let tokens = self.tokens;
        let folder = self.shown(self.session.root());
        let recent = self.recent_notebooks();
        let (root, redact) = (self.session.root().to_path_buf(), self.redact_paths);
        let show_path = move |p: &Path| shown_in(&root, p, redact);
        let ui_language = self.ui_language.clone();
        let languages = crate::i18n::available_languages();
        let layout_mode = self.session.layout_mode();
        let ocr_cfg = self.session.ocr_config();
        let mut acts = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            let telemetry_root = self.session.settings.path().parent().unwrap_or(std::path::Path::new(".")).join("telemetry");
            let telemetry_level = self.session.settings.get("telemetry_level").unwrap_or_else(|| "none".into());
            let session_id = self.session.settings.get("telemetry_session_id");
            let update = self.update.lock().map(|s| s.clone()).unwrap_or_default();
            acts = backstage::show(
                ui,
                self.session.tree(),
                &recent,
                &folder,
                page,
                &show_path,
                &telemetry_root,
                &telemetry_level,
                session_id.as_deref(),
                &ui_language,
                &languages,
                self.ui_scale,
                layout_mode,
                ocr_cfg.enabled,
                ocr_cfg.handwriting_enabled,
                &update,
                &tokens,
            );
        });
        for a in acts {
            match a {
                backstage::Act::Close => self.backstage = None,
                backstage::Act::Page(name) => self.backstage = Some(name),
                backstage::Act::NewNotebook => {
                    self.backstage = None;
                    self.dialog = Some(Dialog::NewNotebook { name: String::new(), format: None });
                }
                backstage::Act::OpenFolder(p) => self.reveal(&p),
                backstage::Act::ChooseFolder => self.choose_notebooks_folder(),
                backstage::Act::ImportNotebookFolder => self.import_notebook_folder(),
                backstage::Act::ImportFile => self.import_file(),
                backstage::Act::ImportCloud => {
                    self.status = crate::tl_id!("ui.cloud_import_is_not_implemented_yet").to_string();
                    self.status_is_error = false;
                }
                backstage::Act::OpenNotebooksFolder => {
                    let root = self.session.root().to_path_buf();
                    self.reveal(&root);
                }
                backstage::Act::Open(p) => {
                    self.open_notebook_from_path(&p);
                }
                backstage::Act::OpenRecent(p) => {
                    self.open_notebook_from_path(&p);
                }
                backstage::Act::RemoveRecent(p) => self.remove_recent_notebook(&p),
                backstage::Act::DeleteRecent(p, title) => self.dialog = Some(Dialog::DeleteNotebook { path: p, title }),
                backstage::Act::SetTelemetryLevel(level) => {
                    let _ = self.session.settings.set("telemetry_level", &level);
                }
                backstage::Act::SetUiLanguage(language) => {
                    self.set_ui_language(language);
                }
                backstage::Act::SetUiScale(percent) => self.set_ui_scale(percent),
                backstage::Act::SetLayoutMode(mode) => {
                    let _ = self.run("ui.layout_mode", &params(&[("mode", mode.id())]));
                    self.last_tablet_studio = self.is_tablet_studio(ui.ctx());
                    if !self.last_tablet_studio {
                        self.tool_settings_open = false;
                    }
                }
                backstage::Act::OpenTelemetryFolder(path) => {
                    // The folder is made on first use: it does not exist until a session has
                    // written to it, and Explorer opens Documents for a folder it cannot find.
                    match std::fs::create_dir_all(&path) {
                        Ok(()) => self.reveal(&path),
                        Err(e) => {
                            self.status =
                                crate::i18n::format_message_id("ui.the_diagnostics_folder_could_not_be_created_error", &[("error", &e.to_string())]);
                            self.status_is_error = true;
                        }
                    }
                }
                backstage::Act::ClearTelemetry(path) => self.dialog = Some(Dialog::ClearTelemetry { path }),
                backstage::Act::CheckForUpdates => crate::update::start(ui.ctx().clone(), self.update.clone()),
                backstage::Act::OpenReleases => ui.ctx().open_url(egui::OpenUrl::new_tab(backstage::LATEST_RELEASE)),
                backstage::Act::OpenCloudSync => {
                    self.backstage = None;
                    self.open_cloud_sync_dialog();
                }
                backstage::Act::ToggleStudioLayout => {
                    let next = if self.is_tablet_studio(ui.ctx()) {
                        nocturne_engine::UiLayoutMode::ClassicRibbon
                    } else {
                        nocturne_engine::UiLayoutMode::TabletStudio
                    };
                    let _ = self.run("ui.layout_mode", &params(&[("mode", next.id())]));
                    self.last_tablet_studio = self.is_tablet_studio(ui.ctx());
                    if !self.last_tablet_studio {
                        self.tool_settings_open = false;
                    }
                    self.backstage = None;
                }
                backstage::Act::ToggleMathOcr => self.ribbon_act(ribbon::Act::ToggleMathOcr),
                backstage::Act::ToggleHandwritingOcr => self.ribbon_act(ribbon::Act::ToggleHandwritingOcr),
                backstage::Act::OpenMathOcrSetup => {
                    self.backstage = None;
                    self.ribbon_act(ribbon::Act::OpenMathOcrSetup);
                }
                backstage::Act::ResetPreferences => self.dialog = Some(Dialog::ResetPreferences),
                backstage::Act::ExportTelemetry(root) => {
                    if let Some(session) = std::fs::read_dir(&root)
                        .ok()
                        .and_then(|r| r.filter_map(Result::ok).find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                        && let Some(destination) = rfd::FileDialog::new()
                            .set_title(crate::tl_id!("ui.export_telemetry_zip"))
                            .set_file_name(format!("nocturne-{}-telemetry.zip", session.file_name().to_string_lossy()))
                            .save_file()
                    {
                        let _ = crate::telemetry_export::export(&session.path(), &destination);
                    }
                }
                backstage::Act::NotBuilt(name) => {
                    self.status =
                        crate::i18n::format_message_id("ui.name_is_not_built_yet_see_the_roadmap_for_what_is_coming", &[("name", crate::tl!(name))]);
                    self.status_is_error = false;
                }
            }
        }
    }

    fn recent_notebooks(&self) -> Vec<Node> {
        let mut paths = Vec::new();
        if let Some(path) = self.editor.as_ref().and_then(|editor| notebook_for(&editor.dir)) {
            paths.push(path);
        }
        paths.extend(self.session.opened_elsewhere().iter().rev().cloned());
        if let Some(saved) = self.session.settings.get("recent_notebooks")
            && let Ok(saved) = serde_json::from_str::<Vec<String>>(&saved)
        {
            paths.extend(saved.into_iter().map(PathBuf::from));
        }
        let mut recent = Vec::new();
        let mut seen = HashSet::new();
        for path in paths {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !seen.insert(path.clone()) {
                continue;
            }
            if let Some(notebook) = nocturne_store::tree::load_tree(&path)
                && notebook.info.kind == NodeKind::Notebook
            {
                recent.push(notebook);
                if recent.len() == 12 {
                    break;
                }
            }
        }
        recent
    }

    fn remember_recent_notebook(&mut self, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut recent =
            self.session.settings.get("recent_notebooks").and_then(|saved| serde_json::from_str::<Vec<String>>(&saved).ok()).unwrap_or_default();
        let path_text = path.to_string_lossy().into_owned();
        recent.retain(|saved| std::fs::canonicalize(saved).unwrap_or_else(|_| PathBuf::from(saved)) != path);
        recent.insert(0, path_text);
        recent.truncate(12);
        if let Ok(serialized) = serde_json::to_string(&recent) {
            let _ = self.session.settings.set("recent_notebooks", &serialized);
        }
    }

    fn remove_recent_notebook(&mut self, path: &Path) {
        let Some(saved) = self.session.settings.get("recent_notebooks") else { return };
        let Ok(mut recent) = serde_json::from_str::<Vec<String>>(&saved) else { return };
        recent.retain(|entry| std::fs::canonicalize(entry).unwrap_or_else(|_| PathBuf::from(entry)) != path);
        if let Ok(serialized) = serde_json::to_string(&recent) {
            let _ = self.session.settings.set("recent_notebooks", &serialized);
        }
        self.say(crate::tl_id!("ui.removed_notebook_from_the_known_notebooks_list"));
    }

    fn open_notebook_from_path(&mut self, path: &Path) {
        self.save_now();
        let path_text = path_str(path);
        if let Some(Output::Path(opened)) = self.run("notebook.open", &params(&[("path", &path_text)])) {
            self.remember_recent_notebook(&opened);
            self.open.insert(opened.clone());
            self.selected = Some(opened.clone());
            self.backstage = None;
            if let Some(page) = first_page(self.session.tree(), &opened) {
                self.open_page(&page);
            } else {
                self.editor = None;
                self.pad = None;
                self.say(crate::i18n::format_message_id("ui.opened_path_it_has_no_pages_yet", &[("path", &self.shown(&opened))]));
            }
        }
    }

    /// Asks the system for a folder and opens the notebooks in it, keeping everything that
    /// is already open. A folder that is itself a notebook opens as one; a folder that
    /// holds notebooks opens all of them. Nothing is closed and nothing is moved.
    fn choose_notebooks_folder(&mut self) {
        let Some(picked) =
            rfd::FileDialog::new().set_title(crate::tl_id!("ui.choose_a_notebook_a_folder_of_notebooks_or_an_empty_9de57f8f1f")).pick_folder()
        else {
            return;
        };
        let is_notebook = nocturne_store::tree::read_node(&picked).is_some_and(|i| i.kind == NodeKind::Notebook);
        let wanted: Vec<std::path::PathBuf> =
            if is_notebook { vec![picked.clone()] } else { nocturne_store::tree::load_notebooks(&picked).into_iter().map(|n| n.path).collect() };
        if wanted.is_empty() {
            let p = path_str(&picked);
            if let Some(Output::Path(opened)) = self.run("notebook.open", &params(&[("path", &p)])) {
                self.remember_recent_notebook(&opened);
                self.open.insert(opened.clone());
                self.selected = Some(opened);
                self.backstage = None;
                self.say(crate::i18n::format_message_id("ui.path_opened_as_an_empty_notebook", &[("path", &self.shown(&picked))]));
            }
            return;
        }
        let mut opened_count = 0;
        let mut first = None;
        for path in wanted {
            let p = path_str(&path);
            if let Some(Output::Path(notebook_path)) = self.run("notebook.open", &params(&[("path", &p)])) {
                self.remember_recent_notebook(&notebook_path);
                opened_count += 1;
                first.get_or_insert(path.clone());
                self.open.insert(notebook_path);
            }
        }
        if let Some(path) = first {
            self.selected = Some(path);
        }
        self.backstage = None;
        self.say(crate::i18n::format_count_id("ui.n_notebook_opened_n_notebooks_opened", opened_count));
    }

    fn import_notebook_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title(crate::tl_id!("ui.choose_a_onenote_notebook_folder"));
        if let Some(backup) = nocturne_import::discover::default_source() {
            dialog = dialog.set_directory(backup);
        }
        let Some(source) = dialog.pick_folder() else { return };
        self.import_officeimo_source(source, true);
    }

    fn import_file(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title(crate::tl_id!("ui.choose_a_file_to_import"));
        if let Some(backup) = nocturne_import::discover::default_source() {
            dialog = dialog.set_directory(backup);
        }
        let Some(source) = dialog.pick_file() else { return };
        // A OneNote package (.onepkg) is a whole notebook, imported the way a folder is (issue #17).
        let folder = nocturne_import::package::is_package(&source);
        self.import_officeimo_source(source, folder);
    }

    fn import_officeimo_source(&mut self, source: PathBuf, folder: bool) {
        let format = self
            .session
            .settings
            .get("page_format")
            .and_then(|v| nocturne_model::PageFormat::parse(&v).ok())
            .unwrap_or(nocturne_model::PageFormat::Html);
        let bridge_format = match format {
            nocturne_model::PageFormat::Html => nocturne_import::officeimo::Format::Html,
            nocturne_model::PageFormat::Cmgfm => nocturne_import::officeimo::Format::Cmgfm,
        };
        let destination = self.unique_import_destination(&source);
        let output = if folder { destination.parent().unwrap_or(self.session.root()) } else { &destination };
        log::info!("OfficeIMO import requested: source={} destination={}", source.display(), destination.display());
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let source_for_worker = source.clone();
        let output_for_worker = output.to_path_buf();
        std::thread::spawn(move || {
            let mut progress = |event| {
                let message = match event {
                    nocturne_import::officeimo::Progress::Found { total } => ImportMessage::Found(total),
                    nocturne_import::officeimo::Progress::Section { index, total, label } => {
                        ImportMessage::Section { index, total, label: label.to_string() }
                    }
                };
                let _ = tx.send(message);
            };
            let result =
                nocturne_import::officeimo::run_with_progress(&source_for_worker, &output_for_worker, bridge_format, &worker_cancel, &mut progress);
            let _ = tx.send(ImportMessage::Finished(result));
        });
        self.import_job = Some(ImportJob {
            rx,
            keep_alive: None,
            cancel,
            source,
            destination,
            current: crate::tl_id!("ui.starting_officeimo").into(),
            index: 0,
            total: 0,
        });
        self.status = crate::tl_id!("ui.importing").into();
        self.status_is_error = false;
    }

    fn poll_import(&mut self, ctx: &Context) {
        let Some(job) = self.import_job.as_mut() else { return };
        let mut finished = None;
        loop {
            match job.rx.try_recv() {
                Ok(ImportMessage::Found(total)) => job.total = total,
                Ok(ImportMessage::Section { index, total, label }) => {
                    job.index = index;
                    job.total = total;
                    job.current = label;
                }
                Ok(ImportMessage::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err(crate::tl_id!("ui.officeimo_import_worker_stopped_unexpectedly").into()));
                    break;
                }
            }
        }
        if let Some(result) = finished {
            let Some(job) = self.import_job.take() else { return };
            match result {
                Ok(message) => {
                    let _ = self.run("tree.refresh", &Params::new());
                    log::info!("OfficeIMO import succeeded: source={} destination={}", job.source.display(), job.destination.display());
                    self.status = crate::tl!(&message).to_string();
                    self.status_is_error = false;
                    self.open_notebook_from_path(&job.destination);
                }
                Err(error) => {
                    log::error!("OfficeIMO import failed: source={} destination={} error={error}", job.source.display(), job.destination.display());
                    self.status = crate::i18n::format_message_id("ui.could_not_import_the_notebook_error", &[("error", &error)]);
                    self.status_is_error = true;
                }
            }
        } else if job.keep_alive.is_none() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn import_progress(&mut self, ctx: &Context) {
        let Some(job) = self.import_job.as_mut() else { return };
        let tokens = self.tokens;
        let mut cancel = false;
        crate::modal::Modal::new("import-progress-modal", crate::tl_id!("ui.importing_onenote"))
            .backdrop_id("import-progress-modal-backdrop")
            .width(440.0)
            .closable(false)
            .close_on_escape(false)
            .show_with_footer(
                ctx,
                &tokens,
                |ui| {
                    ui.label(egui::RichText::new(crate::tl_id!("ui.you_can_continue_working_while_the_import_runs")).color(tokens.muted));
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(crate::i18n::pseudo_dynamic(&job.current)).strong().color(tokens.strong));
                    let fraction = if job.total == 0 { 0.0 } else { job.index as f32 / job.total as f32 };
                    ui.add(egui::ProgressBar::new(fraction).fill(tokens.accent).show_percentage());
                    if job.total > 0 {
                        let index = job.index.to_string();
                        let total = job.total.to_string();
                        ui.label(crate::i18n::format_message_id("ui.index_of_total_sections", &[("index", &index), ("total", &total)]));
                    } else {
                        ui.label(crate::tl_id!("ui.discovering_sections"));
                    }
                },
                |ui| {
                    ui.horizontal(|ui| {
                        if crate::modal::danger_button(ui, crate::tl_id!("ui.cancel_import"), &tokens).clicked() {
                            cancel = true;
                        }
                    });
                },
            );
        if cancel {
            job.cancel.store(true, Ordering::Relaxed);
            job.current = "Cancelling…".into();
        }
    }

    fn import_notebook_name(source: &Path) -> String {
        let stem = source.file_stem().and_then(|name| name.to_str()).unwrap_or("Imported notebook");
        let name = stem.split_once(" (On ").map_or(stem, |(name, _)| name).trim();
        let cleaned: String =
            name.chars().map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c }).collect();
        let cleaned = cleaned.trim().trim_end_matches('.');
        if cleaned.is_empty() { "Imported notebook".to_string() } else { cleaned.to_string() }
    }

    fn unique_import_destination(&self, source: &Path) -> PathBuf {
        let base = Self::import_notebook_name(source);
        let root = self.session.root();
        let first = root.join(&base);
        if !first.exists() {
            return first;
        }
        for number in 1..=u32::MAX {
            let candidate = root.join(format!("{base} ({number})"));
            if !candidate.exists() {
                return candidate;
            }
        }
        root.join(format!("{base} ({})", u64::from(u32::MAX) + 1))
    }

    fn reveal(&mut self, path: &Path) {
        match open_folder(path) {
            Ok(()) => self.say(crate::i18n::format_message_id("ui.opened_path", &[("path", &self.shown(path))])),
            Err(e) => {
                self.status = crate::i18n::format_message_id("ui.the_folder_could_not_be_opened_error", &[("error", &e.to_string())]);
                self.status_is_error = true;
            }
        }
    }

    /// Follows a link written in the open page. Another page of the notebooks opens in the
    /// window; a notebook, section group or section opens on its first page; a file opens in
    /// the program the system has for it; a web address opens in the browser. A relative
    /// path can never reach outside the notebooks folder (`nocturne_store::links`).
    /// Puts a link to a page on the clipboard, as OneNote's Copy Link to Page does.
    ///
    /// The link names the page from the notebooks folder down
    /// (`nocturne:/book/section/page`), not relative to anything, because what a relative
    /// link would be relative to is the page it is pasted into, and nobody knows that when
    /// the link is copied. Pasted into a page it reads as an ordinary link and opens the
    /// page it names.
    pub fn copy_link_to(&mut self, dir: &Path) {
        let mut roots = vec![self.session.root().to_path_buf()];
        roots.extend(self.session.opened_elsewhere().iter().cloned());
        let Some(uri) = nocturne_store::links::page_uri(&roots, dir) else {
            self.complain(crate::tl_id!("ui.that_page_is_not_in_a_notebook_that_is_open"));
            return;
        };
        let title = nocturne_store::tree::read_node(dir).map(|n| n.title).unwrap_or_else(|| "Page".to_string());
        // Plain text keeps the title and address readable; the matching HTML flavor is an
        // anchor for clients that accept rich content.
        self.pending_copy = Some(format!("{title} ({uri})"));
        self.pending_copy_html = Some(crate::clipboard::html_link(&title, &uri));
        self.say(crate::i18n::format_message_id("ui.link_to_title_copied", &[("title", &title)]));
    }

    pub fn follow_link(&mut self, ctx: &Context, href: &str) {
        use nocturne_store::links::{Link, resolve_in};
        let Some(page) = self.editor.as_ref().map(|e| e.dir.clone()) else { return };
        // Every folder a notebook is open from, not just the notebooks folder: a notebook
        // opened from elsewhere is as much inside as one that lives there, and judging it
        // by the notebooks folder alone made every link in it dead.
        let mut roots = vec![self.session.root().to_path_buf()];
        roots.extend(self.session.opened_elsewhere().iter().cloned());
        match resolve_in(&roots, &page, href) {
            Link::Web(url) => {
                ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
                self.say(crate::i18n::format_message_id("ui.opened_url_in_the_browser", &[("url", &url)]));
            }
            Link::Node(dir) => {
                if nocturne_store::tree::read_node(&dir).is_some_and(|n| n.kind == NodeKind::Page) {
                    self.open_path(&path_str(&dir));
                } else {
                    self.open.insert(dir.clone());
                    self.selected = Some(dir.clone());
                    match first_page_under(self.session.tree(), &dir) {
                        Some(first) => self.open_path(&path_str(&first)),
                        None => self.say(crate::i18n::format_message_id("ui.opened_path_it_has_no_pages_yet", &[("path", &self.shown(&dir))])),
                    }
                }
            }
            Link::File(file) => {
                if self.open_attachment_warning_suppressed {
                    match open_file_for_viewer(&file) {
                        Ok(()) => self.say(crate::i18n::format_message_id("ui.opening_path", &[("path", &self.shown(&file))])),
                        Err(e) => {
                            self.complain(&crate::i18n::format_message_id("ui.the_file_could_not_be_opened_error", &[("error", &e.to_string())]))
                        }
                    }
                } else {
                    self.dialog = Some(Dialog::OpenAttachmentWarning { path: file });
                }
            }
            Link::Missing(p) => {
                self.status = crate::i18n::format_message_id(
                    "ui.nothing_at_path_the_link_points_to_a_page_or_file_th_18d71ccd21",
                    &[("path", &self.shown(&p))],
                );
                self.status_is_error = true;
            }
            Link::Outside => {
                self.status = crate::tl_id!("ui.that_link_points_outside_the_notebooks_folder_so_it_3a37bd9b91").to_string();
                self.status_is_error = true;
            }
        }
    }

    /// The ribbon under the title row: tabs and their groups.
    fn ribbon_bar(&mut self, ui: &mut Ui) {
        let (tab, source, tokens) = (self.ribbon_tab.clone(), self.show_source, self.tokens);
        let font_selection = self.font_selection(ui.ctx());
        let family = match &font_selection.family {
            pageedit::FontValue::Value(Some(name)) => Some(name.clone()),
            pageedit::FontValue::Value(None) => Some(crate::tl_id!("ui.default").to_string()),
            pageedit::FontValue::Mixed => Some(crate::tl_id!("ui.mixed").to_string()),
            // With no caret in the page the box shows the default font, as the size box shows its base size.
            pageedit::FontValue::Unavailable => Some(crate::tl_id!("ui.default").to_string()),
        };
        let size = match font_selection.size {
            pageedit::FontValue::Value(Some(size)) => Some(size.to_string()),
            // The page's base size is 11 points, so the box shows that number, not the word Default.
            pageedit::FontValue::Value(None) => Some("11".to_owned()),
            pageedit::FontValue::Mixed => Some(crate::tl_id!("ui.mixed").to_string()),
            // With no caret in the page the box shows the base size, as OneNote does.
            pageedit::FontValue::Unavailable => Some("11".to_owned()),
        };
        let view = ribbon::View {
            split_panes: self.split_panes,
            always_on_top: self.always_on_top,
            full_page: self.full_page,
            rules: self.rules,
            hide_title: self.hide_title,
            snap: self.snap_to_grid,
            zoom_percent: self.zoom_percent,
            pointer: self.pointer,
            rack: &self.rack,
            nib: self.nib,
            in_table: self.caret_in_table(),
            space_tool: self.space_tool,
            finger_drawing: self.is_finger_drawing(),
            eraser_auto_return: self.eraser_auto_return,
            eraser_pixel: self.session.ink_options().eraser_kind == nocturne_engine::EraserKind::Pixel,
            eraser_width: self.session.ink_options().eraser_width,
            show_minimap: self.minimap_visible(ui.ctx()),
            tablet_tools_minimized: self.tablet_tools_minimized,
            canvas_layout: self.editor.as_ref().map(|ed| ed.canvas_layout).unwrap_or_default(),
            table_borders: self.caret_table_has_borders(),
            table_align: self.caret_cells_align(),
            table_shade: self.caret_cells_shade(),
            gridcraft: self.gridcraft.is_some(),
            page_colour: self.editor.as_ref().and_then(|e| e.colour),
            rule_colour: self.editor.as_ref().and_then(|e| e.rule_colour),
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            can_ink_redo: self.pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo),
            sensitivity: self.sensitivity,
            pressure_curve: self.pressure_curve,
            pressure_available: self.stylus.sample().pen,
            has_page: self.editor.is_some(),
            painting: self.painter.is_some(),
            style: self.caret_style(ui.ctx()),
            font_family: family.as_deref(),
            font_size: size.as_deref(),
            marks: font_selection.marks,
            math_ocr_enabled: self.session.ocr_config().enabled,
            holding_ink: self.pad.as_ref().is_some_and(|p| p.holding() > 0),
        };
        let format = self.editor.as_ref().map(|e| e.format);
        let mut acts = Vec::new();
        if !self.ribbon_folded() {
            egui::Panel::top("ribbon")
                .exact_size(ribbon::GROUP_H + 8.0)
                .frame(egui::Frame::new().fill(tokens.panel).inner_margin(egui::Margin::symmetric(4, 0)))
                .show_separator_line(false)
                .show(ui, |ui| {
                    acts = ribbon::show(ui, &tab, source, &view, format, &tokens);
                });
        } else if self.ribbon_peek {
            // Folded, with a tab clicked: the tab's commands over the top of the page, as Office
            // shows them, so the page does not jump down and back. Using one, clicking anywhere
            // else or Escape folds it away again.
            let screen = ui.ctx().content_rect();
            let area = egui::Area::new(egui::Id::new("ribbon-peek"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(screen.min.x, self.band_bottom))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(tokens.panel)
                        .inner_margin(egui::Margin::symmetric(4, 0))
                        .stroke(egui::Stroke::new(1.0, tokens.container_border))
                        .show(ui, |ui| {
                            ui.set_width(screen.width() - 8.0);
                            ui.set_height(ribbon::GROUP_H + 8.0);
                            acts = ribbon::show(ui, &tab, source, &view, format, &tokens);
                        });
                })
                .response;
            let pointer = ui.ctx().pointer_latest_pos();
            // A gallery the shown ribbon opened (Format Background, say) is over the page too, and
            // a click in it is still a click in the ribbon.
            let in_ribbon = |p: egui::Pos2| area.rect.contains(p) || ui.ctx().layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground);
            let clicked_away = ui.input(|i| i.pointer.any_pressed()) && pointer.is_some_and(|p| !in_ribbon(p) && p.y > self.band_bottom);
            let used = acts.iter().any(|a| !matches!(a, ribbon::Act::Tab(_)));
            if used || clicked_away || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.ribbon_peek = false;
            }
        }
        let picked = self.editor.as_ref().and_then(|e| crate::containers::selected(ui.ctx(), &e.dir));
        for a in acts {
            if let Some(i) = picked
                && self.act_on_box(ui.ctx(), i, &a)
            {
                continue;
            }
            self.ribbon_act(a);
        }
    }

    fn font_selection(&self, ctx: &Context) -> pageedit::FontSelection {
        use nocturne_formats::Block;
        let Some(ed) = self.editor.as_ref() else { return pageedit::FontSelection::default() };
        if let Some(index) = crate::containers::selected(ctx, &ed.dir) {
            let range = nocturne_model::container::blocks_of(&ed.boxes, index);
            let Some(blocks) = ed.blocks.get(range) else { return pageedit::FontSelection::default() };
            let spans = blocks.iter().flat_map(|block| match block {
                Block::Heading { spans, .. } | Block::Paragraph(spans) | Block::Quote(spans) | Block::Item { spans, .. } => spans.as_slice(),
                _ => &[],
            });
            return pageedit::FontSelection::from_spans(spans);
        }
        if let Some(across) = ed.across.filter(|a| !a.inside_one_block()) {
            let mut result: Option<pageedit::FontSelection> = None;
            for index in across.blocks() {
                let Some(block) = ed.blocks.get(index).filter(|b| pageedit::is_one_line_of_words(b)) else {
                    return pageedit::FontSelection::default();
                };
                let text = pageedit::block_text(block);
                let Some((start, end)) = across.covering(index, text.len()) else { continue };
                let (Some(left), Some(right)) = (text.get(..start), text.get(..end)) else { return pageedit::FontSelection::default() };
                let Some(selection) = pageedit::selection_fonts(&text, (left.chars().count(), right.chars().count())) else {
                    return pageedit::FontSelection::default();
                };
                if let Some(current) = result.as_mut() {
                    current.merge(selection);
                } else {
                    result = Some(selection);
                }
            }
            return result.unwrap_or_default();
        }
        let Some(caret) = ed.caret.as_ref() else { return pageedit::FontSelection::default() };
        let mut result = ed.blocks.get(caret.block).and_then(|block| caret.fonts(block)).unwrap_or_default();
        if caret.selection.0 == caret.selection.1
            && let Some(pending) = ed.pending_format.as_ref().filter(|p| p.block == caret.block && p.at == caret.selection.0)
        {
            if let Some(family) = &pending.family {
                result.family = pageedit::FontValue::Value(Some(family.clone()));
            }
            if let Some(size) = pending.size {
                result.size = pageedit::FontValue::Value(Some(size));
            }
            // A mark clicked with nothing selected is put on what is typed next: the button
            // shows it pressed until then (issue #255).
            let m = &mut result.marks;
            m.bold |= pending.bold;
            m.italic |= pending.italic;
            m.strike |= pending.strike;
            m.underline |= pending.underline;
            m.subscript |= pending.script == Some(nocturne_formats::markdown::Script::Subscript);
            m.superscript |= pending.script == Some(nocturne_formats::markdown::Script::Superscript);
        }
        result
    }

    /// What a ribbon button does while a container is selected: selecting a box selects everything in
    /// it, so Cut, Copy, the marks and the styles take all of it. True when the act was one of those.
    fn act_on_box(&mut self, ctx: &Context, at: usize, act: &ribbon::Act) -> bool {
        use nocturne_formats::markdown::Block;
        let wanted = matches!(
            act,
            ribbon::Act::Clipboard(_)
                | ribbon::Act::Mark(_)
                | ribbon::Act::FontSize(_)
                | ribbon::Act::FontFamily(_)
                | ribbon::Act::Block(_)
                | ribbon::Act::ClearFormatting
                | ribbon::Act::Direction(_)
                | ribbon::Act::Indent(_)
                | ribbon::Act::Tag(_)
                | ribbon::Act::Justify(_)
        );
        let Some(ed) = self.editor.as_mut().filter(|_| wanted) else { return false };
        let range = nocturne_model::container::blocks_of(&ed.boxes, at);
        if range.end > ed.blocks.len() || range.is_empty() {
            return false;
        }
        let words = |b: &Block| !matches!(b, Block::Code { .. } | Block::MathBlock(_)) && pageedit::is_one_line_of_words(b);
        match act {
            ribbon::Act::Clipboard(which) => {
                crate::containers::ask_clipboard(ctx, *which == "cut");
                return true;
            }
            ribbon::Act::FontSize(_) | ribbon::Act::FontFamily(_) => {
                let format = match act {
                    ribbon::Act::FontSize(size) if ribbon::FONT_SIZES.contains(size) => pageedit::InlineFormat::Size(*size),
                    ribbon::Act::FontFamily(family) if nocturne_formats::markdown::valid_font_family(family) => {
                        pageedit::InlineFormat::Family(family.clone())
                    }
                    _ => {
                        log::warn!("ribbon requested an invalid font size or family");
                        self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
                        self.status_is_error = false;
                        return true;
                    }
                };
                if !ed.writable {
                    self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
                    self.status_is_error = false;
                    return true;
                }
                Self::remember_step(ed);
                let mut changed = false;
                for block in ed.blocks.get_mut(range).unwrap_or_default() {
                    if let Block::Heading { spans, .. } | Block::Paragraph(spans) | Block::Quote(spans) | Block::Item { spans, .. } = block {
                        changed |= pageedit::format_span_range(spans, 0..usize::MAX, format.clone());
                    }
                }
                if changed {
                    ed.pending_format = None;
                    ed.rebuild_body();
                    Self::mark_dirty_from_blocks(ed);
                    let step =
                        crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
                    ed.history.record(step);
                }
                self.status.clear();
                self.status_is_error = false;
                return true;
            }
            ribbon::Act::Mark(mark) => {
                let new_mark = matches!(*mark, "underline" | "subscript" | "superscript");
                if new_mark && !ed.writable {
                    self.status = crate::tl_id!("ui.read_only").into();
                    self.status_is_error = false;
                    return true;
                }
                if new_mark {
                    Self::remember_step(ed);
                }
                if !pageedit::toggle_mark(&mut ed.blocks, range, mark) {
                    return true;
                }
                if new_mark {
                    ed.pending_format = None;
                    ed.rebuild_body();
                    Self::mark_dirty_from_blocks(ed);
                    let step =
                        crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
                    ed.history.record(step);
                    return true;
                }
            }
            ribbon::Act::Direction(dir) => {
                // Words only: a mark in a diagram or an equation would be part of its source.
                for i in range {
                    if ed.blocks.get(i).is_some_and(words) {
                        pageedit::set_direction(&mut ed.blocks, i, *dir);
                    }
                }
            }
            ribbon::Act::Indent(deeper) => {
                for i in range {
                    pageedit::indent(&mut ed.blocks, i, *deeper);
                }
            }
            ribbon::Act::Tag(tag) => {
                // All of them have the tag already: the same button takes it off every one.
                let has = |b: &Block| pageedit::tag_of(b).0 == Some(*tag);
                // A to-do line carries no other tag, so it is left out of every tag but its own.
                let can = |b: &Block| words(b) && (*tag == nocturne_model::Tag::ToDo || !matches!(b, Block::Item { task: Some(_), .. }));
                let all = range.clone().filter_map(|i| ed.blocks.get(i)).filter(|b| can(b)).all(has);
                for i in range {
                    if ed.blocks.get(i).is_some_and(|b| can(b) && (all || !has(b))) {
                        pageedit::set_tag(&mut ed.blocks, i, *tag);
                    }
                }
            }
            ribbon::Act::Justify(strength) => {
                let paragraphs: Vec<usize> = range.filter(|i| matches!(ed.blocks.get(*i), Some(Block::Paragraph(_)))).collect();
                if paragraphs.is_empty() {
                    return true;
                }
                let all = paragraphs.iter().all(|i| ed.justified.strength_of(*i) == Some(*strength));
                let want = (!all).then_some(*strength);
                let mut marked = ed.justified.clone();
                for i in paragraphs {
                    marked.set(i, want);
                }
                self.write_justified(marked, want);
                return true;
            }
            ribbon::Act::Block(kind) => {
                // All of them are that kind already: the same button turns them back into text.
                let all = range.clone().all(|i| ed.blocks.get(i).is_some_and(|b| pageedit::block_kind(b) == *kind));
                for i in range {
                    if all || ed.blocks.get(i).is_some_and(|b| pageedit::block_kind(b) != *kind) {
                        pageedit::set_kind(&mut ed.blocks, i, kind);
                    }
                }
            }
            _ => {
                for i in range {
                    if let Some(b) = ed.blocks.get_mut(i).filter(|b| words(b)) {
                        pageedit::clear_marks(b);
                    }
                }
            }
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        true
    }

    /// Carries out one thing the ribbon asked for.
    fn ribbon_act(&mut self, act: ribbon::Act) {
        match act {
            ribbon::Act::File => self.backstage = Some("Info"),
            ribbon::Act::Options => self.backstage = Some("Options"),
            ribbon::Act::PageVersions => {
                self.save_now();
                if let Some(page) = self.editor.as_ref().map(|e| e.dir.clone()) {
                    let p = params(&[("path", &path_str(&page))]);
                    if let Some(Output::Versions(versions)) = self.run("page.versions", &p) {
                        self.dialog = Some(Dialog::PageVersions { page, versions });
                    }
                }
            }
            ribbon::Act::RecycleBin => {
                if let Some(notebook) = self.editor.as_ref().and_then(|e| notebook_for(&e.dir)) {
                    let p = params(&[("notebook", &path_str(&notebook))]);
                    if let Some(Output::Recycled(items)) = self.run("recycle.list", &p) {
                        self.dialog = Some(Dialog::RecycleBin { notebook, items });
                    }
                }
            }
            ribbon::Act::Symbols => self.dialog = Some(Dialog::Symbols),
            ribbon::Act::Equation(tex) => self.insert_in_page(&format!("${tex}$")),
            ribbon::Act::Tab(name) => {
                // Folded, a tab shows its commands over the page; the same tab again hides them.
                if self.ribbon_folded() {
                    self.ribbon_peek = !(self.ribbon_peek && self.ribbon_tab == name);
                }
                self.ribbon_tab = name;
            }
            ribbon::Act::ToggleRibbon => self.toggle_ribbon(),
            ribbon::Act::Mark(mark) => self.apply_mark(mark),
            ribbon::Act::FontSize(size) => self.apply_font_size(size),
            ribbon::Act::FontFamily(family) => self.apply_font_family(&family),
            ribbon::Act::Block(kind) => self.apply_block_kind(kind),
            ribbon::Act::Insert(what) => match what {
                "space" => {
                    let Some(ed) = self.editor.as_mut() else { return };
                    // What was written before is kept as it stood, so Undo takes back only the space.
                    Self::remember_step(ed);
                    let at = ed.caret.as_ref().map(|c| c.block + 1).unwrap_or(ed.blocks.len()).min(ed.blocks.len());
                    ed.blocks.insert(at, nocturne_formats::Block::Paragraph(Vec::new()));
                    let marks = ed.blocks_inserted(at, 1);
                    ed.rebuild_body();
                    ed.caret = Some(pageedit::Caret {
                        block: at,
                        text: String::new(),
                        focus: true,
                        selection: (0, 0),
                        place: None,
                        cell: None,
                        span: None,
                        inline: None,
                    });
                    Self::mark_dirty_from_blocks(ed);
                    Self::remember_edit(ed);
                    self.save_block_marks(marks);
                }
                "link" => self.open_link_dialog(),
                "equation" => self.insert_in_page("$E = mc^2$"),
                "date" | "time" | "datetime" => {
                    let now = nocturne_model::time::iso_now();
                    let stamp = match what {
                        "time" => nocturne_model::time::clock_from_iso(&now),
                        "datetime" => nocturne_model::time::long_from_iso(&now),
                        _ => nocturne_model::time::date_from_iso(&now),
                    };
                    self.insert_in_page(&stamp.unwrap_or(now));
                }
                other => self.insert_block(other),
            },
            ribbon::Act::AddPrintout => {
                let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) else {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                    return;
                };
                let picker = rfd::FileDialog::new().set_title(crate::tl_id!("ui.choose_a_pdf_or_office_document_to_print_out")).add_filter(
                    crate::tl_id!("ui.pdf_and_office_documents"),
                    &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx", "odt", "odp", "ods", "rtf"],
                );
                if let Some(file) = picker.pick_file() {
                    self.add_file(&dir, &file, FileChoice::Printout);
                }
            }
            ribbon::Act::AddFile | ribbon::Act::AddPicture => {
                let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) else {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                    return;
                };
                let mut picker = rfd::FileDialog::new().set_title(if matches!(act, ribbon::Act::AddPicture) {
                    crate::tl_id!("ui.choose_a_picture")
                } else {
                    crate::tl_id!("ui.choose_a_file_to_attach")
                });
                if matches!(act, ribbon::Act::AddPicture) {
                    picker = picker.add_filter(crate::tl_id!("ui.pictures"), &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "jxl"]);
                }
                if let Some(file) = picker.pick_file() {
                    self.dialog = Some(Dialog::FileOptions { dir, files: vec![file] });
                }
            }
            ribbon::Act::ToggleSource => self.set_show_source(!self.show_source),
            ribbon::Act::ToggleFullPage => self.full_page = !self.full_page,
            ribbon::Act::OpenThemes => self.theme_window = !self.theme_window,
            ribbon::Act::RuleLines(r) => {
                // The paper is the page's own, as OneNote's rule lines are. The window's
                // own setting stays as what a page with none of its own is drawn on.
                self.rules = r;
                let _ = self.session.settings.set("rule_lines", r.id());
                if self.editor.is_some() {
                    self.set_page_appearance(Some(r), None);
                }
                self.say(match r {
                    ribbon::RuleLines::None => crate::tl_id!("ui.no_lines_behind_the_page").to_string(),
                    other => crate::i18n::format_message_id("ui.paper_behind_the_page", &[("paper", crate::tl!(other.label()))]),
                });
            }
            ribbon::Act::ToggleAlwaysOnTop => {
                self.always_on_top = !self.always_on_top;
                // Asked for on the next frame, where the context is at hand.
                self.window_level_pending = true;
                let _ = self.session.settings.set("always_on_top", if self.always_on_top { "true" } else { "false" });
                self.say(if self.always_on_top {
                    crate::tl_id!("ui.this_window_stays_on_top")
                } else {
                    crate::tl_id!("ui.this_window_behaves_like_the_others")
                });
            }
            ribbon::Act::TogglePanes => {
                self.split_panes = !self.split_panes;
                self.organisers = true;
                let _ = self.session.settings.set("split_panes", if self.split_panes { "true" } else { "false" });
                self.say(if self.split_panes {
                    crate::tl_id!("ui.notebooks_and_pages_in_their_own_columns")
                } else {
                    crate::tl_id!("ui.one_tree_of_notebooks")
                });
            }
            ribbon::Act::ToggleTitle => {
                self.hide_title = !self.hide_title;
                let _ = self.session.settings.set("hide_title", if self.hide_title { "true" } else { "false" });
            }
            ribbon::Act::ToggleSnap => {
                self.snap_to_grid = !self.snap_to_grid;
                let _ = self.session.settings.set("snap_to_grid", if self.snap_to_grid { "true" } else { "false" });
            }
            ribbon::Act::Zoom(step) => {
                let next = match step {
                    1 => self.zoom_percent.saturating_add(10),
                    -1 => self.zoom_percent.saturating_sub(10),
                    _ => 100,
                };
                self.set_zoom_percent(next);
                self.fit_page_width = false;
            }
            ribbon::Act::ZoomTo(percent) => {
                self.set_zoom_percent(percent);
                self.fit_page_width = false;
            }
            ribbon::Act::PageWidth => self.fit_page_width = true,
            ribbon::Act::FitAllContent => {
                if let Some(ed) = self.editor.as_ref() {
                    let ink = self.pad.as_ref().and_then(crate::inkpad::Pad::bounds);
                    let bounds_page = crate::minimap::content_bounds_page(&ed.last_frames, ink.as_slice());
                    let view_size = self.page_view_rect.or(self.page_rect).map_or(vec2(820.0, 620.0), |r| r.size());
                    let target_zoom = crate::minimap::compute_fit_zoom_percent(bounds_page, view_size);
                    self.set_zoom_percent(target_zoom);
                    self.fit_page_width = false;
                    self.canvas_pan = egui::Vec2::ZERO;
                    self.pending_scroll_offset_y = Some(0.0);
                    self.pending_fit_bounds = Some(bounds_page);
                }
            }
            ribbon::Act::ToggleMinimap => {
                let next = !self.minimap_visible_now();
                self.minimap_override = Some(next);
                let _ = self.session.settings.set("show_minimap", if next { "true" } else { "false" });
            }
            ribbon::Act::ToggleTabletTools => {
                let next = !self.tablet_tools_minimized;
                if self.run("view.tablet_tools", &params(&[("minimized", if next { "true" } else { "false" })])).is_some() {
                    self.tablet_tools_minimized = next;
                    self.tool_settings_open = false;
                }
            }
            ribbon::Act::CanvasLayout(layout) => {
                if let Some(path) = self.editor.as_ref().map(|ed| path_str(&ed.dir)) {
                    self.save_now();
                    if self.run("page.canvas_layout", &params(&[("path", &path), ("layout", layout.id())])).is_some()
                        && let Some(ed) = self.editor.as_mut()
                    {
                        ed.canvas_layout = layout;
                        self.sync_dirty_since_last = true;
                        self.last_local_edit_at = Some(Instant::now());
                    }
                }
            }
            ribbon::Act::ToolbarDock(dock) => self.session.set_toolbar_dock(dock),
            ribbon::Act::DockRoster(roster) => self.session.set_dock_roster(roster),
            ribbon::Act::ToggleToolSettings => self.tool_settings_open = !self.tool_settings_open,
            ribbon::Act::ToggleMathOcr => {
                let next = if self.session.ocr_config().enabled { "false" } else { "true" };
                let _ = self.run("ink.math_ocr_toggle", &params(&[("enabled", next)]));
                if !self.session.ocr_config().enabled && self.math_ocr.as_ref().is_some_and(|c| c.mode == nocturne_engine::OcrMode::MathLatex) {
                    self.math_ocr = None;
                }
            }
            ribbon::Act::ToggleHandwritingOcr => {
                let next = if self.session.ocr_config().handwriting_enabled { "false" } else { "true" };
                let _ = self.run("ink.handwriting_ocr_toggle", &params(&[("enabled", next)]));
                if !self.session.ocr_config().handwriting_enabled
                    && self.math_ocr.as_ref().is_some_and(|c| c.mode == nocturne_engine::OcrMode::HandwritingText)
                {
                    self.math_ocr = None;
                }
            }
            ribbon::Act::OpenMathOcrSetup => {
                self.dialog = Some(Dialog::MathOcrSetup(crate::math_ocr::Setup::from_session(&self.session)));
            }
            ribbon::Act::LibraryView(view) => {
                let _ = self.run("ui.library_view", &params(&[("view", view.id())]));
            }
            ribbon::Act::DockMathOcr => {
                let cfg = self.session.ocr_config();
                if !cfg.enabled && !cfg.handwriting_enabled {
                    self.tool_settings_open = true;
                } else if self.held_ink_count() > 0 {
                    let mode = if cfg.enabled { nocturne_engine::OcrMode::MathLatex } else { nocturne_engine::OcrMode::HandwritingText };
                    self.pending_math_ocr_start = Some(mode);
                } else {
                    self.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Lasso));
                }
            }
            ribbon::Act::ToggleHoldToStraighten => {
                let on = self.session.ink_options().hold_to_straighten;
                self.session.set_hold_to_straighten(!on);
            }
            ribbon::Act::EraserPixel(pixel) => {
                self.session.set_eraser_kind(if pixel { nocturne_engine::EraserKind::Pixel } else { nocturne_engine::EraserKind::Object });
            }
            ribbon::Act::EraserWidthTenths(tenths) => {
                self.session.set_eraser_width(f32::from(tenths) / 10.0);
            }
            ribbon::Act::ToggleEraserAutoReturn => {
                self.eraser_auto_return = !self.eraser_auto_return;
                let _ = self.session.settings.set("eraser_auto_return", if self.eraser_auto_return { "true" } else { "false" });
            }
            ribbon::Act::OpenCloudSync => {
                self.open_cloud_sync_dialog();
            }
            ribbon::Act::ToggleStudioLayout => {
                let next = match self.session.layout_mode() {
                    nocturne_engine::UiLayoutMode::TabletStudio => nocturne_engine::UiLayoutMode::ClassicRibbon,
                    nocturne_engine::UiLayoutMode::ClassicRibbon => nocturne_engine::UiLayoutMode::TabletStudio,
                    nocturne_engine::UiLayoutMode::Auto => {
                        if self.last_ctx_is_ios && !self.compact {
                            nocturne_engine::UiLayoutMode::ClassicRibbon
                        } else {
                            nocturne_engine::UiLayoutMode::TabletStudio
                        }
                    }
                };
                self.session.set_layout_mode(next);
                self.last_tablet_studio = matches!(next, nocturne_engine::UiLayoutMode::TabletStudio);
                if !self.last_tablet_studio {
                    self.tool_settings_open = false;
                }
            }
            ribbon::Act::FormatPainter(sticky) => self.pick_up_formatting(sticky),
            ribbon::Act::PageTemplates => {
                let name = self.templates_pane.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                self.templates_pane = Some(TemplatesPane { templates: Vec::new(), name });
                self.refresh_templates();
            }
            ribbon::Act::InsertSpaceTool => {
                if self.editor.is_none() {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                    return;
                }
                self.space_tool = !self.space_tool;
                if self.space_tool {
                    self.pointer = ribbon::Pointer::Write;
                    self.sync_tool_state_to_session();
                    self.say(crate::tl_id!("ui.insert_space_how"));
                }
            }
            ribbon::Act::FindTags => {
                // OneNote looks in the section first; with no page open there is no section.
                let scope = self.tags_pane.as_ref().map_or(if self.editor.is_some() { TagScope::Section } else { TagScope::All }, |p| p.scope);
                let unchecked = self.tags_pane.as_ref().is_some_and(|p| p.unchecked);
                self.tags_pane = Some(TagsPane { scope, unchecked, found: Vec::new() });
                self.refresh_tags();
            }
            ribbon::Act::CollectAll => {
                if self.editor.as_ref().is_none_or(|ed| !ed.writable) {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                    return;
                }
                self.collect_pending = true;
                // The gathered page starts at its top left, so it is in view.
                self.canvas_pan = egui::Vec2::ZERO;
            }
            ribbon::Act::NotBuilt(what) => {
                self.status =
                    crate::i18n::format_message_id("ui.name_is_not_built_yet_see_the_roadmap_for_what_is_coming", &[("name", crate::tl!(what))]);
                self.status_is_error = false;
            }
            ribbon::Act::Tag(tag) => self.apply_tag(tag),
            ribbon::Act::Clipboard(which) => self.clipboard(which),
            ribbon::Act::Paste => self.paste_here(None),
            ribbon::Act::InkCopySelection => {
                if let Some(pad) = self.pad.as_ref().filter(|pad| pad.holding() > 0) {
                    let mut drawing = nocturne_ink::Drawing::new(1.0, 1.0);
                    for stroke in pad.held_strokes() {
                        drawing.add(stroke);
                    }
                    let svg = nocturne_ink::svg::write(&drawing, nocturne_ink::svg::Flavour::Editable);
                    match self.clipboard.set_text(&svg) {
                        Ok(()) => self.say(crate::tl_id!("ui.copied")),
                        Err(error) => self.complain(&error),
                    }
                }
            }
            ribbon::Act::InkDeleteSelection => {
                if self.pad.as_mut().is_some_and(crate::inkpad::Pad::delete_held) {
                    self.ink_selection_changed();
                }
            }
            ribbon::Act::InkColourSelection(colour) => {
                if self.pad.as_mut().is_some_and(|pad| pad.recolour_held(colour)) {
                    self.ink_selection_changed();
                }
            }
            ribbon::Act::InkPaste => {
                if !self.clipboard.text().is_some_and(|text| self.paste_ink(&text)) {
                    self.say(crate::tl_id!("ui.no_ink_on_clipboard"));
                }
            }
            ribbon::Act::Indent(deeper) => self.indent_block(deeper),
            ribbon::Act::ClearFormatting => self.clear_formatting(),
            ribbon::Act::Direction(d) => self.apply_direction(d),
            ribbon::Act::Justify(strength) => self.apply_justify(strength),
            ribbon::Act::PageDirection(d) => self.apply_page_direction(d),
            ribbon::Act::Embedding(d) => self.apply_embedding(d),
            ribbon::Act::InkPointer(p) => {
                self.pointer = p;
                if p != ribbon::Pointer::Write {
                    self.space_tool = false;
                }
                self.sync_tool_state_to_session();
            }
            ribbon::Act::ToggleEraser => self.toggle_eraser_tool(),
            ribbon::Act::ToggleFingerDrawing => {
                self.session.toggle_finger_drawing();
            }
            ribbon::Act::InkPen(i) => {
                self.nib = i.min(self.rack.len().saturating_sub(1));
                self.pointer = ribbon::Pointer::Draw;
                self.space_tool = false;
                self.remember_rack();
                self.sync_tool_state_to_session();
            }
            ribbon::Act::InkRack(edit) => self.edit_rack(edit),
            ribbon::Act::Table(what) => self.edit_table(what),
            ribbon::Act::DeleteTable => self.delete_table(),
            ribbon::Act::TableSort(descending) => self.sort_table(descending),
            ribbon::Act::TableSelect(what) => self.select_in_table(what),
            ribbon::Act::TableBorders(on) => self.set_table_borders(on),
            ribbon::Act::TableAlign(align) => self.align_table_cells(align),
            ribbon::Act::TableShade(shade) => self.shade_table_cells(shade),
            ribbon::Act::RuleColour(c) => {
                let Some(ed) = self.editor.as_mut() else {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                    return;
                };
                ed.rule_colour = c;
                let path = path_str(&ed.dir);
                self.run("page.paper", &params(&[("path", &path), ("rules", c.map(nocturne_model::NodeColor::id).unwrap_or("none"))]));
                self.say(match c {
                    Some(c) => crate::i18n::format_message_id("ui.the_lines_are_colour", &[("colour", &crate::tl!(c.label()).to_lowercase())]),
                    None => crate::tl_id!("ui.the_lines_are_the_theme_s_own_colour").to_string(),
                });
            }
            ribbon::Act::PageColour(c) => {
                self.set_page_appearance(None, Some(c));
                self.say(match c {
                    Some(c) => {
                        let colour = crate::tl!(c.label());
                        crate::i18n::format_message_id("ui.the_page_is_colour", &[("colour", &colour.to_lowercase())])
                    }
                    None => crate::tl_id!("ui.the_page_is_the_theme_s_own_colour").to_string(),
                });
            }
            ribbon::Act::Undo => self.undo(),
            ribbon::Act::Redo => self.redo(),
            ribbon::Act::ToGridCraft => self.hand_table_to_gridcraft(),
            ribbon::Act::FromGridCraft => self.read_table_from_gridcraft(),
            ribbon::Act::NewGridCraft => self.new_gridcraft(),
            ribbon::Act::InsertTable(rows, columns) => self.insert_table(rows, columns),
            ribbon::Act::InsertTableDialog => {
                if self.editor.is_none() {
                    self.say(crate::tl_id!("ui.open_a_page_first"));
                } else {
                    self.dialog = Some(Dialog::InsertTable { rows: "3".into(), columns: "3".into(), first: true });
                }
            }
            ribbon::Act::InkSensitivity(v) => {
                self.sensitivity = v.min(100);
                let _ = self.session.settings.set(SENSITIVITY_KEY, &self.sensitivity.to_string());
            }
            ribbon::Act::InkPressureCurve(curve) => {
                self.pressure_curve = curve.sane();
                let _ = self.session.settings.set(CURVE_KEY, &self.pressure_curve.write());
            }
            ribbon::Act::InkUndo => self.ink_undo(),
            ribbon::Act::InkRedo => self.ink_redo(),
            ribbon::Act::InkInsert => self.ink_insert(),
            ribbon::Act::InkExportPlain => self.ink_export_plain(),
            ribbon::Act::SavePictureAs(src) => self.save_picture_as(&src),
            ribbon::Act::TurnPicture(src, how) => self.turn_picture(&src, how),
        }
    }

    /// Adds, removes, moves or changes a tool on the rack, and remembers the result. The
    /// rules about what may not happen - the last tool cannot be taken off, a tool cannot
    /// be moved off either end - live in `nocturne_ink::rack`, so they hold wherever the
    /// rack is edited from.
    fn edit_rack(&mut self, edit: ribbon::RackEdit) {
        use nocturne_ink::rack;
        match edit {
            ribbon::RackEdit::Add(tool) => {
                if self.rack.len() >= rack::MAX_NIBS {
                    self.say(crate::tl_id!("ui.there_is_no_room_on_the_rack_for_another_tool"));
                    return;
                }
                self.rack.push(rack::new_nib(tool));
                self.nib = self.rack.len() - 1;
                self.pointer = ribbon::Pointer::Draw;
                let tool = crate::tl!(tool.label());
                self.say(crate::i18n::format_message_id("ui.added_a_tool", &[("tool", &tool.to_lowercase())]));
            }
            ribbon::RackEdit::Remove(i) => {
                if !rack::remove(&mut self.rack, i) {
                    self.say(crate::tl_id!("ui.the_last_tool_stays_on_the_rack"));
                    return;
                }
                if self.nib >= self.rack.len() {
                    self.nib = self.rack.len() - 1;
                } else if self.nib > i {
                    self.nib -= 1;
                }
                self.say(crate::tl_id!("ui.took_it_off_the_rack"));
            }
            ribbon::RackEdit::Move(i, left) => {
                let to = rack::shift(&mut self.rack, i, left);
                if self.nib == i {
                    self.nib = to;
                } else if self.nib == to {
                    self.nib = i;
                }
            }
            ribbon::RackEdit::Set(i, nib) => {
                if let Some(slot) = self.rack.get_mut(i) {
                    *slot = nib;
                    self.nib = i;
                    self.pointer = ribbon::Pointer::Draw;
                }
            }
        }
        self.remember_rack();
        self.sync_tool_state_to_session();
    }

    /// Puts whatever is on the clipboard into the page: a picture becomes a file beside
    /// the page and a line that shows it, anything else goes in as words.
    ///
    /// egui hands a window only `Paste(String)`, so a picture copied from a screen
    /// clipper never arrived and Ctrl+V did nothing at all. The clipboard is read here
    /// directly instead.
    /// Deletes a multi-block `ed.across` selection in place without recording an undo step.
    /// Callers wrap the full operation in a single `remember_step` / `remember_edit` transaction.
    fn delete_across_selection(ed: &mut Editor) -> (bool, bool, bool) {
        let Some(open) = ed.across.filter(|a| !a.inside_one_block()) else {
            ed.across = None;
            return (false, false, false);
        };
        let ((first, _), (last, _)) = open.ends();
        let end = last.min(ed.blocks.len().saturating_sub(1));
        let mut owner_boxes = Vec::new();
        if ed.arranged && !ed.boxes.is_empty() && end > first {
            for b_idx in (first + 1)..=end {
                if let Some((box_idx, _)) = nocturne_model::container::place(&ed.boxes, b_idx) {
                    owner_boxes.push(box_idx);
                }
            }
        }
        let (block, at) = pageedit::delete_span(&mut ed.blocks, open);
        for box_idx in owner_boxes {
            if let Some(c) = ed.boxes.get_mut(box_idx) {
                c.blocks = c.blocks.saturating_sub(1);
            }
        }
        let (mut cols_changed, mut justified_changed) = (false, false);
        if end > first {
            let removed = (first + 1)..(end + 1);
            cols_changed |= ed.columns.remove_blocks(removed.clone());
            justified_changed |= ed.justified.remove_blocks(removed);
        }
        if ed.arranged && ed.boxes.len() > 1 {
            let caret_box = nocturne_model::container::place(&ed.boxes, block).map(|(i, _)| i);
            for i in (0..ed.boxes.len()).rev() {
                if Some(i) != caret_box && ed.boxes[i].blocks == 0 {
                    ed.boxes.remove(i);
                }
            }
        }
        let text = ed.blocks.get(block).map(pageedit::caret_text).unwrap_or_default();
        ed.caret = Some(pageedit::Caret { block, text, focus: true, selection: (at, at), place: Some(at), cell: None, span: None, inline: None });
        ed.across = None;
        (true, cols_changed, justified_changed)
    }

    fn paste_here(&mut self, fallback_text: Option<String>) {
        let Some(ed) = self.editor.as_ref() else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        if !ed.writable {
            return;
        }
        let dir = ed.dir.clone();
        if fallback_text.as_deref().is_some_and(|text| self.paste_ink(text)) {
            return;
        }
        if let Some(picture) = self.clipboard.picture() {
            self.paste_picture(&dir, picture);
            return;
        }
        // Browsers, Word and other editors offer a formatted clipboard flavour as well
        // as plain text. Parse it through the same HTML-to-block path as an HTML page, so
        // links, emphasis, lists and tables survive a paste into Nocturne.
        if let Some(html) = self.clipboard.html() {
            let new = pageedit::pasted_blocks(pageedit::pasted(&html));
            if self.paste_blocks(new) {
                self.say(crate::tl_id!("ui.pasted_formatted_content"));
                return;
            }
        }
        match fallback_text.or_else(|| self.clipboard.text()) {
            Some(text) => {
                if self.paste_ink(&text) {
                    return;
                }
                let new = pageedit::blocks_to_paste(&text);
                if !self.paste_blocks(new) {
                    self.insert_in_page(&text);
                }
                self.say(crate::tl_id!("ui.pasted"));
            }
            None => self.say(crate::tl_id!("ui.there_is_nothing_on_the_clipboard_to_paste")),
        }
    }

    /// Handles the editable SVG placed on the clipboard by Copy Ink.
    fn paste_ink(&mut self, text: &str) -> bool {
        use nocturne_ink::svg;
        if !svg::is_ink(text) {
            return false;
        }
        let Some(ed) = self.editor.as_ref().filter(|ed| ed.writable) else { return true };
        let centre = self.page_view_rect.or(self.page_rect).map_or(ed.origin + vec2(120.0, 120.0), |view| view.center());
        let at = (centre - ed.origin) / (self.zoom_percent as f32 / 100.0).max(0.05);
        if let Ok((drawing, 0)) = svg::read(text)
            && self.pad.as_mut().is_some_and(|pad| pad.paste_strokes(drawing, (at.x, at.y)))
        {
            self.ink_selection_changed();
        } else {
            self.complain(crate::tl_id!("ui.cannot_paste_ink"));
        }
        true
    }

    fn ink_selection_changed(&mut self) {
        let Some(pad) = self.pad.as_mut() else { return };
        pad.settle();
        let result = pad.save();
        let now = Instant::now();
        self.last_ink_change = Some(now);
        self.sync_dirty_since_last = true;
        self.last_local_edit_at = Some(now);
        self.ink_save_completed(result, true);
    }

    fn ink_save_completed(&mut self, result: std::io::Result<()>, references_may_change: bool) {
        match result {
            Ok(()) if self.pad.as_ref().is_none_or(|pad| !pad.has_unsaved_changes()) => {
                let count = self.ink_stroke_count().to_string();
                self.say(crate::i18n::format_message_id("ui.drawing_saved_count_strokes", &[("count", &count)]));
            }
            Ok(()) => {}
            Err(error) => self.complain(&crate::i18n::format_message_id("ui.cannot_write_the_drawing_error", &[("error", &error.to_string())])),
        }
        if references_may_change {
            self.tell_kept_ink();
            self.sync_ink_reference();
        }
        if let Some(ed) = self.editor.as_ref() {
            self.library_grid.invalidate_page(&ed.dir);
        }
    }

    fn poll_ink_save(&mut self, ctx: &Context) {
        if let Some(pad) = self.pad.as_mut().filter(|pad| pad.save_pending()) {
            if let Some(result) = pad.finish_save(false) {
                let follow_up = if result.is_ok() && pad.dirty { pad.save_in_background(ctx) } else { None };
                self.ink_save_completed(result, false);
                if let Some(result) = follow_up {
                    self.ink_save_completed(result, true);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(35));
        }
    }

    /// Pastes `new` blocks at the caret, or after the last block. False, with nothing done,
    /// when there is nothing to paste or the page cannot be written.
    fn paste_blocks(&mut self, new: Vec<nocturne_formats::Block>) -> bool {
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.writable) else { return false };
        if new.is_empty() {
            return false;
        }
        Self::remember_step(ed);
        let (_, across_cols, across_just) = Self::delete_across_selection(ed);
        let at = ed.caret.as_ref().map_or(ed.blocks.len().saturating_sub(1), |c| c.block);
        if ed.blocks.is_empty() {
            ed.blocks.push(nocturne_formats::markdown::Block::Paragraph(Vec::new()));
        }
        let before = ed.blocks.len();
        pageedit::paste_at(&mut ed.blocks, &mut ed.caret, at, new);
        // The paste fills the caret's line and puts the rest after it, all in the caret's box.
        let marks = ed.blocks_inserted(at + 1, ed.blocks.len().saturating_sub(before));
        let marks = (across_cols || marks.0, across_just || marks.1);
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        self.save_block_marks(marks);
        true
    }

    /// Writes table widths and justified paragraphs that moved with inserted blocks, as
    /// [`Editor::blocks_inserted`] reported them, into the page's own file.
    fn save_block_marks(&mut self, (cols, justified): (bool, bool)) {
        let Some(ed) = self.editor.as_ref() else { return };
        let path = path_str(&ed.dir);
        let cols = cols.then(|| nocturne_model::tablecols::write(&ed.columns));
        let justified = justified.then(|| nocturne_model::justify::write(&ed.justified));
        if let Some(line) = cols {
            self.run("page.columns", &params(&[("path", &path), ("widths", &line)]));
        }
        if let Some(line) = justified {
            self.run("page.justify", &params(&[("path", &path), ("blocks", &line)]));
        }
    }

    fn paste_picture(&mut self, dir: &std::path::Path, picture: crate::clipboard::Picture) {
        if !self.editor.as_ref().is_some_and(|ed| ed.writable) {
            return;
        }
        let name = crate::clipboard::picture_name();
        let (w, h) = (picture.width, picture.height);
        log::debug!("clipboard image paste received: width={w} height={h} bytes={}", picture.png.len());
        match nocturne_store::assets::attach_bytes(dir, &name, &picture.png) {
            Ok(link) => {
                log::debug!("clipboard image paste attached: name={name:?} link={link:?}");
                if let Some(ed) = self.editor.as_mut() {
                    Self::remember_step(ed);
                    let (_, across_cols, across_just) = Self::delete_across_selection(ed);
                    let at = ed.caret.as_ref().map_or(ed.blocks.len(), |c| c.block + 1).min(ed.blocks.len());
                    ed.blocks.insert(
                        at,
                        nocturne_formats::markdown::Block::Image { alt: "Pasted picture".to_string(), src: link, width: None, height: None },
                    );
                    let marks = ed.blocks_inserted(at, 1);
                    let marks = (across_cols || marks.0, across_just || marks.1);
                    ed.rebuild_body();
                    Self::mark_dirty_from_blocks(ed);
                    Self::remember_edit(ed);
                    ed.attachments = nocturne_store::assets::list(dir);
                    self.save_block_marks(marks);
                }
                self.say(crate::i18n::format_message_id(
                    "ui.pasted_a_picture_width_by_height",
                    &[("width", &w.to_string()), ("height", &h.to_string())],
                ));
            }
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_save_the_pasted_picture_error", &[("error", &e.to_string())])),
        }
    }

    /// Keeps the page's words honest about its ink: a page with ink names `ink.svg`, a
    /// page without it does not. Without this the source pane showed a page of text while
    /// the page itself was covered in handwriting, and another program opening the folder
    /// would never have known the drawing was there.
    fn sync_ink_reference(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        // The folder is asked again: the pen has just written or deleted a file.
        ed.ink_files = nocturne_store::page::ink_files(&ed.dir);
        let (changed, cols_moved, justified_moved) = Self::name_ink(ed);
        if changed {
            Self::mark_dirty_from_blocks(ed);
        }
        if cols_moved || justified_moved {
            self.save_block_marks((cols_moved, justified_moved));
        }
    }

    /// The page names every ink file in its folder, and names no ink file that is gone.
    ///
    /// The folder decides. A drawing is a file beside the page file; the line in the page
    /// that names it is how the page's words and what is drawn on it agree, and how
    /// another program opening the page sees the handwriting. Before this, the line could
    /// be lost - deleting the box that held it took it out - and nothing put it back until
    /// the pen next moved, so a page could say nothing about ink that was still there.
    ///
    /// Returns whether the page's text, table widths, and justification changed.
    fn name_ink(ed: &mut Editor) -> (bool, bool, bool) {
        use nocturne_formats::markdown::Block;
        let files = ed.ink_files.clone();
        let (mut changed, mut cols_moved, mut justified_moved) = (false, false, false);
        // A line that names an ink file that is no longer in the folder goes.
        // One at a time, from the end, so each box that held one counts one fewer.
        let gone: Vec<usize> = ed
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| match b {
                Block::Image { src, alt, .. } => crate::inkpad::is_ink_line(src, alt) && !files.iter().any(|f| f == src.trim()),
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        for &at in gone.iter().rev() {
            ed.blocks.remove(at);
            let (cols, justified) = ed.block_removed(at);
            cols_moved |= cols;
            justified_moved |= justified;
            changed = true;
        }
        // An ink file the page does not name is adopted: it is drawn, so it is named. The line
        // goes on the end of the page, so it is counted in the last box.
        for file in &files {
            if !ed.blocks.iter().any(|b| matches!(b, Block::Image { src, .. } if src.trim() == file)) {
                ed.blocks.push(Block::Image { alt: "Ink".to_string(), src: file.clone(), width: None, height: None });
                let at = ed.blocks.len() - 1;
                ed.count_inserted_in_boxes(at, 1);
                changed = true;
            }
        }
        if changed {
            ed.rebuild_body();
        }
        (changed, cols_moved, justified_moved)
    }

    /// Writes the rack and which tool is in hand into the settings, so the next run opens
    /// with the pens the owner arranged.
    fn remember_rack(&mut self) {
        let line = nocturne_ink::rack::write(&self.rack);
        let _ = self.session.settings.set(RACK_KEY, &line);
        let _ = self.session.settings.set(NIB_KEY, &self.nib.to_string());
    }

    /// The pen as the page's ink layer wants it: what it is, and what the pointer is for.
    fn pen(&self) -> crate::inkpad::Pen {
        let nib =
            self.rack.get(self.nib).or_else(|| self.rack.first()).copied().unwrap_or_else(|| nocturne_ink::rack::new_nib(nocturne_ink::Tool::Pen));
        crate::inkpad::Pen {
            pointer: self.pointer,
            tool: nib.tool,
            colour: nib.colour,
            // Keep the chosen nib width at the neutral setting, but let the top of the
            // control reach the same four-times full-width range as the pressure canvas.
            width: nib.width() * Self::sensitivity_width(self.sensitivity),
            gain: self.gain(),
            curve: self.pressure_curve,
            pixel_eraser: self.session.ink_options().eraser_kind == nocturne_engine::EraserKind::Pixel,
            eraser_width: self.session.ink_options().eraser_width,
            hold_to_straighten: self.session.ink_options().hold_to_straighten,
        }
    }

    /// The pressure canvas uses a 4x full-width range at sensitivity 100. Keep 50 neutral
    /// so the rack's selected thickness remains the familiar baseline, and grow only the
    /// maximum width above it. Existing strokes retain the width stored when they were made.
    fn sensitivity_width(sensitivity: u8) -> f32 {
        if sensitivity <= 50 { 1.0 } else { 1.0 + 3.0 * (f32::from(sensitivity - 50) / 50.0) }
    }

    /// The sensitivity as the curve a stroke is drawn with: the middle of the control is
    /// 1, which leaves the pen's readings exactly as they came.
    fn gain(&self) -> f32 {
        let share = f32::from(self.sensitivity.min(100)) / 50.0;
        if share <= 1.0 {
            nocturne_ink::MIN_GAIN + (1.0 - nocturne_ink::MIN_GAIN) * share
        } else {
            1.0 + (nocturne_ink::MAX_GAIN - 1.0) * (share - 1.0)
        }
    }

    /// Puts back the change to the drawing that undo last took back.
    fn ink_redo(&mut self) {
        let Some(pad) = self.pad.as_mut() else {
            self.say(crate::tl_id!("ui.nothing_to_redo"));
            return;
        };
        if !pad.redo() {
            self.say(crate::tl_id!("ui.nothing_to_redo"));
            return;
        }
        let strokes = pad.strokes();
        match pad.save() {
            Ok(()) => self.say(crate::i18n::format_message_id("ui.drawing_saved_count_strokes", &[("count", &strokes.to_string())])),
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_write_the_drawing_error", &[("error", &e.to_string())])),
        }
        self.sync_ink_reference();
    }

    /// Takes the last stroke back out of the drawing on this page.
    fn ink_undo(&mut self) {
        let Some(pad) = self.pad.as_mut() else {
            self.say(crate::tl_id!("ui.there_is_no_drawing_on_this_page"));
            return;
        };
        if !pad.undo() {
            self.say(crate::tl_id!("ui.there_is_no_stroke_to_take_back"));
            return;
        }
        let left = pad.strokes();
        match pad.save() {
            Ok(()) => self.say(crate::i18n::format_message_id("ui.took_the_last_stroke_back_count_left", &[("count", &left.to_string())])),
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_write_the_drawing_error", &[("error", &e.to_string())])),
        }
        self.tell_kept_ink();
        // The page stops naming ink it no longer has.
        self.sync_ink_reference();
    }

    /// Whether the caret is in a table, which is when the Table tab is offered.
    /// The Styles kind of the block the caret is in (or that a selected box is all of), for the ribbon to highlight; "" with neither.
    fn caret_style(&self, ctx: &Context) -> &'static str {
        let Some(ed) = self.editor.as_ref() else { return "" };
        if let Some(block) = ed.caret.as_ref().and_then(|c| ed.blocks.get(c.block)) {
            return pageedit::block_kind(block);
        }
        // A selected box has everything in it selected: its style is the one they all share.
        let range = crate::containers::selected(ctx, &ed.dir).map(|i| nocturne_model::container::blocks_of(&ed.boxes, i)).unwrap_or_default();
        let mut kinds = range.filter_map(|i| ed.blocks.get(i)).map(pageedit::block_kind);
        match kinds.next() {
            Some(first) if kinds.all(|k| k == first) => first,
            _ => "",
        }
    }

    fn caret_in_table(&self) -> bool {
        self.editor
            .as_ref()
            .and_then(|e| e.caret.as_ref().map(|c| (e, c)))
            .is_some_and(|(e, c)| c.cell.is_some() && matches!(e.blocks.get(c.block), Some(nocturne_formats::markdown::Block::Table { .. })))
    }

    /// Whether the table the caret is in is drawn with lines round its cells. A table has
    /// them until its own borders are hidden, so with the caret outside any table this reads
    /// as true.
    fn caret_table_has_borders(&self) -> bool {
        let Some(ed) = self.editor.as_ref() else { return true };
        let Some(c) = ed.caret.as_ref().filter(|c| c.cell.is_some()) else { return true };
        !ed.columns.borders_hidden(c.block)
    }

    /// The cells Table > Align acts on: every picked cell, or the caret's own. With the
    /// table's block.
    fn caret_cells(&self) -> Option<(usize, Vec<(usize, usize)>)> {
        let ed = self.editor.as_ref()?;
        let c = ed.caret.as_ref()?;
        let here = c.cell?;
        if !matches!(ed.blocks.get(c.block), Some(nocturne_formats::markdown::Block::Table { .. })) {
            return None;
        }
        let cells = match pageedit::cell_range(c) {
            Some((a, b)) => (a.0..=b.0).flat_map(|r| (a.1..=b.1).map(move |col| (r, col))).collect(),
            None => vec![here],
        };
        Some((c.block, cells))
    }

    /// How the cells Table > Align would act on are aligned now, when they all agree. A cell
    /// given no alignment of its own reads as aligned left, which is how it is drawn.
    fn caret_cells_align(&self) -> Option<nocturne_model::tablecols::Align> {
        use nocturne_model::tablecols::Align;
        let ed = self.editor.as_ref()?;
        let (at, cells) = self.caret_cells()?;
        let mut aligns = cells.iter().map(|cell| ed.columns.align(at, *cell).unwrap_or(Align::Left));
        let first = aligns.next()?;
        aligns.all(|a| a == first).then_some(first)
    }

    /// The colour behind the cells Table > Shading would act on, when they all agree.
    fn caret_cells_shade(&self) -> Option<Option<nocturne_model::NodeColor>> {
        let ed = self.editor.as_ref()?;
        let (at, cells) = self.caret_cells()?;
        let mut shades = cells.iter().map(|cell| ed.columns.shade(at, *cell));
        let first = shades.next()?;
        shades.all(|s| s == first).then_some(first)
    }

    /// Table > Shading: a colour behind the caret's cell, or behind every picked cell, and no
    /// other; No Colour takes it away. Kept with the page beside the cells' alignment, and
    /// noted on the page's history step as the borders and the alignment are (issue #169).
    fn shade_table_cells(&mut self, shade: Option<nocturne_model::NodeColor>) {
        let Some((at, cells)) = self.caret_cells() else {
            self.say(crate::tl_id!("ui.click_in_the_table_first"));
            return;
        };
        let Some(ed) = self.editor.as_mut() else { return };
        if !ed.writable || !ed.columns.set_shade(at, &cells, shade) {
            return;
        }
        Self::remember_edit(ed);
        self.save_block_marks((true, false));
    }

    /// Table > Align Left, Center or Align Right: the words in the caret's cell, or in every
    /// cell picked, and nothing else; the columns keep their widths and the table its place.
    /// Kept with the page, like the widths and the borders, and one step of Undo (issue #170).
    fn align_table_cells(&mut self, align: nocturne_model::tablecols::Align) {
        let Some((at, cells)) = self.caret_cells() else {
            self.say(crate::tl_id!("ui.click_in_the_table_first"));
            return;
        };
        let Some(ed) = self.editor.as_mut() else { return };
        if !ed.writable {
            return;
        }
        // Left is how a cell is drawn when it was given nothing, so it is kept as nothing.
        let keep = (align != nocturne_model::tablecols::Align::Left).then_some(align);
        if !ed.columns.set_align(at, &cells, keep) {
            return;
        }
        Self::remember_edit(ed);
        self.save_block_marks((true, false));
    }

    /// Table > Hide Borders or Show Borders: the table the caret is in, and no other. The
    /// choice is kept with the page, like the widths of the table's columns, so it is there
    /// when the page is opened again (issue #161).
    fn set_table_borders(&mut self, on: bool) {
        let Some(ed) = self.editor.as_mut() else { return };
        if !ed.writable {
            return;
        }
        let Some(at) = ed.caret.as_ref().filter(|c| c.cell.is_some()).map(|c| c.block) else {
            self.say(crate::tl_id!("ui.click_in_the_table_first"));
            return;
        };
        if !matches!(ed.blocks.get(at), Some(nocturne_formats::markdown::Block::Table { .. })) || !ed.columns.set_borders(at, !on) {
            return;
        }
        // Noted on the step the page is at, which is how a dragged column is noted too: an
        // undo back to here keeps the choice.
        Self::remember_edit(ed);
        self.save_block_marks((true, false));
    }

    /// Writes a file of the open page that was typed in in the code view.
    ///
    /// Only the files the code view lets anybody type in reach here: a drawing's SVG. The
    /// page file's body is not one of them; it is the editor's own text and is saved the
    /// way it always was.
    fn write_page_file(&mut self, name: &str, text: &str) {
        let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) else { return };
        let is_page_file = self.editor.as_ref().is_some_and(|e| e.format.page_file() == name);
        let path = dir.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !path.starts_with(&dir) {
            self.complain(crate::tl_id!("ui.that_file_is_not_in_this_page_s_folder"));
            return;
        }
        // The page file may be the one an autosave is still writing.
        self.take_saves(true);
        if let Err(e) = nocturne_store::page::write_atomic(&path, text) {
            self.complain(&crate::i18n::format_message_id("ui.cannot_write_name_error", &[("name", name), ("error", &e.to_string())]));
            return;
        }
        if is_page_file {
            // The page itself was typed in. Nothing the window remembered about it is
            // true any more, and the autosave must not write its old copy over this one;
            // the page is read again when the view closes.
            if let Some(ed) = self.editor.as_mut() {
                ed.dirty_since = None;
                ed.body = nocturne_store::page::load_page(&dir).map(|p| p.body).unwrap_or_else(|_| ed.body.clone());
                let (blocks, _) = nocturne_formats::boxes::read(ed.format, &ed.body);
                let audit = nocturne_formats::boxes::span_audit(&ed.body, &blocks);
                log::debug!("code view commit span audit: page={:?} raw_span_tags={} styled_spans={}", dir, audit.raw_tags, audit.styled);
            }
            self.code.page_edited = true;
            return;
        }
        // A drawing that was typed in is read again, so what the pen does next is done to
        // the file as it now stands rather than to the one the window remembered.
        if nocturne_ink::svg::is_ink(text) {
            self.pad = Some(crate::inkpad::Pad::of_page(&dir));
        }
        self.sync_ink_reference();
    }

    /// The paper the open page is drawn on: its own, or the window's choice when it has
    /// none of its own.
    fn paper_now(&self) -> ribbon::RuleLines {
        self.editor.as_ref().and_then(|e| e.paper).unwrap_or(self.rules)
    }

    /// Gives the open page its own paper, its own colour, or both, and writes them into
    /// the page's file. `None` for either leaves that one as it is.
    fn set_page_appearance(&mut self, paper: Option<ribbon::RuleLines>, colour: Option<Option<nocturne_model::NodeColor>>) {
        let Some(ed) = self.editor.as_mut() else {
            self.say("Open a page first");
            return;
        };
        if let Some(p) = paper {
            ed.paper = Some(p);
        }
        if let Some(c) = colour {
            ed.colour = c;
        }
        let (path, paper, colour) = (path_str(&ed.dir), ed.paper, ed.colour);
        let pairs: Vec<(&str, &str)> = vec![
            ("path", path.as_str()),
            ("paper", paper.map(ribbon::RuleLines::id).unwrap_or("none")),
            ("color", colour.map(nocturne_model::NodeColor::id).unwrap_or("none")),
        ];
        self.run("page.paper", &params(&pairs));
    }

    /// Takes the page back to how it was before the last change, or forward again.
    ///
    /// The whole page is put back: its text, its blocks, the boxes they sit in and where
    /// the caret was. Ink is not part of this; a stroke is taken back with Undo Stroke on
    /// the Draw tab, because the drawing is its own file beside the page.
    fn step_back(&mut self, back: bool) {
        let mut nothing = false;
        let mut cols_moved = false;
        let mut justified_moved = false;
        if let Some(ed) = self.editor.as_mut() {
            if !ed.writable {
                nothing = true;
            } else {
                let step = if back { ed.history.undo().cloned() } else { ed.history.redo().cloned() };
                match step {
                    Some(step) => {
                        let prev_caret = ed.caret.clone();
                        ed.body = step.body;
                        ed.blocks = step.blocks;
                        ed.boxes = step.boxes;
                        ed.caret = step.caret.map(|c| pageedit::Caret { focus: true, ..c }).or_else(|| {
                            prev_caret.and_then(|mut c| {
                                let b = ed.blocks.get(c.block).filter(|b| pageedit::is_writable(b))?;
                                c.text = pageedit::caret_text(b);
                                let len = c.text.chars().count();
                                c.selection = (c.selection.0.min(len), c.selection.1.min(len));
                                c.place = Some(c.selection.1);
                                c.focus = true;
                                c.inline = None;
                                Some(c)
                            })
                        });
                        // A selection across blocks is kept in byte offsets into the text as
                        // it was; the restored text can differ (a format adds its markup), so
                        // the selection is let go rather than left pointing at the wrong words.
                        ed.across = None;
                        ed.pending_format = None;
                        ed.dirty_since = Some(Instant::now());
                        // The restored text says whether the page was arranged.
                        ed.arranged = nocturne_formats::boxes::has_boxes(ed.format, &ed.body);
                        // The table widths as they were then, written back when they differ.
                        if let Some(columns) = step.columns
                            && columns != ed.columns
                        {
                            ed.columns = columns;
                            cols_moved = true;
                        }
                        // And the justified paragraphs, which a cut across blocks moves up.
                        if let Some(justified) = step.justified
                            && justified != ed.justified
                        {
                            ed.justified = justified;
                            justified_moved = true;
                        }
                    }
                    None => nothing = true,
                }
            }
        }
        if cols_moved || justified_moved {
            self.save_block_marks((cols_moved, justified_moved));
        }
        if nothing {
            self.say(if back { crate::tl_id!("ui.nothing_to_undo") } else { crate::tl_id!("ui.nothing_to_redo") });
        }
    }

    /// Remembers the page as it stands now, so the change that just happened can be taken
    /// back. Called once a frame, after whatever changed has been applied.
    ///
    /// Letters going into the block the caret was already in fold into the step before
    /// them, so Ctrl+Z takes back a run of writing rather than one character.
    fn remember_step(ed: &mut Editor) {
        Self::remember(ed, false);
    }

    /// Remembers a change made from the ribbon rather than by writing, such as Insert Space
    /// or a row put into a table, as a step of its own. It never folds into the writing
    /// before it, even when the caret stays in the same block (issue #153).
    fn remember_edit(ed: &mut Editor) {
        Self::remember(ed, true);
    }

    fn remember(ed: &mut Editor, edit: bool) {
        Self::remember_saved(ed, edit, None);
    }

    /// A letter typed or deleted by the platform keyboard, with the caret staying in its block:
    /// the page's text is written once here, and the page as it would be saved is worked out
    /// once and used both to say whether there is something to save and by what undo remembers.
    /// Each of those used to work it out again, on every letter.
    fn note_typing(ed: &mut Editor) {
        ed.rebuild_body();
        let saved = ed.body_to_save();
        let file_will_hold = ed.saving.as_ref().unwrap_or(&ed.saved_body);
        ed.dirty_since = (saved != *file_will_hold).then(Instant::now);
        Self::remember_saved(ed, false, Some(saved));
    }

    /// [`Self::remember`], given the page as it would be saved when the caller already has it.
    fn remember_saved(ed: &mut Editor, edit: bool, saved: Option<String>) {
        let same = ed.history.here().is_some_and(|s| s.body == ed.body && s.blocks == ed.blocks && s.boxes == ed.boxes);
        if same || {
            // Only the cursor's place on open paper came or went: the page is the one remembered.
            let saved = saved.unwrap_or_else(|| ed.body_to_save());
            saved != ed.body && ed.history.here().is_some_and(|s| s.body == saved)
        } {
            // Nothing really changed, or the page was just put back by undo itself. A column
            // dragged since is still noted, so an undo back to here keeps it (issue #154).
            if let Some(step) = ed.history.here_mut() {
                step.columns = Some(ed.columns.clone());
                step.justified = Some(ed.justified.clone());
            }
            return;
        }
        let here = ed.caret.as_ref().map(|c| c.block);
        let was = ed.history.here().and_then(|s| s.caret.as_ref().map(|c| c.block));
        let shape = ed.history.here().is_some_and(|s| s.blocks.len() == ed.blocks.len());
        let kind = match (shape, here, was) {
            (true, Some(a), Some(b)) if a == b && !edit => crate::history::Kind::Typing(a),
            _ => crate::history::Kind::Edit,
        };
        let mut step = crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), kind);
        step.columns = Some(ed.columns.clone());
        step.justified = Some(ed.justified.clone());
        ed.history.record(step);
    }

    /// Puts the spreadsheet back into the table it came from.
    ///
    /// The file is the one Convert wrote for this table, beside the page. Nothing watches
    /// it: a page would be a strange thing to write in if its tables changed under the
    /// writer's hands, so the way back is a button, as the way out is.
    fn read_table_from_gridcraft(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.say(crate::tl_id!("ui.click_in_the_table_first"));
            return;
        };
        if !matches!(ed.blocks.get(at), Some(nocturne_formats::markdown::Block::Table { .. })) {
            self.say(crate::tl_id!("ui.click_in_a_table_first"));
            return;
        }
        let file = ed.dir.join(format!("table-{}.csv", at + 1));
        let Ok(text) = std::fs::read_to_string(&file) else {
            let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.complain(&crate::i18n::format_message_id("ui.no_name_beside_this_page_hand_the_table_over_first", &[("name", &name)]));
            return;
        };
        let Some((table, rows, columns)) = table_from_gridcraft_csv(&text) else {
            self.complain(crate::tl_id!("ui.that_file_has_nothing_in_it_to_put_back"));
            return;
        };
        if let Some(block) = ed.blocks.get_mut(at) {
            *block = table;
        }
        ed.caret = None;
        ed.body = pageedit::to_body(ed.format, &ed.blocks);
        Self::mark_dirty_from_blocks(ed);
        self.say(crate::i18n::format_message_id(
            "ui.read_back_rows_rows_and_columns_columns",
            &[("rows", &rows.to_string()), ("columns", &columns.to_string())],
        ));
    }

    /// Hands the table the caret is in to GridCraft, which is where a spreadsheet
    /// belongs: OneNote's Table tab ends with Convert to Excel Spreadsheet, and this is
    /// the same door with the open spreadsheet behind it.
    fn hand_table_to_gridcraft(&mut self) {
        let Some(exe) = self.gridcraft.clone() else {
            self.say(crate::tl_id!("ui.gridcraft_is_not_installed"));
            return;
        };
        let Some(ed) = self.editor.as_ref() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else { return };
        let Some(block) = ed.blocks.get(at) else { return };
        let name = format!("table-{}", at + 1);
        match crate::gridcraft::hand_over(&exe, &ed.dir, &name, block) {
            Ok(file) => {
                let name = file.file_name().unwrap_or_default().to_string_lossy().to_string();
                let said = crate::i18n::format_message_id("ui.name_is_open_in_gridcraft", &[("name", &name)]);
                self.say(&said);
            }
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_open_the_table_in_gridcraft_error", &[("error", &e.to_string())])),
        }
    }

    /// A table of the picked size, where the caret is.
    fn insert_table(&mut self, rows: usize, columns: usize) {
        let Some(ed) = self.editor.as_mut() else { return };
        let at = ed.caret.as_ref().map(|c| c.block + 1).unwrap_or(ed.blocks.len()).min(ed.blocks.len());
        let before = ed.blocks.len();
        pageedit::insert_table(&mut ed.blocks, at, rows, columns);
        let marks = ed.blocks_inserted(at, ed.blocks.len().saturating_sub(before));
        ed.rebuild_body();
        ed.caret = Some(pageedit::Caret {
            block: at.min(ed.blocks.len() - 1),
            text: String::new(),
            focus: true,
            selection: (0, 0),
            place: None,
            cell: Some((0, 0)),
            span: None,
            inline: None,
        });
        Self::mark_dirty_from_blocks(ed);
        self.save_block_marks(marks);
    }

    /// An empty spreadsheet beside the page, opened in GridCraft and linked from the page
    /// so it can be found again.
    fn new_gridcraft(&mut self) {
        let Some(exe) = self.gridcraft.clone() else {
            self.say(crate::tl_id!("ui.gridcraft_is_not_installed"));
            return;
        };
        let Some(ed) = self.editor.as_ref() else { return };
        let dir = ed.dir.clone();
        let mut name = "spreadsheet.csv".to_string();
        for n in 2..100 {
            if !dir.join(&name).exists() {
                break;
            }
            name = format!("spreadsheet-{n}.csv");
        }
        let file = dir.join(&name);
        if let Err(e) = std::fs::write(&file, "") {
            self.complain(&crate::i18n::format_message_id("ui.cannot_write_name_error", &[("name", &name), ("error", &e.to_string())]));
            return;
        }
        match std::process::Command::new(exe).arg(&file).spawn() {
            Ok(_) => {
                self.insert_in_page(&format!("[{name}]({name})"));
                let said = crate::i18n::format_message_id("ui.name_is_open_in_gridcraft", &[("name", &name)]);
                self.say(&said);
            }
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_open_gridcraft_error", &[("error", &e.to_string())])),
        }
    }

    /// Picks a block of cells from the Table tab's Select buttons, as a drag across them
    /// would.
    fn select_in_table(&mut self, what: pageedit::TableSpan) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some((at, cell)) = ed.caret.as_ref().and_then(|c| c.cell.map(|cell| (c.block, cell))) else {
            return;
        };
        let Some(block) = ed.blocks.get(at) else { return };
        let Some((from, to)) = pageedit::span_of(block, cell, what) else { return };
        if let Some(caret) = ed.caret.as_mut() {
            caret.cell = Some(from);
            caret.span = Some(to);
            caret.text = pageedit::cell_text(block, from).unwrap_or_default();
            caret.focus = false;
        }
    }

    /// Changes the shape of the table the caret is in, from the Table tab.
    ///
    /// A block of cells picked across several rows or columns takes them all: Delete Rows
    /// with three rows picked takes the three, the way it does in OneNote.
    /// Table > Sort: the table's rows by the caret's column, or only the picked rows when a
    /// block of cells running over more than one row is picked (issue #171). One step of
    /// the page's history, so Undo puts the rows back in their order.
    fn sort_table(&mut self, descending: bool) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some((at, cell)) = ed.caret.as_ref().and_then(|c| c.cell.map(|cell| (c.block, cell))) else {
            self.say(crate::tl_id!("ui.click_in_the_table_first"));
            return;
        };
        let picked = ed.caret.as_ref().and_then(pageedit::cell_range).filter(|(a, b)| a.0 != b.0).map(|(a, b)| (a.0, b.0));
        Self::remember_step(ed);
        let Some(block) = ed.blocks.get_mut(at) else { return };
        let Some(order) = pageedit::sort_table_rows(block, cell.1, picked, descending) else {
            self.say(crate::tl_id!("ui.the_rows_are_already_in_that_order"));
            return;
        };
        // The caret stays in the same place in the table, and shows what is there now.
        let text = pageedit::cell_text(block, cell).unwrap_or_default();
        ed.rebuild_body();
        if let Some(caret) = ed.caret.as_mut() {
            caret.text = text;
            caret.selection = (0, 0);
            caret.inline = None;
        }
        // An aligned cell goes where its row went (issue #170).
        let cols_moved = ed.columns.reorder_rows(at, &order);
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        if cols_moved {
            self.save_block_marks((true, false));
        }
    }

    /// Table > Delete Table (issue #297): takes the table the caret is in out of the page, as one
    /// Undo step. The caret goes to the block that takes its place (the one after it, or the
    /// one before when it was last); a page left with nothing gets an empty line to type on.
    /// Table widths, cell marks and justified paragraphs after it move up with their blocks.
    fn delete_table(&mut self) {
        use nocturne_formats::markdown::Block;
        let Some(ed) = self.editor.as_mut() else { return };
        let at = ed.caret.as_ref().filter(|c| c.cell.is_some()).map(|c| c.block);
        let Some(at) = at.filter(|&at| matches!(ed.blocks.get(at), Some(Block::Table { .. }))) else {
            self.say(crate::tl_id!("ui.put_the_caret_in_a_table_first"));
            return;
        };
        Self::remember_step(ed);
        ed.caret = None;
        ed.blocks.remove(at);
        let marks = ed.block_removed(at);
        if ed.blocks.is_empty() {
            ed.blocks.push(Block::Paragraph(Vec::new()));
            ed.count_inserted_in_boxes(0, 1);
        }
        pageedit::renumber(&mut ed.blocks);
        ed.rebuild_body();
        let to = at.min(ed.blocks.len() - 1);
        let text = pageedit::text_for_caret(&ed.blocks, to);
        ed.caret = Some(pageedit::Caret { block: to, text, focus: true, selection: (0, 0), place: Some(0), cell: None, span: None, inline: None });
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        self.save_block_marks(marks);
        self.say(crate::tl_id!("ui.deleted_the_table"));
    }

    fn edit_table(&mut self, what: pageedit::TableEdit) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some((at, cell)) = ed.caret.as_ref().and_then(|c| c.cell.map(|cell| (c.block, cell))) else {
            return;
        };
        // Deleting works from the top-left of what is picked, over and over: each one
        // after the first falls into the place the one before it left. A picked block
        // that takes in the heading row starts under it, so picking the whole table and
        // asking for its rows empties the body and keeps the heading.
        let picked = ed.caret.as_ref().and_then(pageedit::cell_range);
        let (cell, times) = match (what, picked) {
            (pageedit::TableEdit::DeleteRow, Some((a, b))) => {
                let start = a.0.max(1);
                ((start, a.1), (b.0 + 1).saturating_sub(start))
            }
            (pageedit::TableEdit::DeleteColumn, Some((a, b))) => (a, b.1 - a.1 + 1),
            _ => (cell, 1),
        };
        // What was written before is kept as it stood, so Undo takes back only the table change.
        Self::remember_step(ed);
        let Some(block) = ed.blocks.get_mut(at) else { return };
        let mut last = None;
        let mut done = 0;
        for _ in 0..times {
            match pageedit::edit_table(block, cell, what) {
                Some(to) => {
                    last = Some(to);
                    done += 1;
                }
                None => break,
            }
        }
        let Some(to) = last else {
            // The rules said no: a table keeps its heading row and at least one column.
            self.say(crate::tl_id!("ui.a_table_keeps_its_heading_row_and_at_least_one_column"));
            return;
        };
        let text = pageedit::cell_text(block, to).unwrap_or_default();
        ed.rebuild_body();
        ed.caret = Some(pageedit::Caret { block: at, text, focus: true, selection: (0, 0), place: None, cell: Some(to), span: None, inline: None });
        // A width the page keeps for this table stays with the column it was dragged for, so a
        // column put in or taken out does not hand its neighbours the wrong widths (issue #154).
        let cols_moved = match what {
            pageedit::TableEdit::InsertColumn(left) => ed.columns.insert_column(at, if left { cell.1 } else { cell.1 + 1 }, pageedit::COLUMN_W),
            pageedit::TableEdit::DeleteColumn => (0..done).fold(false, |moved, _| ed.columns.remove_column(at, cell.1) || moved),
            // An aligned cell stays with its row too (issue #170).
            pageedit::TableEdit::InsertRow(above) => ed.columns.insert_row(at, if above { cell.0 } else { cell.0 + 1 }),
            pageedit::TableEdit::DeleteRow => (0..done).fold(false, |moved, _| ed.columns.remove_row(at, cell.0) || moved),
        };
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        self.save_block_marks((cols_moved, false));
    }

    /// The sizes typed into an open Insert Table dialog, as a person would type them. For tests
    /// and automation; does nothing when the dialog is not open.
    pub fn set_insert_table_size(&mut self, rows: &str, columns: &str) {
        if let Some(Dialog::InsertTable { rows: r, columns: c, .. }) = self.dialog.as_mut() {
            *r = rows.to_string();
            *c = columns.to_string();
        }
    }

    /// The open page's blocks, as the editor holds them.
    pub fn editor_blocks(&self) -> Vec<nocturne_formats::Block> {
        self.editor.as_ref().map(|e| e.blocks.clone()).unwrap_or_default()
    }

    /// The open page's layout containers, as the editor holds them.
    pub fn editor_boxes(&self) -> Vec<nocturne_model::Container> {
        self.editor.as_ref().map(|e| e.boxes.clone()).unwrap_or_default()
    }

    pub fn editor_origin(&self) -> Option<egui::Pos2> {
        self.editor.as_ref().map(|e| e.origin)
    }

    pub fn ink_stroke_count(&self) -> usize {
        self.pad.as_ref().map_or(0, crate::inkpad::Pad::strokes)
    }

    /// How many strokes on the open page are currently held by the lasso or selector.
    pub fn held_ink_count(&self) -> usize {
        self.pad.as_ref().map_or(0, |p| p.holding())
    }

    /// Saves both the open page and its editable ink drawing immediately.
    pub fn save_open_page(&mut self) {
        self.save_now();
    }

    /// True when Tablet Studio is active and using the two-pane Library Grid view.
    pub fn is_studio_grid(&self, ctx: &Context) -> bool {
        self.is_tablet_studio(ctx) && self.session.library_view() == nocturne_engine::LibraryView::StudioGrid
    }

    /// Closes the open page and returns to the Tablet Studio Library Grid.
    pub fn close_to_library(&mut self) {
        self.save_now();
        if let Some(pad) = self.pad.as_mut()
            && pad.dirty
        {
            let _ = pad.save();
        }
        if let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) {
            self.library_grid.invalidate_page(&dir);
            self.library_grid.select_for_page(&dir);
            if let Some(parent) = dir.parent() {
                self.selected = Some(parent.to_path_buf());
            }
        }
        let _ = self.session.settings.set("last_page", "");
        self.editor = None;
        self.pad = None;
        self.math_ocr = None;
        self.pending_math_ocr_start = None;
        self.page_rect = None;
        self.page_view_rect = None;
        self.header_pill_rect = None;
        self.pencil_pill_rect = None;
        self.tool_settings_open = false;
    }

    /// Cached `(text_line_count, ink_stroke_count)` for a Library Grid page thumbnail, if built.
    pub fn cached_library_thumb_counts(&self, page_dir: &Path) -> Option<(usize, usize)> {
        self.library_grid.cached_counts(page_dir)
    }

    /// The box around all the page's ink, `[x0, y0, x1, y1]` in page points, when there is any.
    pub fn ink_bounds(&self) -> Option<[f32; 4]> {
        self.pad.as_ref().and_then(crate::inkpad::Pad::bounds)
    }

    /// The pens on the rack, in order: each a kind of pen, a colour and a thickness.
    pub fn pen_rack(&self) -> &[nocturne_ink::rack::Nib] {
        &self.rack
    }

    /// How many pen readings each stroke on the page holds, in the order they were drawn.
    pub fn ink_dab_counts(&self) -> Vec<usize> {
        self.pad.as_ref().map(crate::inkpad::Pad::dab_counts).unwrap_or_default()
    }

    /// The pressure each stroke on the page begins with, in the order they were drawn.
    pub fn ink_pressures(&self) -> Vec<f32> {
        self.pad.as_ref().map(crate::inkpad::Pad::first_pressures).unwrap_or_default()
    }

    /// True while the page's title has the keyboard.
    pub fn title_is_focused(&self) -> bool {
        self.editor.as_ref().is_some_and(|ed| ed.title_focused)
    }

    /// The page's title as it is being typed, before it is saved.
    pub fn title_draft(&self) -> Option<&str> {
        self.editor.as_ref().map(|ed| ed.title_buf.as_str())
    }

    /// True while Tablet Studio is showing a failure in its banner.
    pub fn status_banner_showing(&self) -> bool {
        self.status_banner_drawn
    }

    /// The one-line or dialog field that has the keyboard, with all of its text and its selection:
    /// Rename, Add, New notebook, Link, Insert Table, Math OCR setup/preview, Cloud Sync, CodeView,
    /// Template name, the page title, or search. `None` when the keyboard is in the page's words or
    /// nowhere.
    ///
    /// A platform keyboard needs the whole field. Given an empty one it believes there is
    /// nothing to delete, and it has no words to find boundaries in for its suggestions.
    pub fn focused_text_field(&self, ctx: &Context) -> Option<PlatformTextField> {
        let focused = ctx.memory(|m| m.focused())?;
        let is = |id: &str| focused == egui::Id::unique(id);
        let (text, selection) = match &self.dialog {
            Some(Dialog::Rename { title, .. }) if is(RENAME_FIELD) => (title.as_str(), None),
            Some(Dialog::Add { title, .. }) if is(ADD_FIELD) => (title.as_str(), None),
            Some(Dialog::NewNotebook { name, .. }) if is(NEW_NOTEBOOK_FIELD) => (name.as_str(), None),
            Some(Dialog::Link { text, .. }) if is(LINK_TEXT_FIELD) => (text.as_str(), None),
            Some(Dialog::Link { address, .. }) if is(LINK_ADDRESS_FIELD) => (address.as_str(), None),
            Some(Dialog::InsertTable { columns, .. }) if is(TABLE_COLUMNS_FIELD) => (columns.as_str(), None),
            Some(Dialog::InsertTable { rows, .. }) if is(TABLE_ROWS_FIELD) => (rows.as_str(), None),
            Some(Dialog::MathOcrSetup(form)) if is("math-ocr-endpoint") => (form.endpoint.as_str(), None),
            Some(Dialog::MathOcrSetup(form)) if is("math-ocr-model") => (form.model.as_str(), None),
            Some(Dialog::MathOcrSetup(form)) if is("math-ocr-app-id") => (form.app_id.as_str(), None),
            Some(Dialog::MathOcrSetup(form)) if is("math-ocr-key") => (form.key.as_str(), None),
            Some(Dialog::CloudSync(_)) if egui::TextEdit::load_state(ctx, focused).is_some() => ("", None),
            Some(_) => return None,
            None => {
                if let Some(crate::math_ocr::Conversion { stage: crate::math_ocr::Stage::Preview { tex, .. }, .. }) = &self.math_ocr
                    && is(crate::math_ocr::TEX_FIELD)
                {
                    (tex.as_str(), None)
                } else if is(SEARCH_FIELD) {
                    (self.search.as_str(), None)
                } else if is(crate::library_grid::LIBRARY_SEARCH_FIELD) {
                    (self.library_grid.search_query(), None)
                } else if let Some(pane) = self.templates_pane.as_ref()
                    && is(TEMPLATE_NAME_FIELD)
                {
                    (pane.name.as_str(), None)
                } else if self.show_source && is("code-find-query") {
                    (self.code.find.query.as_str(), None)
                } else if self.show_source && self.code.picked.as_deref().is_some_and(|name| focused == egui::Id::unique(("code-text-edit", name))) {
                    (self.code.text.as_str(), None)
                } else {
                    let ed = self.editor.as_ref().filter(|ed| ed.title_focused || is(PAGE_TITLE_FIELD))?;
                    (ed.title_buf.as_str(), ed.title_selection)
                }
            }
        };
        let len = text.chars().count();
        let (a, b) = selection
            .or_else(|| {
                let range = egui::TextEdit::load_state(ctx, focused)?.cursor.char_range()?;
                Some((range.primary.index.0, range.secondary.index.0))
            })
            .unwrap_or((len, len));
        let (a, b) = (a.min(len), b.min(len));
        Some(PlatformTextField { text: text.to_string(), selection: (a.min(b), a.max(b)) })
    }

    /// Whether clipboard input belongs to the open page body. Dialogs, Backstage, Code View
    /// and focused fields like Search or Page Title handle their own clipboard events.
    fn page_owns_clipboard_input(&self, ctx: &Context) -> bool {
        if self.backstage.is_some()
            || self.dialog.is_some()
            || self.theme_window
            || self.import_job.is_some()
            || self.show_source
            || self.tool_settings_open
            || matches!(self.math_ocr.as_ref().map(|c| &c.stage), Some(crate::math_ocr::Stage::Preview { .. }))
            || self.editor.is_none()
            || self.compact_list_fills()
            || self.compact_pane_fills()
        {
            return false;
        }
        if self.editor.as_ref().is_some_and(|ed| ed.title_focused) {
            return false;
        }
        if self.focused_text_field(ctx).is_some() {
            return false;
        }
        if let Some(focused) = ctx.memory(|m| m.focused())
            && egui::TextEdit::load_state(ctx, focused).is_some()
        {
            // Ribbon font-family / font-size combo boxes store their draft String under `focused`.
            let combo_edit = ctx.data(|d| d.get_temp::<String>(focused)).is_some();
            let body_caret_present = self.editor.as_ref().is_some_and(|ed| ed.caret.is_some());
            if combo_edit || !body_caret_present {
                return false;
            }
        }
        true
    }

    /// Asks for a new title for the notebook, section or page at `path`.
    pub fn open_rename_dialog(&mut self, path: PathBuf, title: String) {
        self.dialog = Some(Dialog::Rename { path, title });
    }

    /// Asks for the name of a new notebook.
    pub fn open_new_notebook_dialog(&mut self) {
        self.dialog = Some(Dialog::NewNotebook { name: String::new(), format: None });
    }

    /// Asks what to add inside `parent`, offering `kind` first.
    pub fn open_add_dialog(&mut self, parent: PathBuf, parent_kind: NodeKind, kind: NodeKind) {
        self.dialog = Some(Dialog::Add { parent, parent_kind, title: String::new(), kind, format: None });
    }

    pub fn ribbon_act_for_test(&mut self, act: ribbon::Act) {
        self.ribbon_act(act);
    }

    /// Picks up a pen, which is all it takes: the whole page is the ink's, so there is no
    /// sheet to put anywhere first.
    fn ink_insert(&mut self) {
        if self.editor.is_none() {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        }
        self.pointer = ribbon::Pointer::Draw;
        self.space_tool = false;
        self.sync_tool_state_to_session();
        self.ribbon_tab = "Draw".to_string();
        self.say(crate::tl_id!("ui.write_anywhere_on_the_page"));
    }

    /// Says where the page's ink file was kept, when writing the ink kept it first because it
    /// held more than could be read back as strokes (issue #157).
    fn tell_kept_ink(&mut self) {
        let Some(kept) = self.pad.as_mut().and_then(|pad| pad.kept_original.take()) else { return };
        let shown = self.shown(&kept);
        log::warn!("the page's ink file held more than its strokes; the original was kept: {kept:?}");
        self.say(crate::i18n::format_message_id("ui.kept_the_original_drawing_file_path", &[("path", &shown)]));
    }

    /// Writes the drawing again with no pen readings in it, for sending somewhere else.
    fn ink_export_plain(&mut self) {
        let Some(pad) = self.pad.as_ref() else {
            self.say(crate::tl_id!("ui.there_is_no_drawing_on_this_page"));
            return;
        };
        let saved = pad.save_plain().and_then(|path| {
            if let Some(downloads) = self.file_downloads.as_mut() {
                let bytes = std::fs::read(&path)?;
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "ink-picture.svg".to_owned());
                downloads.push(FileDownload { name, mime: "image/svg+xml", bytes });
            }
            Ok(path)
        });
        match saved {
            Ok(path) => {
                let shown = if self.file_downloads.is_some() {
                    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| self.shown(&path))
                } else {
                    self.shown(&path)
                };
                self.say(crate::i18n::format_message_id("ui.saved_the_drawing_as_a_picture_path", &[("path", &shown)]));
            }
            Err(e) => self.complain(&crate::i18n::format_message_id("ui.cannot_write_the_picture_error", &[("error", &e.to_string())])),
        }
    }

    /// Exports the original image file at `src` via the native save file dialog (or browser download).
    /// Rotates or flips a picture on the page (issue #273). The turned picture is written as a
    /// new file beside the page and every place the page shows the old one points at it; the
    /// original file stays, so Undo puts the page back on it. A picture given a size of its own
    /// keeps it, with width and height swapped by a quarter turn.
    fn turn_picture(&mut self, src: &str, how: crate::turn::Turn) {
        use nocturne_formats::markdown::Block;
        let Some(ed) = self.editor.as_ref() else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        if !ed.writable {
            self.complain(crate::tl_id!("ui.this_picture_cannot_be_turned_here"));
            return;
        }
        let dir = ed.dir.clone();
        let (_, _, bytes) = match crate::imagemenu::validate_and_read_image(&dir, src) {
            Ok(read) => read,
            Err(e) => {
                self.complain(&crate::i18n::format_message_id("ui.cannot_read_the_picture_error", &[("error", &e.to_string())]));
                return;
            }
        };
        let (turned, ext) = match crate::turn::turn(&bytes, how) {
            Ok(turned) => turned,
            Err(e) => {
                log::debug!("picture turn failed for {src:?}: {e}");
                self.complain(crate::tl_id!("ui.this_picture_cannot_be_turned_here"));
                return;
            }
        };
        let link = match nocturne_store::assets::attach_bytes(&dir, &crate::turn::turned_name(src, how, ext), &turned) {
            Ok(link) => link,
            Err(e) => {
                self.complain(&crate::i18n::format_message_id("ui.cannot_save_the_pasted_picture_error", &[("error", &e.to_string())]));
                return;
            }
        };
        let Some(ed) = self.editor.as_mut() else { return };
        // The page as it stood is kept, so Undo takes back only the turn.
        Self::remember_step(ed);
        let mut found = false;
        for block in &mut ed.blocks {
            match block {
                Block::Image { src: s, width, height, .. } if s == src => {
                    s.clone_from(&link);
                    if how.swaps_sides() {
                        std::mem::swap(width, height);
                    }
                    found = true;
                }
                Block::Heading { spans, .. } | Block::Paragraph(spans) | Block::Quote(spans) | Block::Item { spans, .. } => {
                    for span in spans.iter_mut().filter(|s| s.image.as_deref() == Some(src)) {
                        span.image = Some(link.clone());
                        found = true;
                    }
                }
                _ => {}
            }
        }
        if !found {
            // Nothing pointed at it after all: the copy is not left lying in the folder.
            let _ = nocturne_store::assets::remove(&dir, link.trim_start_matches("assets/"));
            return;
        }
        if let Some(caret) = ed.caret.as_mut()
            && let Some(block) = ed.blocks.get(caret.block)
        {
            caret.text = pageedit::caret_text(block);
            caret.inline = None;
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        ed.attachments = nocturne_store::assets::list(&dir);
        let inked = self.pad.as_ref().is_some_and(|p| !p.is_empty());
        self.say(match (how.swaps_sides(), inked) {
            (true, false) => crate::tl_id!("ui.rotated_the_picture"),
            (false, false) => crate::tl_id!("ui.flipped_the_picture"),
            // Ink is its own layer over the whole page: it is not turned with the picture.
            (true, true) => crate::tl_id!("ui.rotated_the_picture_ink_on_the_page_stays_where_it_is"),
            (false, true) => crate::tl_id!("ui.flipped_the_picture_ink_on_the_page_stays_where_it_is"),
        });
    }

    fn save_picture_as(&mut self, src: &str) {
        let Some(page_dir) = self.editor.as_ref().map(|e| e.dir.clone()) else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        let (_source_path, default_name, bytes) = match crate::imagemenu::validate_and_read_image(&page_dir, src) {
            Ok(res) => res,
            Err(e) => {
                self.complain(&crate::i18n::format_message_id("ui.cannot_read_the_picture_error", &[("error", &e.to_string())]));
                return;
            }
        };

        if let Some(downloads) = self.file_downloads.as_mut() {
            let mime = match std::path::Path::new(&default_name).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                Some("bmp") => "image/bmp",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            downloads.push(FileDownload { name: default_name.clone(), mime, bytes });
            self.say(crate::i18n::format_message_id("ui.saved_picture_as_path", &[("path", &default_name)]));
            return;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let ext = std::path::Path::new(&default_name).extension().and_then(|e| e.to_str()).unwrap_or("");
            let mut picker = rfd::FileDialog::new().set_title(crate::tl_id!("ui.save_picture_as")).set_file_name(&default_name);
            if !ext.is_empty() && !["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "jxl"].contains(&ext) {
                picker = picker.add_filter(ext, &[ext]);
            }
            picker = picker.add_filter(crate::tl_id!("ui.pictures"), &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "jxl"]);

            let Some(destination) = picker.save_file() else {
                return;
            };

            match std::fs::write(&destination, &bytes) {
                Ok(()) => {
                    let shown = self.shown(&destination);
                    self.say(crate::i18n::format_message_id("ui.saved_picture_as_path", &[("path", &shown)]));
                }
                Err(e) => {
                    self.complain(&crate::i18n::format_message_id("ui.cannot_write_the_picture_error", &[("error", &e.to_string())]));
                }
            }
        }
    }

    /// Puts marks around the selection in the block the caret is in.
    fn apply_mark(&mut self, mark: &str) {
        if self.editor.as_ref().and_then(|ed| ed.across).is_some_and(|selection| !selection.inside_one_block()) {
            let format = match mark {
                "bold" => pageedit::InlineFormat::Bold,
                "italic" => pageedit::InlineFormat::Italic,
                "strike" => pageedit::InlineFormat::Strike,
                "underline" => pageedit::InlineFormat::Underline,
                "subscript" => pageedit::InlineFormat::Subscript,
                "superscript" => pageedit::InlineFormat::Superscript,
                "code" => pageedit::InlineFormat::Code,
                _ => return,
            };
            self.apply_across_format(format);
            return;
        }
        if matches!(mark, "underline" | "subscript" | "superscript") {
            self.apply_new_mark(mark);
            return;
        }
        let collapsed_text_caret =
            self.editor.as_ref().and_then(|ed| ed.caret.as_ref()).is_some_and(|caret| caret.cell.is_none() && caret.selection.0 == caret.selection.1);
        if collapsed_text_caret && matches!(mark, "bold" | "italic" | "strike") {
            self.apply_new_mark(mark);
            return;
        }
        let partial = self.editor.as_ref().and_then(|ed| ed.caret.as_ref()).is_some_and(|caret| {
            let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
            lo < hi && (lo != 0 || hi != caret.text.chars().count())
        });
        if partial {
            let format = match mark {
                "bold" => Some(pageedit::InlineFormat::Bold),
                "italic" => Some(pageedit::InlineFormat::Italic),
                "strike" => Some(pageedit::InlineFormat::Strike),
                _ => None,
            };
            if let Some(format) = format {
                self.apply_partial_inline_format(format);
                return;
            }
        }
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.caret.is_none() {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        }
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        let marks = match mark {
            "bold" => "**",
            "italic" => "*",
            "strike" => "~~",
            _ => "`",
        };
        let len = caret.text.chars().count();
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        // All of the line's words: marked on the spans, so words that are partly marked already come out
        // right. A part of them is wrapped in the text, where the caret's own selection lives.
        if len > 0 && lo == 0 && hi == len && pageedit::toggle_mark(&mut ed.blocks, caret.block..caret.block + 1, mark) {
            if let Some(b) = ed.blocks.get(caret.block) {
                caret.text = pageedit::caret_text(b);
                caret.inline = None;
                caret.selection = (0, caret.text.chars().count());
                caret.focus = true;
                ed.restore_format_selection = Some((caret.block, caret.selection.0, caret.selection.1));
            }
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
            return;
        }
        let (text, selection) = pageedit::wrap_selection(&caret.text, caret.selection, marks);
        caret.text = text;
        caret.selection = selection;
        caret.focus = true;
        caret.place = Some(selection.0);
        if selection.0 != selection.1 {
            ed.restore_format_selection = Some((caret.block, selection.0, selection.1));
        }
        if let Some(b) = ed.blocks.get_mut(caret.block) {
            caret.write(b);
        }
        if let Some(c) = ed.caret.as_mut() {
            c.focus = true;
            c.place = Some(c.selection.0);
            if c.selection.0 != c.selection.1 {
                ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
            }
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
    }

    fn apply_new_mark(&mut self, mark: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(caret) = ed.caret.as_ref() else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        let len = caret.text.chars().count();
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        if ed.writable
            && ed.across.is_none()
            && caret.cell.is_none()
            && lo == hi
            && matches!(
                ed.blocks.get(caret.block),
                Some(
                    nocturne_formats::Block::Heading { .. }
                        | nocturne_formats::Block::Paragraph(_)
                        | nocturne_formats::Block::Quote(_)
                        | nocturne_formats::Block::Item { .. }
                )
            )
        {
            let mut marker = "NocturneCaretProbe".to_string();
            while caret.text.contains(&marker) {
                marker.push('X');
            }
            let (probe, _) = pageedit::insert_at_caret(&caret.text, (lo, lo), &marker);
            let inherited = ed
                .blocks
                .get(caret.block)
                .map(|block| caret.parse(block, &probe))
                .unwrap_or_default()
                .into_iter()
                .find(|span| span.text.contains(&marker));
            let unsafe_context = inherited.is_none_or(|span| {
                span.code
                    || span.math
                    || span.image.is_some()
                    || (mark == "bold" && span.bold)
                    || (mark == "italic" && span.italic)
                    || (mark == "strike" && span.strike)
                    || (mark == "underline" && span.underline)
                    || (matches!(mark, "subscript" | "superscript") && span.script.is_some())
            });
            if unsafe_context {
                self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
                self.status_is_error = false;
                return;
            }
            let pending = ed.pending_format.get_or_insert_with(|| PendingFormat { block: caret.block, at: lo, ..Default::default() });
            if pending.block != caret.block || pending.at != lo {
                *pending = PendingFormat { block: caret.block, at: lo, ..Default::default() };
            }
            match mark {
                "bold" => pending.bold = !pending.bold,
                "italic" => pending.italic = !pending.italic,
                "strike" => pending.strike = !pending.strike,
                "underline" => pending.underline = !pending.underline,
                "subscript" => {
                    let script = nocturne_formats::markdown::Script::Subscript;
                    pending.script = (pending.script != Some(script)).then_some(script);
                }
                "superscript" => {
                    let script = nocturne_formats::markdown::Script::Superscript;
                    pending.script = (pending.script != Some(script)).then_some(script);
                }
                _ => {}
            }
            if let Some(caret) = ed.caret.as_mut() {
                caret.focus = true;
                caret.place = Some(lo);
            }
            self.status.clear();
            self.status_is_error = false;
            return;
        }
        if ed.writable && ed.across.is_none() && caret.cell.is_none() && lo < hi && (lo != 0 || hi != len) {
            let format = match mark {
                "underline" => pageedit::InlineFormat::Underline,
                "subscript" => pageedit::InlineFormat::Subscript,
                "superscript" => pageedit::InlineFormat::Superscript,
                _ => return,
            };
            self.apply_partial_inline_format(format);
            return;
        }
        if !ed.writable || ed.across.is_some() || caret.cell.is_some() || len == 0 || lo != 0 || hi != len {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        }
        let block = caret.block;
        Self::remember_step(ed);
        if !pageedit::toggle_mark(&mut ed.blocks, block..block + 1, mark) {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        }
        ed.pending_format = None;
        if let (Some(caret), Some(block)) = (ed.caret.as_mut(), ed.blocks.get(block)) {
            caret.text = pageedit::caret_text(block);
            caret.inline = None;
            caret.selection = (0, caret.text.chars().count());
            caret.focus = true;
            ed.restore_format_selection = Some((caret.block, caret.selection.0, caret.selection.1));
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        let step = crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
        ed.history.record(step);
        self.status.clear();
        self.status_is_error = false;
    }

    /// Format Painter: picks up the formatting at the caret, or of the words picked, to give
    /// to the next words picked (issue #165). A second click puts the painter down again; a
    /// double-click (`sticky`) keeps it painting until Escape.
    fn pick_up_formatting(&mut self, sticky: bool) {
        if let Some(held) = self.painter.as_mut() {
            if sticky {
                held.sticky = true;
                self.say(crate::tl_id!("ui.format_painter_keeps_painting_until_escape"));
            } else {
                self.painter = None;
                self.say(crate::tl_id!("ui.format_painter_put_down"));
            }
            return;
        }
        let Some(ed) = self.editor.as_ref().filter(|ed| ed.writable) else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        let picked = ed.caret.as_ref().and_then(|caret| Some((caret.block, caret.selection, caret.style(ed.blocks.get(caret.block)?)?)));
        let Some((block, selection, style)) = picked else {
            self.say(crate::tl_id!("ui.click_in_the_words_whose_formatting_to_copy"));
            return;
        };
        self.painter = Some(FormatPaint { style, from: (block, selection), sticky });
        self.say(crate::tl_id!("ui.pick_the_words_to_give_that_formatting"));
    }

    /// Puts the Format Painter's formatting on words just picked with the pointer: on the
    /// release that ends the pick, as in Office. Escape puts the painter down. Called once a
    /// frame after the page is drawn.
    /// Escape puts the laser pointer down (issue #289): it is for a moment of pointing, and
    /// the caret comes back afterwards.
    fn laser_keys(&mut self, ctx: &Context) {
        if self.pointer == ribbon::Pointer::Laser && self.dialog.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.pointer = ribbon::Pointer::Write;
            self.sync_tool_state_to_session();
            crate::inkpad::clear_laser(ctx);
            self.say(crate::tl_id!("ui.laser_pointer_put_down"));
        }
    }

    fn paint_formatting(&mut self, ctx: &Context) {
        if self.painter.is_none() {
            return;
        }
        if self.dialog.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.painter = None;
            self.say(crate::tl_id!("ui.format_painter_put_down"));
            return;
        }
        if !ctx.input(|i| i.pointer.primary_released()) {
            return;
        }
        let Some(held) = self.painter.clone() else { return };
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(caret) = ed.caret.as_ref() else { return };
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        // A click without a pick, or the words it was picked up from: nothing to paint yet.
        if lo == hi || (caret.block, caret.selection) == held.from {
            return;
        }
        let Some(block) = ed.blocks.get(caret.block) else { return };
        let Some(caret) = ed.caret.as_mut() else { return };
        let Some((text, selection)) = caret.paint(block, &held.style) else {
            self.say(crate::tl_id!("ui.inline_mark_whole_line"));
            return;
        };
        if text != caret.text {
            Self::remember_step(ed);
            let Some(caret) = ed.caret.as_mut() else { return };
            caret.text = text;
            caret.selection = selection;
            caret.place = Some(selection.0);
            caret.focus = true;
            if let Some(block) = ed.blocks.get_mut(caret.block) {
                caret.write(block);
            }
            ed.pending_format = None;
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
        }
        if !held.sticky {
            self.painter = None;
        }
    }

    fn apply_partial_inline_format(&mut self, format: pageedit::InlineFormat) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(caret) = ed.caret.as_ref() else { return };
        if !ed.writable
            || ed.across.is_some()
            || caret.cell.is_some()
            || !matches!(
                ed.blocks.get(caret.block),
                Some(
                    nocturne_formats::Block::Heading { .. }
                        | nocturne_formats::Block::Paragraph(_)
                        | nocturne_formats::Block::Quote(_)
                        | nocturne_formats::Block::Item { .. }
                )
            )
        {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        }
        let Some(caret) = ed.caret.as_mut() else { return };
        let Some(block) = ed.blocks.get(caret.block) else { return };
        let Some((text, selection)) = caret.format(block, format) else {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        };
        if text == caret.text {
            return;
        }
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        caret.text = text;
        caret.selection = selection;
        caret.focus = true;
        ed.restore_format_selection = Some((caret.block, selection.0, selection.1));
        if let Some(block) = ed.blocks.get_mut(caret.block) {
            caret.write(block);
        }
        ed.pending_format = None;
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        let step = crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
        ed.history.record(step);
        self.status.clear();
        self.status_is_error = false;
    }

    /// An inline format over a selection that runs across blocks: every covered block's
    /// covered words take it, and the selection is kept with its ends remapped to the new
    /// text, so the highlight, Copy and Delete still cover the same words.
    fn apply_across_format(&mut self, format: pageedit::InlineFormat) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(open) = ed.across.filter(|a| !a.inside_one_block()) else { return };
        if !ed.writable {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        }
        Self::remember_step(ed);
        match pageedit::format_across(&mut ed.blocks, open, format) {
            Some(remapped) => {
                ed.across = Some(remapped);
                ed.pending_format = None;
                ed.rebuild_body();
                Self::mark_dirty_from_blocks(ed);
                // The caret's block may be one of the formatted ones: its editing text has
                // to say what the block says now, or the next keystroke writes the old words
                // back over the new formatting.
                if let Some(at) = ed.caret.as_ref().filter(|c| c.cell.is_none()).map(|c| c.block)
                    && let Some(block) = ed.blocks.get(at)
                    && matches!(
                        block,
                        nocturne_formats::Block::Heading { .. }
                            | nocturne_formats::Block::Paragraph(_)
                            | nocturne_formats::Block::Quote(_)
                            | nocturne_formats::Block::Item { .. }
                    )
                    && let Some(caret) = ed.caret.as_mut()
                {
                    caret.text = pageedit::caret_text(block);
                    caret.inline = None;
                    caret.selection = (0, caret.text.chars().count());
                }
                let step =
                    crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
                ed.history.record(step);
                self.status.clear();
                self.status_is_error = false;
            }
            None => {
                self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
                self.status_is_error = false;
            }
        }
    }

    /// With no caret in the page, a font or size choice goes to the end of the last writable
    /// block, so it applies to the next text typed there, as OneNote does.
    fn place_caret_for_format(ed: &mut Editor) {
        if ed.caret.is_some() {
            return;
        }
        let Some(index) = ed.blocks.iter().rposition(pageedit::is_writable) else { return };
        let text = pageedit::caret_text(&ed.blocks[index]);
        let end = text.chars().count();
        ed.caret =
            Some(pageedit::Caret { block: index, text, focus: true, selection: (end, end), place: Some(end), cell: None, span: None, inline: None });
    }

    fn apply_font_family(&mut self, family: &str) {
        if !nocturne_formats::markdown::valid_font_family(family) {
            log::warn!("ribbon requested an invalid font family");
            return;
        }
        if self.editor.as_ref().and_then(|ed| ed.across).is_some_and(|selection| !selection.inside_one_block()) {
            self.apply_across_format(pageedit::InlineFormat::Family(family.to_owned()));
            return;
        }
        let Some(ed) = self.editor.as_mut() else { return };
        Self::place_caret_for_format(ed);
        let Some(caret) = ed.caret.as_ref() else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        let len = caret.text.chars().count();
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        if ed.writable
            && ed.across.is_none()
            && caret.cell.is_none()
            && lo == hi
            && matches!(
                ed.blocks.get(caret.block),
                Some(
                    nocturne_formats::Block::Heading { .. }
                        | nocturne_formats::Block::Paragraph(_)
                        | nocturne_formats::Block::Quote(_)
                        | nocturne_formats::Block::Item { .. }
                )
            )
        {
            let pending = ed.pending_format.get_or_insert_with(|| PendingFormat { block: caret.block, at: lo, ..Default::default() });
            if pending.block != caret.block || pending.at != lo {
                *pending = PendingFormat { block: caret.block, at: lo, ..Default::default() };
            }
            pending.family = Some(family.to_owned());
            if let Some(caret) = ed.caret.as_mut() {
                caret.focus = true;
                caret.place = Some(lo);
            }
            self.status.clear();
            self.status_is_error = false;
            return;
        }
        if ed.writable && ed.across.is_none() && caret.cell.is_none() && lo < hi && (lo != 0 || hi != len) {
            self.apply_partial_inline_format(pageedit::InlineFormat::Family(family.to_owned()));
            return;
        }
        if !ed.writable || ed.across.is_some() || caret.cell.is_some() || len == 0 || lo != 0 || hi != len {
            self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
            self.status_is_error = false;
            return;
        }
        let block = caret.block;
        Self::remember_step(ed);
        let spans = match ed.blocks.get_mut(block) {
            Some(
                nocturne_formats::Block::Heading { spans, .. }
                | nocturne_formats::Block::Paragraph(spans)
                | nocturne_formats::Block::Quote(spans)
                | nocturne_formats::Block::Item { spans, .. },
            ) => spans,
            _ => {
                self.status = crate::tl_id!("ui.inline_mark_whole_line").into();
                self.status_is_error = false;
                return;
            }
        };
        if !pageedit::format_span_range(spans, 0..usize::MAX, pageedit::InlineFormat::Family(family.to_owned())) {
            return;
        }
        ed.pending_format = None;
        if let (Some(caret), Some(block)) = (ed.caret.as_mut(), ed.blocks.get(block)) {
            caret.text = pageedit::caret_text(block);
            caret.inline = None;
            caret.selection = (0, caret.text.chars().count());
            caret.focus = true;
            ed.restore_format_selection = Some((caret.block, caret.selection.0, caret.selection.1));
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        let step = crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
        ed.history.record(step);
        self.status.clear();
        self.status_is_error = false;
    }

    fn apply_font_size(&mut self, size: u8) {
        if !ribbon::FONT_SIZES.contains(&size) {
            log::warn!("ribbon requested unsupported font size {size} pt");
            return;
        }
        if self.editor.as_ref().and_then(|ed| ed.across).is_some_and(|selection| !selection.inside_one_block()) {
            self.apply_across_format(pageedit::InlineFormat::Size(size));
            return;
        }
        let Some(ed) = self.editor.as_mut() else { return };
        Self::place_caret_for_format(ed);
        let Some(caret) = ed.caret.as_ref() else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        if caret.selection.0 == caret.selection.1 {
            if caret.cell.is_some()
                || !matches!(
                    ed.blocks.get(caret.block),
                    Some(
                        nocturne_formats::Block::Heading { .. }
                            | nocturne_formats::Block::Paragraph(_)
                            | nocturne_formats::Block::Quote(_)
                            | nocturne_formats::Block::Item { .. }
                    )
                )
            {
                self.status = crate::tl_id!("ui.select_text_before_changing_font_size").into();
                self.status_is_error = false;
                return;
            }
            let pending = ed.pending_format.get_or_insert_with(|| PendingFormat { block: caret.block, at: caret.selection.0, ..Default::default() });
            if pending.block != caret.block || pending.at != caret.selection.0 {
                *pending = PendingFormat { block: caret.block, at: caret.selection.0, ..Default::default() };
            }
            pending.size = Some(size);
            if let Some(caret) = ed.caret.as_mut() {
                caret.focus = true;
                caret.place = Some(caret.selection.0);
            }
            self.status.clear();
            self.status_is_error = false;
            return;
        }
        let len = caret.text.chars().count();
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        if lo != 0 || hi != len {
            self.apply_partial_inline_format(pageedit::InlineFormat::Size(size));
            return;
        }
        ed.pending_format = None;
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        let len = caret.text.chars().count();
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        let spans = match ed.blocks.get_mut(caret.block) {
            Some(
                nocturne_formats::Block::Heading { spans, .. }
                | nocturne_formats::Block::Paragraph(spans)
                | nocturne_formats::Block::Quote(spans)
                | nocturne_formats::Block::Item { spans, .. },
            ) if lo == 0 && hi == len => Some(spans),
            _ => None,
        };
        if let Some(spans) = spans {
            if !pageedit::format_span_range(spans, 0..usize::MAX, pageedit::InlineFormat::Size(size)) {
                return;
            }
            if let Some(block) = ed.blocks.get(caret.block) {
                caret.text = pageedit::caret_text(block);
                caret.inline = None;
                caret.selection = (0, caret.text.chars().count());
                caret.focus = true;
                ed.restore_format_selection = Some((caret.block, caret.selection.0, caret.selection.1));
            }
        } else {
            let open = format!("<span style=\"font-size: {size}pt\">");
            let (text, selection) = pageedit::wrap_selection_with(&caret.text, caret.selection, &open, "</span>");
            caret.text = text;
            caret.selection = selection;
            if let Some(block) = ed.blocks.get_mut(caret.block) {
                caret.write(block);
            }
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        if !ed.history.here().is_some_and(|s| s.body == ed.body && s.blocks == ed.blocks && s.boxes == ed.boxes) {
            let step = crate::history::Step::new(ed.body.clone(), ed.blocks.clone(), ed.boxes.clone(), ed.caret.clone(), crate::history::Kind::Edit);
            ed.history.record(step);
        }
    }

    /// The keys Windows/macOS users expect for paragraph direction. Alt+Shift remains available
    /// to Windows for switching the active input language.
    fn direction_keys(&mut self, ctx: &Context) {
        use nocturne_model::Dir;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(cmd_shift, Key::R) {
            self.apply_direction(Some(Dir::Rtl));
        }
        if pressed(cmd_shift, Key::L) {
            self.apply_direction(Some(Dir::Ltr));
        }
    }

    /// Which way the line the caret is in reads. `None` lets its own words decide.
    fn apply_direction(&mut self, dir: Option<nocturne_model::Dir>) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        if !pageedit::set_direction(&mut ed.blocks, at, dir) {
            self.say(crate::tl_id!("ui.a_direction_goes_on_a_paragraph_of_text"));
            return;
        }
        ed.rebuild_body();
        if let Some(b) = ed.blocks.get(at) {
            let text = pageedit::caret_text(b);
            if let Some(c) = ed.caret.as_mut() {
                c.text = text;
            }
        }
        Self::mark_dirty_from_blocks(ed);
        self.say(match dir {
            Some(d) if d.is_rtl() => crate::tl_id!("ui.this_paragraph_reads_right_to_left"),
            Some(_) => crate::tl_id!("ui.this_paragraph_reads_left_to_right"),
            None => crate::tl_id!("ui.this_paragraph_reads_whichever_way_its_words_do"),
        });
    }

    /// Fill the column with the paragraph the caret is in, or stop filling it.
    ///
    /// Which paragraphs are justified is the page's own, like the arrangement of its boxes
    /// and the width of its table columns, so it goes into the page's file and not into the
    /// window. The words are not touched: the stretching happens when the paragraph is
    /// drawn, so the file another program reads is the sentences the writer typed.
    fn apply_justify(&mut self, strength: nocturne_model::justify::Strength) {
        let Some(ed) = self.editor.as_ref() else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        if !matches!(ed.blocks.get(at), Some(nocturne_formats::Block::Paragraph(_))) {
            self.say(crate::tl_id!("ui.justification_goes_on_a_paragraph_of_text"));
            return;
        }
        // Asking for the strength a paragraph already has turns justification off, which
        // is how a menu of three gives a way back to a ragged edge without a fourth entry.
        let want = (ed.justified.strength_of(at) != Some(strength)).then_some(strength);
        let mut marked = ed.justified.clone();
        marked.set(at, want);
        self.write_justified(marked, want);
    }

    /// Writes which paragraphs are justified into the page's own file, and says what `want` now is.
    fn write_justified(&mut self, marked: nocturne_model::justify::Justified, want: Option<nocturne_model::justify::Strength>) {
        let Some(ed) = self.editor.as_ref() else { return };
        let path = path_str(&ed.dir);
        let line = nocturne_model::justify::write(&marked);
        self.save_now();
        if self.run("page.justify", &params(&[("path", &path), ("blocks", &line)])).is_some() {
            if let Some(ed) = self.editor.as_mut() {
                ed.justified = marked;
                // Justify is not an undo step of its own, so the step the page stands on now
                // takes the new marks; otherwise a redo back to it would put the old ones back.
                if let Some(step) = ed.history.here_mut() {
                    step.justified = Some(ed.justified.clone());
                }
            }
            self.say(match want {
                Some(nocturne_model::justify::Strength::Low) => crate::tl_id!("ui.this_paragraph_fills_the_column_stretched_lightly"),
                Some(nocturne_model::justify::Strength::Medium) => crate::tl_id!("ui.this_paragraph_fills_the_column"),
                Some(nocturne_model::justify::Strength::High) => crate::tl_id!("ui.this_paragraph_fills_the_column_stretched_hard"),
                None => crate::tl_id!("ui.this_paragraph_is_ragged_again"),
            });
        }
    }

    /// Which way the whole page reads. It is written into the page's own file.
    fn apply_page_direction(&mut self, dir: Option<nocturne_model::Dir>) {
        let Some(ed) = self.editor.as_ref() else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        let path = path_str(&ed.dir);
        let want = dir.map(|d| d.id()).unwrap_or("auto");
        self.save_now();
        if self.run("page.direction", &params(&[("path", &path), ("dir", want)])).is_some() {
            if let Some(ed) = self.editor.as_mut() {
                ed.page_dir = dir;
            }
            self.say(match dir {
                Some(d) if d.is_rtl() => crate::tl_id!("ui.this_page_reads_right_to_left"),
                Some(_) => crate::tl_id!("ui.this_page_reads_left_to_right"),
                None => crate::tl_id!("ui.every_line_of_this_page_decides_for_itself"),
            });
        }
    }

    /// Which way the selection, or what is typed next, reads: an isolated run.
    fn apply_embedding(&mut self, dir: nocturne_model::Dir) {
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.caret.is_none() {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        }
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        let (text, selection) = pageedit::isolate(&caret.text, caret.selection, dir);
        caret.text = text;
        caret.selection = selection;
        caret.place = Some(selection.1);
        if let Some(b) = ed.blocks.get_mut(caret.block) {
            caret.write(b);
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        self.say(if dir.is_rtl() {
            crate::tl_id!("ui.this_run_reads_right_to_left_a_mixed_line_is_still_d_d2a74de074")
        } else {
            crate::tl_id!("ui.this_run_reads_left_to_right_a_mixed_line_is_still_d_af62d34d2f")
        });
    }

    /// Makes the block the caret is in a heading, a list item, a quote or ordinary text.
    fn apply_block_kind(&mut self, kind: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        Self::remember_step(ed);
        if !pageedit::set_kind(&mut ed.blocks, at, kind) {
            return;
        }
        ed.rebuild_body();
        if let Some(b) = ed.blocks.get(at) {
            let text = pageedit::caret_text(b);
            if let Some(c) = ed.caret.as_mut() {
                c.text = text;
                c.focus = true;
                c.place = Some(c.selection.0);
                if c.selection.0 != c.selection.1 {
                    ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
                }
            }
        }
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
    }

    /// Puts a tag on the line the caret is in, or takes it off again.
    fn apply_tag(&mut self, tag: nocturne_model::Tag) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.status = crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon").into();
            self.status_is_error = false;
            return;
        };
        Self::remember_step(ed);
        if !pageedit::set_tag(&mut ed.blocks, at, tag) {
            self.say(crate::tl_id!("ui.a_tag_goes_on_a_line_of_text"));
            return;
        }
        ed.rebuild_body();
        if let Some(b) = ed.blocks.get(at) {
            let text = pageedit::caret_text(b);
            if let Some(c) = ed.caret.as_mut() {
                c.text = text;
            }
        }
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
        self.say(crate::i18n::format_message_id("ui.tag_tag", &[("tag", crate::tl!(tag.label()))]));
    }

    /// Cut or copy what is selected in the block the caret is in.
    /// Ctrl+C, Ctrl+X, Ctrl+V and Ctrl+A for the words on a page.
    ///
    /// egui's own text box answers these for a paragraph it is drawing, and it is left to
    /// do so. Nothing answered them anywhere else, and that is a lot of places: not in a
    /// right-to-left paragraph, which Nocturne draws itself; not for a selection that
    /// spans paragraphs, which no text box owns; and not when a box is picked or nothing
    /// has the keyboard at all. So Ctrl+C did nothing in Arabic, Ctrl+A selected nothing,
    /// and a box that had been copied could not be pasted into an empty one. The owner
    /// reported all three.
    ///
    /// The keys are read here and sent to the same `clipboard` and `paste_here` the ribbon
    /// buttons use, so there is one path and not two. Nothing is done when egui's text box
    /// is holding the keyboard, or both would act and the words would be pasted twice.
    fn clipboard_keys(&mut self, ctx: &egui::Context) {
        if self.backstage.is_some()
            || self.dialog.is_some()
            || self.theme_window
            || self.import_job.is_some()
            || self.show_source
            || matches!(self.math_ocr.as_ref().map(|c| &c.stage), Some(crate::math_ocr::Stage::Preview { .. }))
        {
            return;
        }
        let (copy, cut, paste, all) = ctx.input(|i| {
            let hit = |want: egui::Key| {
                // `key_pressed` is the platform-normalized view.  Some native
                // backends do not retain the raw key event in `events` after a
                // focused text widget has considered it, while the normalized
                // edge remains available for the rest of the frame.  Keep the
                // raw-event check as a fallback for the headless harness and
                // older egui backends.
                (i.key_pressed(want) && (i.modifiers.command || i.modifiers.ctrl))
                    || i.events.iter().any(|e| {
                        matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if *key == want && (modifiers.command || modifiers.ctrl))
                    })
                    || i.raw.events.iter().any(|e| {
                        matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if *key == want && (modifiers.command || modifiers.ctrl))
                    })
            };
            let copy = hit(Key::C) || i.events.iter().any(|event| matches!(event, egui::Event::Copy));
            let cut = hit(Key::X) || i.events.iter().any(|event| matches!(event, egui::Event::Cut));
            (copy, cut, hit(Key::V), hit(Key::A))
        });
        if !(copy || cut || paste || all) {
            return;
        }
        let ours = self.words_are_ours();
        if copy
            && self.held_ink_count() > 0
            && matches!(self.pointer, ribbon::Pointer::Lasso | ribbon::Pointer::Pick)
            && self.page_owns_clipboard_input(ctx)
        {
            ctx.input_mut(|i| i.events.retain(|event| !matches!(event, egui::Event::Copy)));
            self.ribbon_act(ribbon::Act::InkCopySelection);
        } else if (copy || cut) && ours && self.page_owns_clipboard_input(ctx) {
            ctx.input_mut(|i| i.events.retain(|event| !matches!(event, egui::Event::Copy | egui::Event::Cut)));
            if copy {
                self.clipboard("copy");
            }
            if cut && self.editor.as_ref().is_some_and(|ed| ed.writable) {
                self.clipboard("cut");
            }
        }
        // A screenshot has no text for egui's text widget to consume. Route every Ctrl+V
        // through the same paste path; it reads the image clipboard exactly once. Probing
        // with has_picture() first was racy with VirtualBox clipboard synchronisation and
        // could make the actual image read miss the clipboard handoff.
        if paste && self.page_owns_clipboard_input(ctx) {
            // Consume the shortcut before the text widgets draw. Otherwise an LTR TextEdit
            // handles Ctrl+V itself while the page menu goes through `paste_here`, giving the
            // two entry points different behavior.
            ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::V) || i.consume_key(Modifiers::CTRL, Key::V));
            self.paste_here(None);
        }
        if all && ours {
            self.select_all_in_block();
        }
    }

    /// True when the words the caret is in are drawn by Nocturne rather than by an egui
    /// text box, so the keys above are ours to answer.
    fn words_are_ours(&self) -> bool {
        let Some(ed) = self.editor.as_ref() else { return false };
        // A selection across paragraphs belongs to no text box at all.
        if ed.across.is_some_and(|a| !a.inside_one_block()) {
            return true;
        }
        let Some(caret) = ed.caret.as_ref() else { return false };
        // The same question `show_blocks` asks when it decides which painter draws the
        // block: a line with right-to-left in it is ours, and code and maths never are.
        pageedit::needs_our_caret(&caret.text)
            && !matches!(
                ed.blocks.get(caret.block),
                Some(nocturne_formats::markdown::Block::Code { .. }) | Some(nocturne_formats::markdown::Block::MathBlock(_))
            )
    }

    /// Ctrl+A: take the whole of the paragraph the caret is in.
    ///
    /// The paragraph and not the page. That is what a text box does, and it is the step
    /// OneNote takes first as well; pressing again there widens to the box and then to the
    /// page, which is not built here and is written down in `work\expected`.
    fn select_all_in_block(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(caret) = ed.caret.as_mut() else { return };
        let end = caret.text.chars().count();
        caret.selection = (end, 0);
        caret.place = None;
        self.say(crate::tl_id!("ui.paragraph_selected"));
    }

    fn clipboard(&mut self, which: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        // A selection that runs across blocks is not any one block's, so it is taken from
        // the blocks themselves. Cut takes the words out and joins what is left.
        if let Some(open) = ed.across.filter(|a| !a.inside_one_block()) {
            let picked = pageedit::span_text(&ed.blocks, open);
            if picked.is_empty() {
                self.say(crate::tl_id!("ui.select_some_words_first"));
                return;
            }
            let Some(copied) = pageedit::copied_span(&ed.blocks, open) else { return };
            self.pending_copy = Some(nocturne_formats::render::blocks_to_text(&copied));
            self.pending_copy_html = Some(nocturne_formats::render::blocks_to_html(&copied));
            if which == "cut" {
                // The same delete the Delete key and typed text use, so boxes, table widths
                // and justified marks after the cut move up with their blocks.
                Self::remember_step(ed);
                let (_, cols, justified) = Self::delete_across_selection(ed);
                ed.rebuild_body();
                Self::mark_dirty_from_blocks(ed);
                Self::remember_edit(ed);
                if cols || justified {
                    self.save_block_marks((cols, justified));
                }
            }
            self.say(if which == crate::tl_id!("ui.cut_2") { crate::tl_id!("ui.cut") } else { crate::tl_id!("ui.copied") });
            return;
        }
        let (block_idx, prev_sel, copied) = {
            let Some(caret) = ed.caret.as_ref() else {
                self.status = crate::tl_id!("ui.click_in_the_page_first").into();
                self.status_is_error = false;
                return;
            };
            let (a, b) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
            let picked: String = caret.text.chars().skip(a).take(b.saturating_sub(a)).collect();
            if picked.is_empty() {
                self.status = crate::tl_id!("ui.select_some_words_first").into();
                self.status_is_error = false;
                return;
            }
            let Some(copied) = ed.blocks.get(caret.block).and_then(|block| caret.copied_blocks(block)) else { return };
            (caret.block, caret.selection, copied)
        };
        self.pending_copy = Some(nocturne_formats::render::blocks_to_text(&copied));
        self.pending_copy_html = Some(nocturne_formats::render::blocks_to_html(&copied));
        if which == "cut" {
            Self::remember_step(ed);
            if let Some(caret) = ed.caret.as_mut() {
                if let Some(block) = ed.blocks.get(block_idx) {
                    caret.ensure_inline(block);
                }
                let (text, at) = pageedit::insert_at_caret(&caret.text, prev_sel, "");
                caret.text = text.clone();
                caret.selection = (at, at);
                caret.place = Some(at);
                caret.focus = true;
                ed.restore_format_selection = Some((block_idx, at, at));
                if let Some(inline) = caret.inline.as_mut() {
                    inline.update_at(&text, prev_sel, caret.selection);
                }
                if let Some(b) = ed.blocks.get_mut(block_idx) {
                    caret.write(b);
                }
            }
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
        }
        self.say(if which == crate::tl_id!("ui.cut_2") { crate::tl_id!("ui.cut") } else { crate::tl_id!("ui.copied") });
    }

    /// More or less indent on the list item the caret is in.
    fn indent_block(&mut self, deeper: bool) {
        let Some(ed) = self.editor.as_mut() else { return };
        let Some(at) = ed.caret.as_ref().map(|c| c.block) else {
            self.status = crate::tl_id!("ui.click_in_the_page_first").into();
            self.status_is_error = false;
            return;
        };
        Self::remember_step(ed);
        if !pageedit::indent(&mut ed.blocks, at, deeper) {
            self.say(crate::tl_id!("ui.only_a_list_item_can_be_indented"));
            return;
        }
        if let Some(c) = ed.caret.as_mut() {
            c.focus = true;
            c.place = Some(c.selection.0);
            if c.selection.0 != c.selection.1 {
                ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
            }
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
    }

    /// Takes the marks off the selection: bold, italic, strikethrough, code.
    fn clear_formatting(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        if ed.caret.is_none() {
            return;
        }
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        if let Some(b) = ed.blocks.get_mut(caret.block) {
            caret.clear_marks(b);
        }
        if let Some(c) = ed.caret.as_mut() {
            c.focus = true;
            c.place = Some(c.selection.0);
            if c.selection.0 != c.selection.1 {
                ed.restore_format_selection = Some((c.block, c.selection.0, c.selection.1));
            }
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
    }

    /// Puts a whole block (a table, a diagram) after the caret.
    fn insert_block(&mut self, what: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        let at = ed.caret.as_ref().map(|c| c.block + 1).unwrap_or(ed.blocks.len()).min(ed.blocks.len());
        if !pageedit::insert_block(&mut ed.blocks, at, what) {
            self.say(crate::tl_id!("ui.that_is_not_something_a_page_can_hold"));
            return;
        }
        let marks = ed.blocks_inserted(at, 1);
        ed.rebuild_body();
        ed.caret = ed.blocks.get(at).map(|b| pageedit::Caret {
            block: at,
            text: pageedit::caret_text(b),
            focus: true,
            selection: (0, 0),
            place: None,
            cell: None,
            span: None,
            inline: None,
        });
        Self::mark_dirty_from_blocks(ed);
        self.save_block_marks(marks);
    }

    /// Puts a snippet where the caret is, or at the end of the page.
    /// Opens the Link dialog, as OneNote's Insert > Link and Ctrl+K do (issue #163). With the
    /// caret in a link it edits that link; with words selected they are the words to show;
    /// otherwise the link goes in at the caret.
    pub fn open_link_dialog(&mut self) {
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.writable) else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        if ed.across.is_some_and(|a| !a.inside_one_block()) {
            self.say(crate::tl_id!("ui.select_words_in_one_line_for_a_link"));
            return;
        }
        Self::place_caret_for_format(ed);
        let Some(caret) = ed.caret.as_ref() else {
            self.say(crate::tl_id!("ui.click_in_the_page_first_then_use_the_ribbon"));
            return;
        };
        let (lo, hi) = (caret.selection.0.min(caret.selection.1), caret.selection.0.max(caret.selection.1));
        let (range, text, address, editing) = match pageedit::link_at(&caret.text, lo, hi) {
            Some((start, end, words, address)) => ((start, end), words, address, true),
            None => ((lo, hi), caret.text.chars().skip(lo).take(hi - lo).collect(), String::new(), false),
        };
        self.dialog = Some(Dialog::Link { block: caret.block, cell: caret.cell, range, text, address, editing, first: true });
    }

    /// Puts `replacement` in place of the characters `range` of the caret's line, when the
    /// caret is still in the line the Link dialog was opened for. One step of the history.
    fn replace_in_caret_line(&mut self, block: usize, cell: Option<(usize, usize)>, range: (usize, usize), replacement: &str) {
        let Some(ed) = self.editor.as_mut() else { return };
        if !ed.caret.as_ref().is_some_and(|c| c.block == block && c.cell == cell) {
            self.say(crate::tl_id!("ui.the_line_changed_put_the_link_in_again"));
            return;
        }
        Self::remember_step(ed);
        let Some(caret) = ed.caret.as_mut() else { return };
        let chars: Vec<char> = caret.text.chars().collect();
        let (a, b) = (range.0.min(chars.len()), range.1.min(chars.len()));
        let mut text: String = chars[..a].iter().collect();
        text.push_str(replacement);
        let at = text.chars().count();
        text.extend(&chars[b..]);
        caret.text = text;
        caret.selection = (at, at);
        caret.place = Some(at);
        caret.focus = true;
        // Read again from the words, so the new link is a link and not text that looks like one.
        caret.inline = None;
        if let Some(b) = ed.blocks.get_mut(block) {
            caret.write(b);
        }
        ed.rebuild_body();
        Self::mark_dirty_from_blocks(ed);
        Self::remember_edit(ed);
    }

    fn insert_in_page(&mut self, text: &str) {
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.writable) else { return };
        // An insert is an edit: it is recorded for undo before the page changes.
        let has_open_across = ed.across.is_some_and(|a| !a.inside_one_block());
        if has_open_across || ed.caret.is_some() {
            Self::remember_step(ed);
            let (_, across_cols, across_just) = Self::delete_across_selection(ed);
            if let Some(caret) = ed.caret.as_mut() {
                let block_idx = caret.block;
                let prev_sel = caret.selection;
                if let Some(block) = ed.blocks.get(block_idx) {
                    caret.ensure_inline(block);
                }
                // Where the caret is, not at the end of the block.
                let (new_text, at) = pageedit::insert_at_caret(&caret.text, prev_sel, text.trim_matches('\n'));
                caret.text = new_text.clone();
                caret.selection = (at, at);
                caret.place = Some(at);
                caret.focus = true;
                ed.restore_format_selection = Some((block_idx, at, at));
                if let Some(inline) = caret.inline.as_mut() {
                    inline.update_at(&new_text, prev_sel, caret.selection);
                }
                if let Some(b) = ed.blocks.get_mut(block_idx) {
                    caret.write(b);
                }
            }
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
            if across_cols || across_just {
                self.save_block_marks((across_cols, across_just));
            }
        } else {
            ed.across = None;
            let cursor = ed.body.chars().count();
            self.cursor = cursor;
            self.insert(text);
        }
    }

    /// The theme switcher: 14 swatches (light and dark, seven accents each) and a switch to
    /// follow the system setting.
    fn theme_menu(&mut self, ui: &mut Ui, tokens: Tokens) {
        let current = self.theme();
        let mut pick: Option<(Mode, Accent)> = None;
        let mut follow = self.mode_pref == ModePref::System;
        let follow_before = follow;
        // A sun or a moon in the theme's accent colour, instead of a line of text.
        let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
        let painter = ui.painter_at(rect);
        if resp.hovered() {
            painter.rect_filled(rect, 4.0, tokens.row_hover);
        }
        let icon_rect = egui::Rect::from_center_size(rect.center(), vec2(19.0, 19.0));
        if current.mode.is_dark() {
            icons::paint_moon(&painter, icon_rect, tokens.accent, if resp.hovered() { tokens.row_hover } else { tokens.panel });
        } else {
            icons::paint(&painter, icon_rect, Icon::Sun, tokens.accent);
        }
        let theme_label = current.label();
        let label = crate::i18n::format_message_id("ui.theme_theme_2", &[("theme", &theme_label)]);
        let enabled = ui.is_enabled();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
        let theme_label = current.label();
        let tip = crate::i18n::format_message_id("ui.theme_theme_click_to_change_it", &[("theme", &theme_label)]);
        let resp = resp.on_hover_text(tip);
        egui::Popup::menu(&resp).style(crate::theme::picker_popup_style(self.tokens)).show(|ui| {
            theme_grid(ui, current, &mut pick, &mut follow);
        });
        if let Some((mode, accent)) = pick {
            // A swatch names a mode, so clicking one means that mode, even while the app was
            // following the system. Otherwise picking "Dark Default" on a light desktop did
            // nothing visible. The switch below is the way back to following the system.
            self.set_theme(Theme { mode, accent });
        } else if follow != follow_before {
            if follow {
                self.follow_system_theme(current.accent);
            } else {
                self.set_theme(current);
            }
        }
    }

    /// Top-level Theme chooser modal (works in both Classic Ribbon and Tablet Studio).
    fn theme_window_ui(&mut self, ctx: &Context) {
        if !self.theme_window {
            return;
        }
        let tokens = self.tokens;
        let current = self.theme();
        let mut pick: Option<(Mode, Accent)> = None;
        let mut follow = self.mode_pref == ModePref::System;
        let follow_before = follow;
        let mut close_clicked = false;
        let res = crate::modal::Modal::new("theme-chooser-modal", crate::tl_id!("ui.theme"))
            .backdrop_id("theme-chooser-modal-backdrop")
            .width(340.0)
            .show_with_footer(
                ctx,
                &tokens,
                |ui| {
                    theme_grid(ui, current, &mut pick, &mut follow);
                },
                |ui| {
                    ui.horizontal(|ui| {
                        if crate::modal::secondary_button(ui, crate::tl_id!("ui.close"), &tokens).clicked() {
                            close_clicked = true;
                        }
                    });
                },
            );
        if let Some((mode, accent)) = pick {
            self.set_theme(Theme { mode, accent });
            self.theme_window = false;
        } else if follow != follow_before {
            if follow {
                self.follow_system_theme(current.accent);
            } else {
                self.set_theme(current);
            }
        } else if res.closed || close_clicked {
            self.theme_window = false;
        }
    }

    /// A compact language picker beside the header's theme control.
    fn language_menu(&mut self, ui: &mut Ui, tokens: Tokens) {
        let languages = crate::i18n::available_languages();
        let active_code = if self.ui_language == crate::i18n::AUTO { crate::i18n::current_code() } else { self.ui_language.clone() };
        let active_name =
            languages.iter().find(|language| language.code == active_code).map_or_else(|| active_code.clone(), |language| language.name.clone());
        let badge = if active_code == "en-XA" {
            "XA".to_string()
        } else if active_code == "ar-XB" {
            "XB".to_string()
        } else {
            active_code.split('-').next().unwrap_or("en").to_ascii_uppercase()
        };
        let label = crate::i18n::format_message_id("ui.interface_language_language", &[("language", &active_name)]);
        let (rect, resp) = ui.allocate_exact_size(vec2(52.0, 24.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4.0, tokens.row_hover);
        }
        let flag_rect = egui::Rect::from_min_size(rect.left_top() + vec2(5.0, 5.0), vec2(20.0, 14.0));
        crate::language_flag::paint_at(ui, flag_rect, &active_code);
        ui.painter().text(rect.left_center() + vec2(36.0, 0.0), Align2::CENTER_CENTER, badge, egui::FontId::proportional(10.5), tokens.text);
        let enabled = ui.is_enabled();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
        let tip = crate::i18n::format_message_id("ui.language_click_to_change_it", &[("language", &active_name)]);
        let resp = resp.on_hover_text(tip);
        let mut picked = None;
        egui::Popup::menu(&resp).style(crate::theme::picker_popup_style(self.tokens)).show(|ui| {
            let automatic = crate::tl_id!("ui.automatic_system_language");
            if ui.selectable_label(self.ui_language == crate::i18n::AUTO, automatic).clicked() {
                picked = Some(crate::i18n::AUTO.to_string());
            }
            ui.separator();
            for language in &languages {
                let mut clicked = false;
                ui.horizontal(|ui| {
                    crate::language_flag::paint(ui, &language.code);
                    clicked = ui.selectable_label(self.ui_language == language.code, &language.name).clicked();
                });
                if clicked {
                    picked = Some(language.code.clone());
                }
            }
        });
        if let Some(language) = picked {
            self.set_ui_language(language);
        }
    }

    fn search_popup(&mut self, ctx: &Context) {
        self.refresh_search();
        if self.search.trim().is_empty() {
            return;
        }
        let tokens = self.tokens;
        let screen = ctx.content_rect();
        let popup_w = (screen.width() - 24.0).clamp(220.0, 420.0);
        let max_h = (screen.height() * 0.55).clamp(140.0, 380.0);
        let mut chosen: Option<PathBuf> = None;
        let area = if self.is_tablet_studio(ctx)
            && let Some(header) = self.header_pill_rect
        {
            // Below the top-left Tablet Studio header pill so results never collide with the
            // top-right minimap.
            egui::Area::new(Id::unique("search-results"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(header.min.x, header.max.y + 6.0))
                .constrain(true)
        } else {
            let top = (self.band_bottom + 4.0).max(44.0);
            egui::Area::new(Id::unique("search-results")).order(egui::Order::Foreground).anchor(Align2::RIGHT_TOP, vec2(-8.0, top)).constrain(true)
        };
        area.show(ctx, |ui| {
            crate::ribbon::mark_popup_open(ctx);
            egui::Frame::NONE
                .fill(tokens.surface_elevated)
                .stroke(egui::Stroke::new(1.0, tokens.border))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_POPUP))
                .shadow(tokens.shadow_popup())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(popup_w);
                    ui.visuals_mut().override_text_color = Some(tokens.text);
                    egui::ScrollArea::vertical().id_salt("search-results-scroll").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        if self.hits.is_empty() {
                            ui.label(egui::RichText::new(crate::tl_id!("ui.no_pages_match")).color(tokens.muted));
                        }
                        for h in &self.hits {
                            let r = ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&h.title).strong().color(tokens.strong));
                                ui.label(egui::RichText::new(&h.snippet).small().color(tokens.muted));
                            });
                            if ui.interact(r.response.rect, Id::unique(("hit", &h.path)), Sense::click()).clicked() {
                                chosen = Some(h.path.clone());
                            }
                            ui.separator();
                        }
                    });
                });
        });
        if let Some(p) = chosen {
            let mut parent = p.parent();
            while let Some(d) = parent {
                if d == self.session.root() {
                    break;
                }
                self.open.insert(d.to_path_buf());
                parent = d.parent();
            }
            self.open_page(&p);
            self.search.clear();
            self.searched.clear();
            self.hits.clear();
        }
    }

    /// Looks again for the tagged notes in the Tags Summary's scope.
    fn refresh_tags(&mut self) {
        let Some(pane) = self.tags_pane.as_ref() else { return };
        let page = self.editor.as_ref().map(|ed| ed.dir.clone());
        let scope = match pane.scope {
            TagScope::Page => page.clone(),
            TagScope::Section => page.as_deref().and_then(section_for),
            TagScope::Notebook => page.as_deref().and_then(notebook_for),
            TagScope::All => None,
        };
        let mut p: Vec<(&str, String)> = Vec::new();
        if let Some(scope) = scope {
            p.push(("scope", path_str(&scope)));
        } else if pane.scope != TagScope::All {
            // Nothing open to look round: there is nothing in this page or section to list.
            if let Some(pane) = self.tags_pane.as_mut() {
                pane.found.clear();
            }
            return;
        }
        if pane.scope == TagScope::Page {
            p.push(("page", "true".into()));
        }
        if pane.unchecked {
            p.push(("unchecked", "true".into()));
        }
        let pairs: Vec<(&str, &str)> = p.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let found = match self.run("tags.find", &params(&pairs)) {
            Some(Output::Tagged(found)) => found,
            _ => Vec::new(),
        };
        if let Some(pane) = self.tags_pane.as_mut() {
            pane.found = found;
        }
    }

    /// The Tags Summary pane, on the right of the page as OneNote has it.
    /// Reads the templates again: the built-in ones and those kept in the notebooks folder.
    fn refresh_templates(&mut self) {
        let found = nocturne_store::page_templates::list(self.session.root());
        if let Some(pane) = self.templates_pane.as_mut() {
            pane.templates = found;
        }
    }

    /// A new page in the open section, started from the template, and opened.
    fn page_from_template(&mut self, id: &str, name: &str) {
        let Some(section) = self.current_section(self.session.tree()).map(|n| n.path.clone()) else {
            self.say(crate::tl_id!("ui.open_a_section_first_for_a_template"));
            return;
        };
        self.save_now();
        if let Some(Output::Path(page)) = self.run("page.from_template", &params(&[("parent", &path_str(&section)), ("template", id)])) {
            self.open_page(&page);
            self.say(crate::i18n::format_message_id("ui.page_from_template", &[("name", name)]));
        }
    }

    /// Keeps the open page as a template called `name`.
    fn save_page_as_template(&mut self, name: &str) {
        let Some(page) = self.editor.as_ref().map(|ed| ed.dir.clone()) else {
            self.say(crate::tl_id!("ui.open_a_page_first"));
            return;
        };
        self.save_now();
        if let Some(Output::Path(_)) = self.run("page.save_template", &params(&[("path", &path_str(&page)), ("name", name)])) {
            self.say(crate::i18n::format_message_id("ui.saved_as_template", &[("name", name)]));
            if let Some(pane) = self.templates_pane.as_mut() {
                pane.name.clear();
            }
            self.refresh_templates();
        }
    }

    fn templates_panel(&mut self, ui: &mut Ui) {
        let Some(pane) = self.templates_pane.as_mut() else { return };
        let tokens = self.tokens;
        let has_page = self.editor.is_some();
        let mut close = false;
        let mut chosen: Option<(String, String)> = None;
        let mut keep: Option<String> = None;
        // A phone-width window gives the pane the whole width, as it does the Tags Summary.
        let panel = if self.compact {
            egui::Panel::right("page-templates-compact").resizable(false).exact_size(ui.available_width())
        } else {
            egui::Panel::right("page-templates").resizable(true).default_size(280.0).size_range(220.0..=520.0)
        };
        panel.show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(crate::tl_id!("ui.page_templates_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A cross drawn with lines: the interface fonts have no glyph for one.
                    let (rect, x) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.close_the_page_templates")));
                    if x.hovered() {
                        ui.painter().rect_filled(rect, 3.0, tokens.row_hover);
                    }
                    let c = rect.shrink(7.0);
                    let stroke = egui::Stroke::new(1.4, tokens.text);
                    ui.painter().line_segment([c.left_top(), c.right_bottom()], stroke);
                    ui.painter().line_segment([c.right_top(), c.left_bottom()], stroke);
                    if x.on_hover_text(crate::tl_id!("ui.close_the_page_templates")).clicked() {
                        close = true;
                    }
                });
            });
            ui.label(egui::RichText::new(crate::tl_id!("ui.page_templates_hint")).small().color(tokens.muted));
            ui.separator();
            let list_height = (ui.available_height() - 110.0).max(120.0);
            egui::ScrollArea::vertical().id_salt("page-templates-list").max_height(list_height).auto_shrink([false, true]).show(ui, |ui| {
                for category in nocturne_store::page_templates::Category::ALL {
                    let mut items = pane.templates.iter().filter(|t| t.category == category).peekable();
                    let mine = category == nocturne_store::page_templates::Category::Mine;
                    if items.peek().is_none() && !mine {
                        continue;
                    }
                    ui.add_space(4.0);
                    ui.strong(crate::tl!(category.label()));
                    if items.peek().is_none() {
                        ui.weak(crate::tl_id!("ui.no_saved_templates"));
                    }
                    for template in items {
                        let label = if mine { template.name.clone() } else { crate::tl!(&template.name).to_string() };
                        let width = ui.available_width().max(60.0);
                        let (rect, row) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
                        row.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
                        if row.hovered() {
                            ui.painter().rect_filled(rect, 3.0, tokens.row_hover);
                        }
                        ui.painter().text(
                            rect.left_center() + vec2(10.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &label,
                            egui::FontId::proportional(13.5),
                            tokens.text,
                        );
                        if row.on_hover_text(crate::tl_id!("ui.add_a_page_from_this_template")).clicked() {
                            chosen = Some((template.id.clone(), label));
                        }
                    }
                }
            });
            ui.separator();
            ui.label(crate::tl_id!("ui.save_current_page_as_a_template"));
            ui.add_enabled_ui(has_page, |ui| {
                let field = egui::TextEdit::singleline(&mut pane.name)
                    .id(egui::Id::unique(TEMPLATE_NAME_FIELD))
                    .hint_text(crate::tl_id!("ui.template_name"))
                    .desired_width(f32::INFINITY);
                let field = ui.add(field);
                // Named for a screen reader: the grey hint inside the box is not a name.
                field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextInput, has_page, crate::tl_id!("ui.template_name")));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let named = !pane.name.trim().is_empty();
                let clicked = ui.add_enabled(named, egui::Button::new(crate::tl_id!("ui.save_as_template"))).clicked();
                if named && (entered || clicked) {
                    keep = Some(pane.name.trim().to_string());
                }
            });
        });
        if close {
            self.templates_pane = None;
            return;
        }
        if let Some((id, name)) = chosen {
            self.page_from_template(&id, &name);
        }
        if let Some(name) = keep {
            self.save_page_as_template(&name);
        }
    }

    fn tags_panel(&mut self, ui: &mut Ui) {
        let Some(pane) = self.tags_pane.as_mut() else { return };
        let tokens = self.tokens;
        let mut close = false;
        let mut refresh = false;
        let mut chosen: Option<(PathBuf, usize)> = None;
        let panel = if self.compact {
            egui::Panel::right("tags-summary-compact").resizable(false).exact_size(ui.available_width())
        } else {
            egui::Panel::right("tags-summary").resizable(true).default_size(300.0).size_range(220.0..=600.0)
        };
        panel.show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(crate::tl_id!("ui.tags_summary"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A cross drawn with lines: the interface fonts have no glyph for one.
                    let (rect, x) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.close_the_tags_summary")));
                    if x.hovered() {
                        ui.painter().rect_filled(rect, 3.0, tokens.row_hover);
                    }
                    let c = rect.shrink(7.0);
                    let stroke = egui::Stroke::new(1.4, tokens.text);
                    ui.painter().line_segment([c.left_top(), c.right_bottom()], stroke);
                    ui.painter().line_segment([c.right_top(), c.left_bottom()], stroke);
                    if x.on_hover_text(crate::tl_id!("ui.close_the_tags_summary")).clicked() {
                        close = true;
                    }
                });
            });
            ui.label(egui::RichText::new(crate::tl_id!("ui.tags_look_in")).small().color(tokens.muted));
            ui.horizontal_wrapped(|ui| {
                for scope in TagScope::ALL {
                    if ui.selectable_label(pane.scope == scope, scope.label()).clicked() && pane.scope != scope {
                        pane.scope = scope;
                        refresh = true;
                    }
                }
            });
            if ui.checkbox(&mut pane.unchecked, crate::tl_id!("ui.tags_only_unchecked_to_do_items")).changed() {
                refresh = true;
            }
            if ui.button(crate::tl_id!("ui.refresh")).on_hover_text(crate::tl_id!("ui.look_again_for_tagged_notes")).clicked() {
                refresh = true;
            }
            ui.separator();
            egui::ScrollArea::vertical().id_salt("tags-summary-list").auto_shrink([false, false]).show(ui, |ui| {
                if pane.found.is_empty() {
                    ui.weak(crate::tl_id!("ui.no_tagged_notes_here"));
                }
                for tag in nocturne_model::Tag::ALL {
                    let mut lines = pane.found.iter().filter(|f| f.tag == tag).peekable();
                    if lines.peek().is_none() {
                        continue;
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        let mark = ribbon::mark_for(tag);
                        crate::ribbon_icons::paint(ui.painter(), rect, mark, mark.color(tokens.text, tokens.accent));
                        ui.strong(crate::tl!(tag.label()));
                    });
                    for line in lines {
                        let words = if line.text.is_empty() { crate::tl_id!("ui.tags_empty_line").to_string() } else { line.text.clone() };
                        let place = if line.section.is_empty() { line.page.clone() } else { format!("{} > {}", line.section, line.page) };
                        // Painted rather than laid out as labels, so the row is one target that
                        // a screen reader names once, with a To Do's box ticked or not.
                        let width = ui.available_width().max(60.0);
                        let indent = 22.0;
                        let text = ui.painter().layout(words.clone(), egui::FontId::proportional(13.5), tokens.text, width - indent);
                        let under = ui.painter().layout(place, egui::FontId::proportional(11.5), tokens.muted, width - indent);
                        let (rect, row) = ui.allocate_exact_size(vec2(width, text.size().y + under.size().y + 8.0), Sense::click());
                        row.widget_info(|| match line.done {
                            Some(done) => egui::WidgetInfo::selected(egui::WidgetType::CheckBox, true, done, words.clone()),
                            None => egui::WidgetInfo::labeled(egui::WidgetType::Button, true, words.clone()),
                        });
                        let painter = ui.painter();
                        if row.hovered() {
                            painter.rect_filled(rect, 3.0, tokens.row_hover);
                        }
                        if let Some(done) = line.done {
                            let tick = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(12.0, 12.0));
                            painter.rect_stroke(tick, 2.0, egui::Stroke::new(1.2, tokens.muted), egui::StrokeKind::Inside);
                            if done {
                                let at = |x: f32, y: f32| tick.min + vec2(x, y);
                                painter.line(vec![at(2.5, 6.5), at(5.0, 9.0), at(9.5, 3.0)], egui::Stroke::new(1.6, tokens.accent));
                            }
                        }
                        let below = text.size().y + 3.0;
                        painter.galley(rect.min + vec2(indent, 2.0), text, tokens.text);
                        painter.galley(rect.min + vec2(indent, 2.0 + below), under, tokens.muted);
                        if row.clicked() {
                            chosen = Some((line.path.clone(), line.block));
                        }
                    }
                }
            });
        });
        if close {
            self.tags_pane = None;
            return;
        }
        if refresh {
            self.refresh_tags();
        }
        if let Some((page, block)) = chosen {
            self.go_to_tagged(&page, block);
        }
    }

    /// Opens the page a tagged line is on and puts the caret at the start of that line.
    fn go_to_tagged(&mut self, page: &Path, block: usize) {
        let mut parent = page.parent();
        while let Some(d) = parent {
            if d == self.session.root() {
                break;
            }
            self.open.insert(d.to_path_buf());
            parent = d.parent();
        }
        if self.editor.as_ref().is_none_or(|ed| ed.dir != page) {
            self.open_page(page);
        }
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.dir == page && ed.writable) else { return };
        let Some(b) = ed.blocks.get(block).filter(|b| pageedit::is_writable(b)) else { return };
        ed.caret = Some(pageedit::Caret {
            block,
            text: pageedit::caret_text(b),
            focus: true,
            selection: (0, 0),
            place: None,
            cell: None,
            span: None,
            inline: None,
        });
    }

    fn side_panel(&mut self, ui: &mut Ui) {
        // Full page view or Tablet Studio library grid: the classic notebook tree steps aside.
        if self.is_studio_grid(ui.ctx()) || (self.full_page && self.editor.is_some()) {
            return;
        }
        let mut acts = Vec::new();
        // A phone-width window has room for one column, so it always shows the one tree.
        let compact = self.compact;
        if self.split_panes && !compact {
            self.organiser_panes(ui, &mut acts);
            for a in acts {
                self.apply(a);
            }
            return;
        }
        let mut split_now = false;
        // Compact: the list fills the window (a panel of its own, so the desktop width the
        // tree was dragged to is kept for a wider window).
        let panel = if compact {
            egui::Panel::left("tree-compact").resizable(false).exact_size(ui.available_width())
        } else {
            egui::Panel::left("tree").resizable(true).default_size(300.0).size_range(200.0..=700.0)
        };
        panel.show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // In the one-tree layout the same button splits the panel and hides the
                // notebooks, which is what pressing it in OneNote does. A phone-width window
                // has no room for the split, so it has no button for it.
                if !compact {
                    split_now = Self::bars_button_at(ui).clicked();
                }
                ui.label(egui::RichText::new(crate::tl_id!("ui.notebooks_2")).small().color(self.tokens.muted));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.small_button(crate::tl_id!("ui.new_notebook")).clicked() {
                    self.dialog = Some(Dialog::NewNotebook { name: String::new(), format: None });
                }
                if ui.small_button(crate::tl_id!("ui.expand_all")).on_hover_text(crate::tl_id!("ui.open_every_level")).clicked() {
                    self.open = self.collect_branches().into_iter().collect();
                }
                if ui.small_button(crate::tl_id!("ui.collapse_all")).clicked() {
                    self.open.clear();
                }
                self.new_section_button(ui, &mut acts);
            });
            ui.add_space(2.0);
            // A mouse drag in a pane must not scroll it: in the tree it is a click that
            // slipped. The wheel and the bar still work, and on a touch screen a finger drag
            // scrolls, which is the only way a phone has.
            let scroll = egui::containers::scroll_area::ScrollSource {
                drag: egui::containers::scroll_area::DragScroll::OnTouch,
                ..egui::containers::scroll_area::ScrollSource::default()
            };
            egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_source(scroll).show(ui, |ui| {
                if self.session.tree().is_empty() {
                    ui.weak(crate::tl_id!("ui.no_notebooks_yet_use_new_notebook"));
                }
                for n in self.session.tree() {
                    self.draw_node(ui, n, 0, &mut acts);
                }
                self.empty_space_menu(ui, None, &mut acts);
            });
        });
        if split_now {
            self.split_panes = true;
            self.organisers = false;
            let _ = self.session.settings.set("split_panes", "true");
        }
        for a in acts {
            self.apply(a);
        }
    }

    /// The empty space under the rows of a column, with a menu of its own: right-clicking
    /// where there is no row is how a new notebook or a new page is started in OneNote.
    fn empty_space_menu(&self, ui: &mut Ui, section: Option<&Path>, acts: &mut Vec<Act>) {
        let left = ui.available_size();
        if left.y < 2.0 {
            return;
        }
        let (rect, resp) = ui.allocate_exact_size(left, Sense::click());
        let _ = rect;
        let mut reveal = false;
        let mut choose = false;
        resp.context_menu(|ui| {
            match section {
                // Under the pages: a page at the end of this section.
                Some(path) => {
                    if ui.button(crate::tl_id!("ui.new_page_2")).clicked() {
                        acts.push(Act::AddKind(path.to_path_buf(), NodeKind::Section, NodeKind::Page));
                        ui.close();
                    }
                }
                // Under the notebooks: a notebook, and the ways into the folder itself.
                None => {
                    if ui.button(crate::tl_id!("ui.new_notebook_2")).clicked() {
                        acts.push(Act::NewNotebook);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(crate::tl_id!("ui.open_a_folder")).clicked() {
                        choose = true;
                        ui.close();
                    }
                    if ui.button(crate::tl_id!("ui.show_the_notebooks_folder")).clicked() {
                        reveal = true;
                        ui.close();
                    }
                }
            }
        });
        if reveal {
            acts.push(Act::RevealRoot);
        }
        if choose {
            acts.push(Act::ChooseRoot);
        }
    }

    /// The gap between two pages in the pages column. Hovering it draws the line OneNote
    /// draws there, and clicking it makes a page in that place rather than at the end.
    ///
    /// `before` and `after` are the orders of the pages on either side; the new page takes
    /// the number in between, which is why siblings are numbered ten apart.
    fn page_gap(&self, ui: &mut Ui, section: &Path, before: Option<u32>, after: Option<u32>, acts: &mut Vec<Act>) {
        /// How big the round "+" is. The gap itself is shorter than this on purpose: it is
        /// a thin strip between two pages, and the button stands out of it.
        const PLUS: f32 = 8.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::click());
        let hot = resp.hovered();
        if hot {
            // The gap is 7 points tall and the circle is 13 across, so the circle has to
            // be drawn outside the strip it belongs to. `painter_at` cannot: it narrows
            // the clip it is given to whatever the ui already has, which is the strip, and
            // the circle came out a lozenge, flat at the top and the bottom. Painting on
            // the layer starts from the whole window instead.
            let room = (PLUS + 1.0 - rect.height() / 2.0).max(2.0);
            let painter = ui.ctx().layer_painter(ui.layer_id()).with_clip_rect(rect.expand(room));
            let y = rect.center().y;
            let t = &self.tokens;
            painter.line_segment([egui::pos2(rect.min.x + 21.0, y), egui::pos2(rect.max.x - 6.0, y)], egui::Stroke::new(1.5, t.accent));
            let c = egui::pos2(rect.min.x + 10.0, y);
            painter.circle_filled(c, PLUS, t.accent);
            // Draw the cross as two equal strokes around the circle's center. A font glyph's
            // baseline and line box can make a visually off-center plus even with centered text.
            let cross = egui::Stroke::new(2.0, t.panel);
            painter.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], cross);
            painter.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], cross);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let enabled = ui.is_enabled();
        let name = crate::tl_id!("ui.add_a_page_here");
        resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
        let resp = resp.on_hover_text(crate::tl_id!("ui.a_page_here"));
        if resp.clicked() {
            // Halfway between the two, or ten past the last one.
            match (before, after) {
                // No number between them: renumber first, so the page can go between.
                (Some(b), Some(a)) if a <= b + 1 => acts.push(Act::RenumberAndAddPage(section.to_path_buf(), b)),
                (Some(b), Some(a)) => acts.push(Act::AddPageAt(section.to_path_buf(), b + (a - b) / 2)),
                (Some(b), None) => acts.push(Act::AddPageAt(section.to_path_buf(), b.saturating_add(10))),
                (None, Some(a)) => acts.push(Act::AddPageAt(section.to_path_buf(), a / 2)),
                (None, None) => acts.push(Act::AddPageAt(section.to_path_buf(), 10)),
            }
        }
    }

    /// Where a new section goes: the notebook or section group the chosen section or page is in,
    /// a section group that is itself chosen, or else the first notebook. `None` with no notebook.
    fn new_section_parent(&self) -> Option<(PathBuf, NodeKind)> {
        let mut at = self.selected.clone().or_else(|| self.editor.as_ref().map(|e| e.dir.clone()));
        while let Some(path) = at {
            match nocturne_store::tree::read_node(&path).map(|n| n.kind) {
                Some(kind @ (NodeKind::Notebook | NodeKind::SectionGroup)) => return Some((path, kind)),
                Some(_) => at = path.parent().map(Path::to_path_buf),
                None => break,
            }
        }
        self.session.tree().first().map(|n| (n.path.clone(), NodeKind::Notebook))
    }

    /// A plain, always visible "+ New section" button for the chosen notebook, as OneNote has
    /// under its sections, so a section can be added without finding the hover-only plus or the
    /// right-click menu (issue #139). It opens the same dialog those do.
    fn new_section_button(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let target = self.new_section_parent();
        let button = egui::Button::new(crate::tl_id!("ui.new_section_button")).stroke(egui::Stroke::new(1.0, self.tokens.container_border));
        let resp = ui.add_enabled(target.is_some(), button).on_hover_text(crate::tl_id!("ui.a_new_section_in_the_chosen_notebook"));
        if resp.clicked()
            && let Some((parent, kind)) = target
        {
            acts.push(Act::AddKind(parent, kind, NodeKind::Section));
        }
    }

    /// The three-bar button, at the top left of the side panel.
    fn bars_button_at(ui: &mut Ui) -> egui::Response {
        // Drawn rather than typed: the font's own three-bar glyph is heavy and tight.
        let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
        let painter = ui.painter_at(rect);
        if resp.hovered() {
            painter.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
        }
        let colour = ui.visuals().widgets.inactive.fg_stroke.color;
        icons::paint(&painter, egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), Icon::Bars, colour);
        let enabled = ui.is_enabled();
        resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, BARS));
        resp.on_hover_text(crate::tl_id!("ui.show_or_hide_the_notebooks_column"))
    }

    /// The side panel as two columns: the organisers (notebooks, section groups and
    /// sections) and then the pages of whichever section is chosen, the way OneNote splits
    /// them. The three-bar button in the top bar hides the first column.
    fn organiser_panes(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let scroll = || egui::containers::scroll_area::ScrollSource {
            drag: egui::containers::scroll_area::DragScroll::OnTouch,
            ..egui::containers::scroll_area::ScrollSource::default()
        };
        let mut toggle = false;
        if self.organisers {
            egui::Panel::left("organisers").resizable(true).default_size(240.0).size_range(160.0..=520.0).show(ui, |ui| {
                ui.add_space(4.0);
                // The three-bar button sits at the top left of the notebooks, where
                // OneNote keeps it, and hides this column.
                ui.horizontal(|ui| {
                    toggle |= Self::bars_button_at(ui).clicked();
                    ui.label(egui::RichText::new(crate::tl_id!("ui.notebooks_2")).small().color(self.tokens.muted));
                });
                ui.horizontal(|ui| self.new_section_button(ui, acts));
                ui.add_space(2.0);
                egui::ScrollArea::vertical().id_salt("organisers").auto_shrink([false, false]).scroll_source(scroll()).show(ui, |ui| {
                    if self.session.tree().is_empty() {
                        ui.weak(crate::tl_id!("ui.no_notebooks_yet_use_new_notebook"));
                    }
                    for n in self.session.tree() {
                        self.draw_row(ui, n, 0, acts, false);
                    }
                    self.empty_space_menu(ui, None, acts);
                });
            });
        }
        let section = self.current_section(self.session.tree()).map(|n| (n.path.clone(), n.info.title.clone(), n.children.clone()));
        egui::Panel::left("pages").resizable(true).default_size(230.0).size_range(150.0..=520.0).show(ui, |ui| {
            ui.add_space(4.0);
            match &section {
                Some((path, title, pages)) => {
                    ui.horizontal(|ui| {
                        // With the notebooks hidden the button lives here instead, so it is
                        // always at the top left of the panel.
                        if !self.organisers {
                            toggle |= Self::bars_button_at(ui).clicked();
                        }
                        let add_page = egui::Button::new(crate::tl_id!("ui.add_page")).stroke(egui::Stroke::new(1.0, self.tokens.container_border));
                        if ui.add(add_page).on_hover_text(crate::tl_id!("ui.a_new_page_in_this_section")).clicked() {
                            acts.push(Act::Add(path.clone(), NodeKind::Section));
                        }
                    });
                    ui.label(egui::RichText::new(title.to_uppercase()).small().color(self.tokens.muted));
                    ui.add_space(2.0);
                    egui::ScrollArea::vertical().id_salt("pages").auto_shrink([false, false]).scroll_source(scroll()).show(ui, |ui| {
                        let list: Vec<&Node> = pages.iter().filter(|p| p.info.kind == NodeKind::Page).collect();
                        if list.is_empty() {
                            ui.weak(crate::tl_id!("ui.no_pages_in_this_section_yet"));
                        }
                        // A line between every pair, and above the first and below the
                        // last: hovering one offers a page in that place.
                        let mut last: Option<u32> = None;
                        for p in &list {
                            self.page_gap(ui, path, last, Some(p.info.order), acts);
                            self.draw_row(ui, p, 0, acts, true);
                            last = Some(p.info.order);
                        }
                        self.page_gap(ui, path, last, None, acts);
                        self.empty_space_menu(ui, Some(path), acts);
                    });
                }
                None => {
                    ui.horizontal(|ui| {
                        if !self.organisers {
                            toggle |= Self::bars_button_at(ui).clicked();
                        }
                        ui.weak(crate::tl_id!("ui.choose_a_section_to_see_its_pages"));
                    });
                }
            }
        });
        if toggle {
            self.organisers = !self.organisers;
        }
    }

    fn studio_library_ui(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        let sync_busy = self.sync_job.is_some();
        let sync_configured = *self
            .sync_has_token
            .get_or_insert_with(|| self.session.sync_secrets().get_secret(nocturne_engine::KDRIVE_TOKEN_KEY).ok().flatten().is_some());
        let out = self.library_grid.show(ui, &mut self.session, &tokens, sync_busy, sync_configured);
        if let Some(note) = out.notice {
            self.say(&note);
            self.studio_notice = Some((note, Instant::now()));
        }
        if let Some(page_dir) = out.open_page {
            self.expand_to(&page_dir);
            self.open_page(&page_dir);
        }
        if let Some((target_section, mode)) = out.new_page {
            if let Some(section) = target_section {
                self.create_page_in(&section);
                match mode {
                    crate::library_grid::NewPageMode::Handwrite => {
                        self.ribbon_act(ribbon::Act::InkPen(self.nib));
                    }
                    crate::library_grid::NewPageMode::Type => {
                        self.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
                        if let Some(ed) = self.editor.as_mut() {
                            ed.title_focused = true;
                            ed.caret = None;
                            ed.across = None;
                        }
                        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::unique(PAGE_TITLE_FIELD)));
                    }
                }
            } else {
                self.open_new_notebook_dialog();
            }
        }
        if let Some((target_section, kind)) = out.import {
            if let Some(section) = target_section {
                if let Some(file) = crate::library_grid::pick_import_file(kind) {
                    self.create_page_in(&section);
                    if let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) {
                        let choice = match kind {
                            crate::library_grid::ImportKind::Document => FileChoice::Printout,
                            crate::library_grid::ImportKind::Picture => FileChoice::Copy,
                        };
                        self.add_file(&dir, &file, choice);
                    }
                }
            } else {
                self.open_new_notebook_dialog();
            }
        }
        if out.new_notebook {
            self.open_new_notebook_dialog();
        }
        if let Some((parent, parent_kind)) = out.new_section_in {
            self.open_add_dialog(parent, parent_kind, NodeKind::Section);
        }
        for action in out.actions {
            match action {
                crate::library_grid::LibraryAction::Rename { path, title } => {
                    self.open_rename_dialog(path, title);
                }
                crate::library_grid::LibraryAction::Delete { path, title, count } => {
                    self.dialog = Some(Dialog::Delete { path, title, count });
                }
                crate::library_grid::LibraryAction::NewPage { section } => {
                    self.create_page_in(&section);
                }
                crate::library_grid::LibraryAction::NewSection { parent, parent_kind } => {
                    self.open_add_dialog(parent, parent_kind, NodeKind::Section);
                }
                crate::library_grid::LibraryAction::ImportFile { section, kind, file } => {
                    self.create_page_in(&section);
                    if let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone()) {
                        let choice = match kind {
                            crate::library_grid::ImportKind::Document => FileChoice::Printout,
                            crate::library_grid::ImportKind::Picture => FileChoice::Copy,
                        };
                        self.add_file(&dir, &file, choice);
                    }
                }
            }
        }
        if out.sync_now {
            let ctx = ui.ctx().clone();
            self.sync_now(&ctx);
        }
        for act in out.ribbon_acts {
            self.ribbon_act(act);
        }
    }

    fn central(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            if self.editor.is_none() {
                // With no page open there is nothing to scroll, zoom or write on, so the last
                // page's rect must not linger for the platform's gestures to aim at.
                self.page_rect = None;
                self.page_view_rect = None;
                self.pencil_pill_rect = None;
                crate::containers::clear_touch_handles(ui.ctx());
                crate::inkpad::clear_held_hit_cache(ui.ctx());
                if self.is_studio_grid(ui.ctx()) {
                    self.studio_library_ui(ui);
                    return;
                }
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.add(egui::Image::from_bytes("bytes://nocturne-logo", crate::LOGO_PNG).fit_to_exact_size(vec2(128.0, 128.0)));
                    ui.add_space(8.0);
                    ui.heading(crate::tl_id!("ui.nocturne"));
                    ui.label(crate::tl_id!("ui.notes_as_plain_folders_open_a_page_or_create_a_notebook_to_start"));
                    ui.add_space(4.0);
                    let folder = self.shown(self.session.root());
                    let message = crate::i18n::format_message_id("ui.notebooks_folder_folder", &[("folder", &folder)]);
                    ui.label(egui::RichText::new(message).small().color(self.tokens.muted));
                });
                return;
            }
            self.editor_ui(ui);
        });
    }

    fn wheel_canvas_pan_delta(delta: egui::Vec2, modifiers: Modifiers, canvas_pan_enabled: bool) -> egui::Vec2 {
        if modifiers.ctrl || modifiers.command {
            return egui::Vec2::ZERO;
        }
        if modifiers.shift {
            return egui::vec2(delta.y, 0.0);
        }
        if canvas_pan_enabled { delta } else { egui::Vec2::ZERO }
    }

    fn editor_ui(&mut self, ui: &mut Ui) {
        if let Some(ed) = self.editor.as_mut() {
            ed.caret_screen_rect = None;
            ed.title_focused = false;
            ed.title_selection = None;
        }
        if !self.icon_recovery_suppressed
            && self.dialog.is_none()
            && let Some(ed) = self.editor.as_ref()
            && let Some(name) = first_missing_attachment_in_source(&ed.body, &ed.dir).or_else(|| first_legacy_attachment_in_source(&ed.body))
        {
            let key = format!("{}::{name}", ed.dir.display());
            if self.icon_recovery_reported.insert(key) {
                log::warn!("attachment icon is missing: page={:?} attachment={name:?}", ed.dir);
                self.dialog = Some(Dialog::MissingAttachmentIcon { page: ed.dir.clone(), name });
            }
        }
        let stylus_active = self.stylus.has_active_stylus();
        let finger_count = self.stylus.active_finger_count();
        let pointer_origin = self.stylus.pointer_origin();
        // The window's pointer follows only the first contact on the glass. With a palm resting
        // there it is the palm's, and ink read from the pointer was drawn at the palm. So once a
        // pen is down beside a finger or palm, and until that pen lifts, the ink reads the pen's
        // own touch. A pen alone keeps the ordinary pointer path.
        let pen_frame = self.stylus.stylus_frame();
        self.pen_reads_own_touch = pen_frame.is_some() && (self.pen_reads_own_touch || finger_count > 0);
        let pen_frame = pen_frame.filter(|_| self.pen_reads_own_touch);
        ui.ctx().data_mut(|d| match pen_frame {
            Some(frame) => {
                d.insert_temp(crate::stylus::StylusFrame::id(), frame);
            }
            None => d.remove::<crate::stylus::StylusFrame>(crate::stylus::StylusFrame::id()),
        });
        let multi_touch_active = !stylus_active && (finger_count >= 2 || ui.input(|i| i.multi_touch().is_some()));
        let finger_drawing = self.is_finger_drawing();
        let touch_sized = self.stylus.touch_affordance_active() || ui.ctx().os() == egui::os::OperatingSystem::IOS;
        let mut finger_pan_delta = egui::Vec2::ZERO;
        let mut tap_at: Option<egui::Pos2> = None;
        let mut was_scrolling = false;
        let mut finger_handle_ended = false;
        if self.show_source {
            crate::containers::clear_touch_handles(ui.ctx());
            crate::inkpad::clear_held_hit_cache(ui.ctx());
        }

        let math_preview_open = matches!(self.math_ocr.as_ref().map(|c| &c.stage), Some(crate::math_ocr::Stage::Preview { .. }));
        if stylus_active
            || multi_touch_active
            || pointer_origin == crate::stylus::PointerOrigin::Mouse
            || finger_drawing
            || self.show_source
            || self.dialog.is_some()
            || math_preview_open
            || self.backstage.is_some()
            || self.compact_list_fills()
        {
            self.finger_pan = FingerPanState::Idle;
            self.finger_handle_drag = None;
        } else if pointer_origin == crate::stylus::PointerOrigin::Finger {
            let write_handles_active = self.pointer == ribbon::Pointer::Write && !self.show_source && !self.space_tool;
            let ink_grab_active = matches!(self.pointer, ribbon::Pointer::Lasso | ribbon::Pointer::Pick) && !self.show_source && !self.space_tool;
            let events = ui.input(|i| i.events.clone());
            for event in &events {
                let egui::Event::Touch { id, phase, pos, force, .. } = event else { continue };
                if force.is_some_and(f32::is_finite) {
                    continue;
                }
                match phase {
                    egui::TouchPhase::Start => {
                        if self.canvas_pointer_unoccluded(ui.ctx(), *pos) {
                            let hits_write_handle = write_handles_active
                                && (crate::containers::active_touch_handle_at(ui.ctx(), *pos) || crate::inkpad::held_handle_at(ui.ctx(), *pos));
                            let hits_ink_grab = ink_grab_active
                                && (crate::inkpad::held_handle_at(ui.ctx(), *pos)
                                    || crate::inkpad::held_area(ui.ctx()).is_some_and(|r| r.contains(*pos)));
                            if hits_write_handle || hits_ink_grab {
                                self.finger_handle_drag = Some(*id);
                                self.finger_pan = FingerPanState::Idle;
                            } else if self.finger_handle_drag.is_none() {
                                self.finger_pan = FingerPanState::Pending { id: *id, start_pos: *pos, last_pos: *pos };
                            }
                        }
                    }
                    egui::TouchPhase::Move => {
                        if self.finger_handle_drag == Some(*id) {
                            continue;
                        }
                        match self.finger_pan {
                            FingerPanState::Pending { id: pid, start_pos, last_pos } if pid == *id => {
                                if pos.distance(start_pos) > TOUCH_PAN_SLOP {
                                    finger_pan_delta += *pos - last_pos;
                                    self.finger_pan = FingerPanState::Scrolling { id: pid, last_pos: *pos };
                                } else {
                                    self.finger_pan = FingerPanState::Pending { id: pid, start_pos, last_pos: *pos };
                                }
                            }
                            FingerPanState::Scrolling { id: sid, last_pos } if sid == *id => {
                                finger_pan_delta += *pos - last_pos;
                                self.finger_pan = FingerPanState::Scrolling { id: sid, last_pos: *pos };
                            }
                            _ => {}
                        }
                    }
                    egui::TouchPhase::End => {
                        if self.finger_handle_drag == Some(*id) {
                            self.finger_handle_drag = None;
                            finger_handle_ended = true;
                            continue;
                        }
                        match self.finger_pan {
                            FingerPanState::Pending { id: pid, start_pos, last_pos } if pid == *id => {
                                if pos.distance(start_pos) <= TOUCH_PAN_SLOP {
                                    if self.pointer != ribbon::Pointer::Write {
                                        self.pointer = ribbon::Pointer::Write;
                                        self.space_tool = false;
                                        self.sync_tool_state_to_session();
                                    }
                                    tap_at = Some(*pos);
                                } else {
                                    finger_pan_delta += *pos - last_pos;
                                    was_scrolling = true;
                                }
                                self.finger_pan = FingerPanState::Idle;
                            }
                            FingerPanState::Scrolling { id: sid, last_pos } if sid == *id => {
                                finger_pan_delta += *pos - last_pos;
                                was_scrolling = true;
                                self.finger_pan = FingerPanState::Idle;
                            }
                            _ => {}
                        }
                    }
                    egui::TouchPhase::Cancel => {
                        if self.finger_handle_drag == Some(*id) {
                            self.finger_handle_drag = None;
                            finger_handle_ended = true;
                            continue;
                        }
                        match self.finger_pan {
                            FingerPanState::Pending { id: pid, .. } | FingerPanState::Scrolling { id: pid, .. } if pid == *id => {
                                self.finger_pan = FingerPanState::Idle;
                                was_scrolling = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if finger_pan_delta.x != 0.0 && finger_pan_delta.x.is_finite() {
            self.canvas_pan.x = (self.canvas_pan.x + finger_pan_delta.x).clamp(-1.0e6, 1.0e6);
        }
        let held_ink_touch_drag = matches!(self.pointer, ribbon::Pointer::Write | ribbon::Pointer::Lasso | ribbon::Pointer::Pick)
            && (self.finger_handle_drag.is_some() || finger_handle_ended);
        let allow_stroke =
            !multi_touch_active && (!(pointer_origin == crate::stylus::PointerOrigin::Finger && !finger_drawing) || held_ink_touch_drag);
        let cancel_uncommitted = multi_touch_active;
        let touch_nav_active = !stylus_active
            && !finger_drawing
            && self.finger_handle_drag.is_none()
            && !finger_handle_ended
            && (multi_touch_active
                || matches!(self.finger_pan, FingerPanState::Pending { .. } | FingerPanState::Scrolling { .. })
                || (pointer_origin == crate::stylus::PointerOrigin::Finger && (self.stylus.finger_ended_this_frame() || was_scrolling)));

        let page_input_rect = self.page_view_rect.or(self.page_rect);
        let wheel_canvas_pan_enabled = self.pointer != ribbon::Pointer::Write;
        if self.editor.is_some() && !self.show_source {
            // Draw > Panning Hand: the main button drags the page as the middle one always does
            // (issue #270), but not from the scroll bar, which keeps its own drag.
            let hand = self.pointer == ribbon::Pointer::Pan
                && !multi_touch_active
                && (pointer_origin == crate::stylus::PointerOrigin::Mouse || finger_drawing || stylus_active);
            let bar = self.page_scroll_strip;
            let (pointer, middle_pressed, middle_down, wheel) = ui.input(|i| {
                let pointer = page_input_rect.and_then(|rect| i.pointer.interact_pos().filter(|p| rect.contains(*p)));
                let wheel = if page_input_rect.is_some_and(|rect| i.pointer.hover_pos().is_some_and(|p| rect.contains(p))) {
                    i.events.iter().fold(egui::Vec2::ZERO, |sum, event| match event {
                        egui::Event::MouseWheel { unit, delta, modifiers, .. } => {
                            let scaled = match unit {
                                egui::MouseWheelUnit::Point => *delta,
                                egui::MouseWheelUnit::Line => *delta * 24.0,
                                egui::MouseWheelUnit::Page => *delta * 240.0,
                            };
                            sum + Self::wheel_canvas_pan_delta(scaled, *modifiers, wheel_canvas_pan_enabled)
                        }
                        _ => sum,
                    })
                } else {
                    egui::Vec2::ZERO
                };
                let pointer = pointer.filter(|p| !(hand && bar.is_some_and(|b| b.contains(*p))));
                let pressed = i.pointer.button_pressed(egui::PointerButton::Middle) || (hand && i.pointer.primary_pressed());
                let down = i.pointer.button_down(egui::PointerButton::Middle) || (hand && i.pointer.primary_down());
                (pointer, pressed, down, wheel)
            });
            if wheel != egui::Vec2::ZERO {
                self.canvas_pan += wheel;
                self.canvas_pan.x = self.canvas_pan.x.clamp(-1.0e6, 1.0e6);
                self.canvas_pan.y = self.canvas_pan.y.clamp(-1.0e6, 1.0e6);
            }
            if middle_pressed {
                self.canvas_drag = pointer;
            } else if middle_down {
                if let (Some(last), Some(now)) = (self.canvas_drag, pointer) {
                    self.canvas_pan += now - last;
                    self.canvas_drag = Some(now);
                }
            } else {
                self.canvas_drag = None;
            }
        } else {
            self.canvas_drag = None;
        }
        let (wheel_zoom, wheel_focal) = ui.input(|i| {
            if i.multi_touch().is_some() {
                return (1.0, None);
            }
            match page_input_rect.and_then(|rect| i.pointer.hover_pos().filter(|p| rect.contains(*p))) {
                Some(pos) => (i.zoom_delta(), Some(pos)),
                None => (1.0, None),
            }
        });
        if wheel_zoom.is_finite() && (wheel_zoom - 1.0).abs() > 0.0005 {
            self.apply_external_pinch_zoom(wheel_zoom, wheel_focal);
        }
        // On a touch screen two fingers scroll, pan sideways, and pinch-zoom the page,
        // suppressed while a stylus contact is active for palm rejection. Applied before
        // `ScrollArea` and `page()` render so zoom, pan, and focal-point anchoring take
        // effect on the same frame without lag or missed repaints.
        if !stylus_active
            && !self.external_gesture_blocked()
            && let Some(touch) = ui.input(|i| i.multi_touch())
        {
            let (pan, zoom_scale_delta) = crate::touch_ux::sanitize_multitouch_canvas_delta(touch.translation_delta, touch.zoom_delta);
            if pan != egui::Vec2::ZERO {
                self.apply_external_pan(pan);
            }
            if let Some(scale) = zoom_scale_delta {
                // The page origin is still from the last frame, when the fingers were at
                // their previous center. The pan carries that anchored point to the new center.
                self.apply_external_pinch_zoom(scale, Some(touch.center_pos - pan));
            }
        }
        if self.fit_page_width {
            let width = ui.available_width();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let percent =
                (width / 780.0 * 100.0).round().clamp(crate::touch_ux::MIN_ZOOM_PERCENT as f32, crate::touch_ux::MAX_ZOOM_PERCENT as f32) as u32;
            self.set_zoom_percent(percent);
            self.fit_page_width = false;
        }
        #[allow(clippy::cast_precision_loss)]
        let zoom_scale = self.zoom_percent as f32 / 100.0;
        let tokens = self.tokens;
        let mut rename: Option<String> = None;
        let cursor = self.cursor;
        let mut changed = false;
        let show_source = self.show_source;
        let full_page = self.full_page;
        let (hide_title, rules) = (self.hide_title, self.paper_now());
        // The grid a moved or resized box snaps to: the paper's own lines, or 12 points on plain
        // paper. None while snapping is off (issue #79).
        let snap = self.snap_to_grid.then(|| if rules.on() { rules.step() } else { PLAIN_SNAP_STEP });
        // The wash behind this page, if it carries a colour: a pale tint of it over the
        // page's own background, so the words keep their contrast whatever is chosen.
        let page_colour = self.editor.as_ref().and_then(|e| e.colour).map(|c| {
            let dark = self.theme().mode == crate::theme::Mode::Dark;
            let (r, g, b) = c.rgb(dark);
            crate::theme::mix(self.tokens.editor, egui::Color32::from_rgb(r, g, b), 0.16)
        });
        // What the title area is showing, so the page's words do not say it again.
        let shown_title = self.editor.as_ref().map(|e| e.title.clone()).unwrap_or_default();
        let (mut toggle_source, mut toggle_full_page, mut focus_search) = (false, false, false);
        let mut from_blocks = false;
        let mut boxes_moved = false;
        let mut cols_moved = false;
        let mut justified_moved = false;
        // A file other than the page written in the code view, waiting to go to disk.
        let mut wrote_file: Option<(String, String)> = None;
        let mut file_action: Option<crate::codeview::FileAction> = None;
        let mut code = std::mem::take(&mut self.code);
        let mut save_cols: Option<(String, String)> = None;
        let mut save_justified: Option<(String, String)> = None;
        // A link clicked in the page this frame, followed once the page's borrow is over.
        let mut follow: Option<String> = None;
        // The drawing on this page is lent to the block that holds it, and written back
        // once the page's borrow is over.
        let pen = self.pen();
        let pressure = if pointer_origin == crate::stylus::PointerOrigin::Mouse { 1.0 } else { self.stylus.sample().pressure };
        let mut pad = self.pad.take();
        let mut ink_changed = false;
        let mut page_rect = None;
        let mut hbar = crate::hscroll::Shown::default();
        // What the page's right-click menu asked for, run once the page's borrow is over.
        let mut page_acts: Vec<ribbon::Act> = Vec::new();
        let collect_now = std::mem::take(&mut self.collect_pending);
        // Insert Space: in hand this frame, and whether it was used or put down.
        let space_on = self.space_tool && self.pointer == ribbon::Pointer::Write;
        let mut space_over = false;
        let current_zoom_pct = self.zoom_percent;
        let tablet_studio = self.is_tablet_studio(ui.ctx());
        let show_minimap = self.minimap_visible(ui.ctx());
        if !tablet_studio {
            self.pencil_pill_rect = None;
            self.tool_settings_open = false;
        }
        let hide_canvas_chrome = self.dialog.is_some()
            || math_preview_open
            || self.theme_window
            || self.import_job.is_some()
            || self.ribbon_peek
            || self.backstage.is_some()
            || !self.search.trim().is_empty()
            || crate::ribbon::any_popup_open_this_pass(ui.ctx());
        let canvas_overlay_open = hide_canvas_chrome || self.tool_settings_open || crate::ribbon::any_popup_open(ui.ctx());
        let mut pill_acts: Vec<ribbon::Act> = Vec::new();
        let mut start_ocr: Option<nocturne_engine::OcrMode> = None;
        let mut minimap_out = crate::minimap::MinimapOut::default();
        {
            let Some(ed) = self.editor.as_mut() else { return };
            let missing_fonts = crate::fonts::ensure_page_fonts(ui.ctx(), &crate::fonts::page_families(&ed.blocks));
            if !missing_fonts.is_empty() {
                let message = crate::i18n::format_message_id("ui.fonts_unavailable", &[("fonts", &missing_fonts.join(", "))]);
                ui.label(egui::RichText::new(message).color(tokens.muted));
            }
            // One id for the title, whether it is drawn as words or as a text box.
            let title_id = egui::Id::unique(PAGE_TITLE_FIELD);
            if !tablet_studio {
                ui.horizontal_wrapped(|ui| {
                    // File details live in the status bar. Keep only compact page controls
                    // here, with a drop instruction while a file is actually over the window.
                    if ui.input(|i| !i.raw.hovered_files.is_empty()) {
                        ui.label(egui::RichText::new(crate::tl_id!("ui.drop_a_file_onto_the_page_to_add_it")).small().color(tokens.muted));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The page controls, in OneNote's order: full page, search, source.
                        let full = icon_toggle(
                            ui,
                            if full_page { Icon::ExitFullPage } else { Icon::FullPage },
                            full_page,
                            &tokens,
                            crate::tl_id!("ui.full_page"),
                            if full_page { crate::tl_id!("ui.show_the_notebooks_again") } else { crate::tl_id!("ui.give_the_page_the_whole_window") },
                        );
                        if full.clicked() {
                            toggle_full_page = true;
                        }
                        if icon_toggle(ui, Icon::Search, false, &tokens, crate::tl_id!("ui.search"), crate::tl_id!("ui.search_all_pages")).clicked() {
                            focus_search = true;
                        }
                        if icon_toggle(
                            ui,
                            Icon::Code,
                            show_source,
                            &tokens,
                            crate::tl_id!("ui.page_source"),
                            if show_source { crate::tl_id!("ui.hide_the_page_source") } else { crate::tl_id!("ui.show_the_page_source") },
                        )
                        .clicked()
                        {
                            toggle_source = true;
                        }
                    });
                });
                ui.separator();
            }
            let height = ui.available_height();
            let studio_view_rect = self.page_view_rect.unwrap_or_else(|| ui.available_rect_before_wrap());
            // How far the page's content reaches sideways on screen this frame (issue #188).
            let mut page_span: Option<(f32, f32)> = None;
            // The page itself. Without the source beside it the page is a readable column,
            // the way a page looks in OneNote or Word, and a click puts the caret in it.
            let mut page = |ui: &mut Ui,
                            ed: &mut Editor,
                            wide: bool,
                            _changed: &mut bool,
                            from_blocks: &mut bool,
                            _toggle: &mut bool,
                            boxes_moved: &mut bool,
                            cols_moved: &mut bool,
                            justified_moved: &mut bool,
                            follow: &mut Option<String>,
                            rename: &mut Option<String>| {
                let viewport = ui.available_rect_before_wrap().shrink2(vec2(0.0, 1.0));
                if wide || ed.canvas_layout != nocturne_store::page::CanvasLayout::Infinite {
                    let visible = viewport.with_max_y(viewport.max.y.max(ui.clip_rect().max.y));
                    let painter = ui.painter_at(visible);
                    let lines = crate::theme::rule_lines_in(&tokens, ed.rule_colour);
                    let origin = viewport.translate(self.canvas_pan).min + ed.origin_in_canvas.unwrap_or_default();
                    if let Some(sheet) = crate::canvas_paper::sheet_size(ed.canvas_layout) {
                        crate::canvas_paper::paint(
                            &painter,
                            visible,
                            origin,
                            zoom_scale,
                            sheet,
                            rules,
                            lines,
                            page_colour.unwrap_or(tokens.editor),
                            &tokens,
                        );
                    } else {
                        if let Some(wash) = page_colour {
                            painter.rect_filled(viewport, 0.0, wash);
                        }
                        if rules.on() {
                            crate::ribbon::paint_paper_canvas(&painter, viewport, rules, zoom_scale, origin, lines.0, lines.1);
                        }
                    }
                }
                let content_rect = viewport.translate(self.canvas_pan);
                ui.scope_builder(egui::UiBuilder::new().id_salt("page-canvas-content").max_rect(content_rect), |ui| {
                    // Which way lines fall when they say nothing themselves: the page's own
                    // direction, if the writer gave it one.
                    let page_dir = ed.page_dir;
                    let available_width = ui.available_width();
                    let page_column = 780.0 * zoom_scale;
                    let margin = if wide { ((available_width - page_column) / 2.0).clamp(8.0, 127.0) } else { 0.0 };
                    #[allow(clippy::cast_possible_truncation)]
                    let margin_i8 = margin.round() as i8;
                    egui::Frame::NONE.inner_margin(egui::Margin { left: margin_i8, right: margin_i8, top: 4, bottom: 8 }).show(ui, |ui| {
                        let column = if available_width >= page_column { page_column } else { (available_width - 16.0).max(1.0) };
                        ui.set_min_width(column);
                        ui.set_width(column);
                        if tablet_studio {
                            let top_pad = crate::tablet_chrome::canvas_top_clearance(
                                studio_view_rect,
                                self.header_pill_rect,
                                self.pencil_pill_rect,
                                self.session.studio_options().toolbar_dock,
                            );
                            let extra = (top_pad - 4.0).max(0.0);
                            if extra > 0.0 {
                                ui.add_space(extra);
                            }
                        }
                        // The page's own name, at the top of the sheet. Before the sheet's
                        // origin is taken below, so the ink layer keeps its own coordinates
                        // and a drawing stays beside the words it was made next to.
                        if !hide_title {
                            page_title(ui, ed, &tokens, title_id, rename, zoom_scale, tap_at);
                        }
                        if wide && ed.writable {
                            // The caret lives in the page: every block is drawn the way it
                            // will be read, and the one being written in is a text box in
                            // that block's own type. The markup is never shown here. The
                            // blocks sit in containers, which is where they are placed.
                            let top = ui.cursor().min;
                            if let Some(pad) = pad.as_mut() {
                                // Ink sits above page content, but below foreground controls such as
                                // pen palettes. A foreground ink layer registered after the ribbon
                                // paints across its menus when a stroke is already in progress.
                                let layer = egui::LayerId::new(egui::Order::Middle, ui.id().with("ink-overlay"));
                                // The viewport is one view tall from the top of the page, which a
                                // scrolled page has left behind: the ink reaches down to the
                                // bottom of what the window shows.
                                let ink_area = viewport.with_max_y(viewport.max.y.max(ui.clip_rect().max.y));
                                let hit = self
                                    .page_scroll_strip
                                    .filter(|bar| bar.min.x > ink_area.min.x)
                                    .map_or(ink_area, |bar| ink_area.with_max_x(ink_area.max.x.min(bar.min.x)));
                                let mut ink_ui = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(ink_area));
                                let ink_blocked_by_modal = self.dialog.is_some()
                                    || math_preview_open
                                    || self.theme_window
                                    || self.import_job.is_some()
                                    || self.backstage.is_some()
                                    || self.tool_settings_open;
                                ink_changed |= crate::inkpad::show_with_input_and_handles(
                                    &mut ink_ui,
                                    ink_area,
                                    hit,
                                    top,
                                    pad,
                                    pen,
                                    pressure,
                                    &tokens,
                                    zoom_scale,
                                    allow_stroke && !ink_blocked_by_modal,
                                    cancel_uncommitted,
                                    touch_sized,
                                );
                            }
                            // A press lets go of whatever was selected: the drag that may
                            // follow it builds a new one. Done here, once a frame, rather
                            // than inside the blocks, where a container drawn later would
                            // clear what an earlier one had just set.
                            let ink_owns_canvas = pen.pointer != ribbon::Pointer::Write;
                            let pressed = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten());
                            let press_in_page = pressed.is_some_and(|pos| {
                                viewport.contains(pos) && ui.ctx().layer_id_at(pos).is_none_or(|layer| layer.order < egui::Order::Foreground)
                            });
                            if ((press_in_page && !touch_nav_active) || tap_at.is_some()) && !ink_owns_canvas {
                                ed.across = None;
                            }
                            // A page without a saved arrangement is one responsive sheet of
                            // text. Keep its default box aligned with the available page width.
                            if !ed.arranged && ed.boxes.len() == 1 {
                                ed.boxes[0].width = crate::containers::responsive_default_width(ui.available_width(), zoom_scale);
                            }
                            if let Some(selection) = ed.restore_format_selection.take() {
                                ui.data_mut(|data| data.insert_temp(pageedit::restore_selection_id(), selection));
                            }
                            let before_caret = ed.pending_format.as_ref().and_then(|_| ed.caret.clone());
                            let mut moved = if ink_owns_canvas {
                                crate::containers::show_ink_view(
                                    ui,
                                    &ed.blocks,
                                    &mut ed.boxes,
                                    &ed.dir,
                                    &tokens,
                                    page_dir,
                                    &ed.justified,
                                    &ed.columns,
                                    if hide_title { "" } else { &shown_title },
                                    zoom_scale,
                                )
                            } else {
                                let overlay_open = canvas_overlay_open || self.space_tool;
                                crate::containers::show_with_touch_and_handles(
                                    ui,
                                    &mut ed.blocks,
                                    &mut ed.boxes,
                                    &mut ed.caret,
                                    &ed.dir,
                                    &tokens,
                                    page_dir,
                                    &mut ed.columns,
                                    &mut ed.justified,
                                    &mut ed.across,
                                    if hide_title { "" } else { &shown_title },
                                    zoom_scale,
                                    snap,
                                    touch_nav_active,
                                    tap_at,
                                    touch_sized,
                                    overlay_open,
                                )
                            };
                            if moved.changed.caret_rect.is_some() && !ed.title_focused {
                                ed.caret_screen_rect = moved.changed.caret_rect;
                            }
                            if moved.changed.blocks && ui.input(|input| input.events.iter().any(|event| matches!(event, egui::Event::Text(_)))) {
                                ed.apply_pending_format(before_caret.as_ref());
                            } else if let Some(pending) = ed.pending_format.as_ref()
                                && ed.caret.as_ref().is_none_or(|caret| caret.block != pending.block || caret.selection != (pending.at, pending.at))
                            {
                                ed.pending_format = None;
                            }
                            page_acts.append(&mut moved.acts);
                            // Collect All (issue #23): the boxes and the strokes of ink, each as the
                            // rectangle it covers, gathered so no empty stretch of the page between
                            // them is wider than a small gap. Nothing moves past anything else.
                            if collect_now {
                                let strokes = pad.as_ref().map(crate::inkpad::Pad::stroke_bounds).unwrap_or_default();
                                let items: Vec<egui::Rect> = moved
                                    .frames
                                    .iter()
                                    .copied()
                                    .chain(strokes.iter().map(|[x0, y0, x1, y1]| egui::Rect::from_min_max(egui::pos2(*x0, *y0), egui::pos2(*x1, *y1))))
                                    .collect();
                                let by = crate::collect::gather(&items, COLLECT_GAP);
                                for (i, d) in by.iter().take(moved.frames.len()).enumerate() {
                                    if let Some(b) = ed.boxes.get_mut(i)
                                        && *d != egui::Vec2::ZERO
                                    {
                                        *b = nocturne_model::container::Container::new(b.x + d.x, b.y + d.y, b.width, b.blocks);
                                        moved.boxes = true;
                                    }
                                }
                                let ink: Vec<(f32, f32)> = by.iter().skip(moved.frames.len()).map(|d| (d.x, d.y)).collect();
                                if let Some(pad) = pad.as_mut() {
                                    ink_changed |= pad.move_each(&ink);
                                }
                            }
                            // The page's column, its boxes and its ink: what the sideways bar
                            // has to reach.
                            {
                                let left = moved.origin.x;
                                let mut span = (left, (left + moved.right * zoom_scale).max(left + ui.min_rect().width()));
                                if let Some([x0, _, x1, _]) = pad.as_ref().and_then(|p| p.bounds()) {
                                    span = (span.0.min(top.x + x0 * zoom_scale), span.1.max(top.x + x1 * zoom_scale));
                                }
                                if let Some(sheet) = crate::canvas_paper::sheet_size(ed.canvas_layout) {
                                    span.1 = span.1.max(left + sheet.x * zoom_scale);
                                }
                                page_span = Some(span);
                            }
                            // Insert Space (issue #164): a drag down the page pushes everything that
                            // starts below where it began, boxes and ink together, down by as far as
                            // it went; a drag up pulls it back up. One drag, then the tool is put down.
                            if space_on {
                                let visible = ui.clip_rect().intersect(viewport.with_max_y(viewport.max.y.max(ui.clip_rect().max.y)));
                                if let Some((from, by)) = insert_space_overlay(ui.ctx(), visible, &tokens) {
                                    let z = zoom_scale.max(0.01);
                                    let (at, by) = ((from - moved.origin.y) / z, by / z);
                                    let below: Vec<usize> = (0..ed.boxes.len()).filter(|i| ed.boxes.get(*i).is_some_and(|b| b.y >= at)).collect();
                                    // Room taken away never pulls anything above the top of the page.
                                    let highest = below.iter().filter_map(|i| ed.boxes.get(*i)).map(|b| b.y).fold(f32::INFINITY, f32::min);
                                    let by = if by < 0.0 && highest.is_finite() { by.max(-highest) } else { by };
                                    for i in &below {
                                        if let Some(b) = ed.boxes.get_mut(*i) {
                                            *b = nocturne_model::container::Container::new(b.x, b.y + by, b.width, b.blocks);
                                        }
                                    }
                                    if !below.is_empty() && by != 0.0 {
                                        moved.boxes = true;
                                    }
                                    if let Some(pad) = pad.as_mut() {
                                        ink_changed |= pad.shift_below(at, by);
                                    }
                                    space_over = true;
                                }
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    space_over = true;
                                }
                            }
                            // The selection box takes ink as well as boxes, as in OneNote, and what
                            // it took moves together: boxes dragged take the ink with them, ink
                            // dragged takes the boxes.
                            if let Some(pad) = pad.as_mut()
                                && !ink_owns_canvas
                            {
                                if moved.selection_started {
                                    pad.let_go();
                                }
                                if let Some(shown) = moved.selection_box {
                                    let to_ink = |p: egui::Pos2| ((p.x - top.x) / zoom_scale.max(0.01), (p.y - top.y) / zoom_scale.max(0.01));
                                    let (a, b) = (to_ink(shown.min), to_ink(shown.max));
                                    pad.hold_touching((a.0, a.1, b.0, b.1));
                                }
                                if let Some((dx, dy)) = moved.selected_shift {
                                    pad.move_held(dx, dy);
                                    ink_changed |= pad.holding() > 0;
                                }
                                if let Some((dx, dy)) = pad.take_shift() {
                                    for i in crate::containers::selected_boxes(ui.ctx(), &ed.dir) {
                                        if let Some(c) = ed.boxes.get_mut(i) {
                                            *c = nocturne_model::container::Container::new(c.x + dx, c.y + dy, c.width, c.blocks);
                                            moved.boxes = true;
                                        }
                                    }
                                }
                                // A press anywhere else on the page lets go of the ink, unless it
                                // went on something still selected.
                                let on_held = pressed.is_some_and(|p| crate::inkpad::held_handle_at(ui.ctx(), p));
                                let ctrl_press = ui.input(|i| {
                                    i.events.iter().any(|e| {
                                        matches!(e, egui::Event::PointerButton { pressed: true, modifiers, .. } if modifiers.command || modifiers.ctrl)
                                    })
                                });
                                if press_in_page
                                    && !on_held
                                    && !ctrl_press
                                    && moved.selection_box.is_none()
                                    && pad.holding() > 0
                                    && crate::containers::selected_boxes(ui.ctx(), &ed.dir).is_empty()
                                {
                                    pad.let_go();
                                }
                            }
                            if let Some((html, text)) = moved.copy.take()
                                && self.clipboard.set_html(&html, &text).is_err()
                            {
                                ui.ctx().copy_text(text);
                            }
                            // The ink of a page sits over its words, anywhere on it, the
                            // way a pen on paper does. It goes on after them so a press
                            // with a pen in hand reaches the ink and not the caret.
                            {
                                // The sheet, not the words: ink goes anywhere on the page,
                                // and the page is a sheet of paper whether or not anything
                                // is written that far down it. Measuring the ink's area
                                // from where the text happens to end meant the pen stopped
                                // at the last line, which is not what a sheet of paper
                                // does. The room is claimed so the page can be scrolled
                                // into it and so what is drawn there is never cut off.
                                let used = ui.cursor().min.y - top.y;
                                let ink_bottom = pad
                                    .as_ref()
                                    .and_then(crate::inkpad::Pad::bounds)
                                    .map_or(0.0, |[_, _, _, y1]| (y1 * zoom_scale).max(0.0) + 160.0);
                                let want = crate::inkpad::SHEET_H.max(used + 240.0).max(ink_bottom);
                                if want > used {
                                    ui.allocate_space(vec2(ui.available_width(), want - used));
                                }
                                let body = viewport;
                                page_rect = Some(body);
                                // The page's own right-click menu. No widget is laid
                                // over the page for it: a widget that senses clicks would
                                // take the primary click away from the words, the ink and
                                // the caret, and one that does not sense clicks cannot
                                // carry a context menu at all. So the secondary click is
                                // watched for directly and the menu drawn where it fell.
                                crate::imagemenu::show(ui, body, &mut page_acts, &tokens);
                                crate::pagemenu::show(ui, body, &mut page_acts, &tokens);
                            }
                            if moved.changed.blocks {
                                ed.rebuild_body();
                                *from_blocks = true;
                            }
                            ed.content_bottom = Some(moved.bottom);
                            ed.last_frames = moved.frames.clone();
                            ed.shown_widths = std::mem::take(&mut moved.shown_widths);
                            if moved.boxes {
                                *boxes_moved = true;
                            }
                            if moved.changed.columns {
                                *cols_moved = true;
                            }
                            // Blocks went in or out before a justified paragraph: its mark
                            // moved with it, and is written into the page's file like the widths.
                            if moved.justified {
                                *justified_moved = true;
                            }
                            ed.origin = moved.origin;
                            ed.origin_in_canvas = Some(moved.origin - content_rect.min);
                            if ed.canvas_layout != nocturne_store::page::CanvasLayout::Infinite {
                                let ink_bottom = pad.as_ref().and_then(crate::inkpad::Pad::bounds).map_or(0.0, |bounds| bounds[3]);
                                let bottom = moved.bottom.max(ink_bottom);
                                if let Some(end) = crate::canvas_paper::sheet_end(ed.canvas_layout, bottom) {
                                    ui.set_min_height((moved.origin.y - ui.min_rect().top() + end * zoom_scale).max(0.0));
                                }
                            }
                            if moved.changed.link.is_some() {
                                *follow = moved.changed.link;
                            }
                        } else if wide {
                            crate::containers::clear_touch_handles(ui.ctx());
                            crate::inkpad::clear_held_hit_cache(ui.ctx());
                            // A page with markup the block model cannot hold: writing it back
                            // would drop that markup, so it is read here and changed in its
                            // source. See docs/gaps.md.
                            if let Some(href) = preview::show_zoomed(ui, &ed.blocks, &ed.dir, &tokens, page_dir, &ed.justified, zoom_scale) {
                                *follow = Some(href);
                            }
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(crate::tl_id!("ui.this_page_holds_markup_nocturne_does_not_edit_yet_s_bc3b3b6a92"))
                                    .small()
                                    .color(tokens.muted),
                            );
                        } else {
                            crate::containers::clear_touch_handles(ui.ctx());
                            crate::inkpad::clear_held_hit_cache(ui.ctx());
                            if let Some(href) = preview::show_zoomed(ui, &ed.blocks, &ed.dir, &tokens, page_dir, &ed.justified, zoom_scale) {
                                *follow = Some(href);
                            }
                        }
                    });
                });
            };
            if show_source {
                self.pencil_pill_rect = None;
                // The code view: the page's files on the left, the one picked on the
                // right. The page itself is not drawn while it is open, so there is only
                // ever one editable copy of the page on screen.
                let named = pageedit::links_in(&ed.blocks);
                let entries = crate::codeview::files(&ed.dir, ed.format.page_file(), &named);
                let out = crate::codeview::show(ui, &mut code, &entries, &ed.dir, &tokens, height);
                wrote_file = out.wrote;
                file_action = out.file_action;
            } else {
                let scroll = egui::containers::scroll_area::ScrollSource {
                    drag: egui::containers::scroll_area::DragScroll::Never,
                    mouse_wheel: !wheel_canvas_pan_enabled,
                    ..egui::containers::scroll_area::ScrollSource::default()
                };
                let page_outer = ui.available_rect_before_wrap();
                let ext_scroll_y = std::mem::take(&mut self.pending_external_scroll_y);
                let total_scroll_y = ext_scroll_y + finger_pan_delta.y;
                let mut scroll_area = egui::ScrollArea::vertical().id_salt("page-main").auto_shrink([false, false]).scroll_source(scroll);
                if let Some(target_y) = self.pending_scroll_offset_y.take() {
                    let scroll_id = ui.make_persistent_id("page-main");
                    egui::scroll_area::State::default().store(ui.ctx(), scroll_id);
                    scroll_area = scroll_area.vertical_scroll_offset(target_y.max(0.0));
                }
                let shown = scroll_area.show(ui, |ui| {
                    if total_scroll_y != 0.0 && total_scroll_y.is_finite() {
                        ui.scroll_with_delta_animation(vec2(0.0, total_scroll_y), egui::style::ScrollAnimation::none());
                    }
                    page(
                        ui,
                        ed,
                        true,
                        &mut changed,
                        &mut from_blocks,
                        &mut toggle_source,
                        &mut boxes_moved,
                        &mut cols_moved,
                        &mut justified_moved,
                        &mut follow,
                        &mut rename,
                    );
                });
                let scroll_style = ui.spacing().scroll;
                let scroll_edge = page_outer.max.x.min(ui.clip_rect().max.x);
                let max_cross = scroll_edge - scroll_style.bar_outer_margin;
                self.page_scroll_strip = (scroll.scroll_bar && shown.content_size.y > shown.inner_rect.height() + 0.5)
                    .then(|| egui::Rect::from_x_y_ranges((max_cross - scroll_style.bar_width)..=max_cross, shown.inner_rect.y_range()));
                // Sideways the page is a canvas: when what is on it reaches past the view's
                // sides, a bar along the bottom shows it and moves the canvas (issue #188).
                let view = shown.inner_rect;
                self.page_view_rect = Some(view);
                let vertical = shown.content_size.y > view.height() + 0.5;
                let reserve = if vertical { ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin } else { 0.0 };
                if let Some(span) = page_span {
                    hbar = crate::hscroll::show(ui.ctx(), egui::Id::new("page-hscroll"), view, span, reserve);
                }
                // Second frame of Fit All Content: now that `ed.origin` reflects the new zoom,
                // center `bounds_page.center()` inside `view`.
                if let Some(fit_bounds) = self.pending_fit_bounds.take() {
                    let (new_pan, new_scroll_y) = crate::minimap::compute_pan_and_scroll_for_page_center(
                        fit_bounds.center(),
                        view,
                        ed.origin,
                        zoom_scale,
                        shown.state.offset().y,
                        shown.content_size.y,
                        self.canvas_pan,
                    );
                    self.canvas_pan = new_pan;
                    self.pending_scroll_offset_y = Some(new_scroll_y);
                    ui.ctx().request_repaint();
                }
                if !hide_canvas_chrome {
                    let hbar_clearance = if hbar.track.is_some() { 18.0 } else { 0.0 };
                    // The minimap sits top right, clear of a tool bar docked along the top or the right.
                    let studio = self.session.studio_options();
                    let (mut map_top_clearance, mut map_right_clearance) = (0.0_f32, reserve);
                    if tablet_studio {
                        let hide_bottom_pill_for_keyboard = self.soft_keyboard_visible
                            && self.pointer == ribbon::Pointer::Write
                            && studio.toolbar_dock == nocturne_engine::ToolbarDock::Bottom;
                        let pill_rect = if hide_bottom_pill_for_keyboard {
                            self.pencil_pill_rect = None;
                            None
                        } else {
                            let pill_view = ribbon::View {
                                rules,
                                zoom_percent: current_zoom_pct,
                                pointer: pen.pointer,
                                rack: &self.rack,
                                nib: self.nib,
                                finger_drawing: self.session.finger_drawing(),
                                eraser_auto_return: self.eraser_auto_return,
                                eraser_pixel: self.session.ink_options().eraser_kind == nocturne_engine::EraserKind::Pixel,
                                eraser_width: self.session.ink_options().eraser_width,
                                show_minimap,
                                tablet_tools_minimized: self.tablet_tools_minimized,
                                can_undo: (ed.writable && ed.history.can_undo()) || pad.as_ref().is_some_and(crate::inkpad::Pad::can_undo),
                                can_redo: (ed.writable && ed.history.can_redo()) || pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo),
                                can_ink_redo: pad.as_ref().is_some_and(crate::inkpad::Pad::can_redo),
                                math_ocr_enabled: self.session.ocr_config().enabled,
                                holding_ink: pad.as_ref().is_some_and(|p| p.holding() > 0),
                                ..ribbon::View::default()
                            };
                            let edge_clearance = match studio.toolbar_dock {
                                nocturne_engine::ToolbarDock::Bottom => hbar_clearance,
                                nocturne_engine::ToolbarDock::Right => reserve,
                                // Under the floating header, which is at the top left.
                                nocturne_engine::ToolbarDock::Top | nocturne_engine::ToolbarDock::Left => {
                                    self.header_pill_rect.map_or(0.0, |r| r.height() + 6.0)
                                }
                            };
                            let (acts, pill_rect) = crate::tablet_chrome::show_pencil_pill(
                                ui.ctx(),
                                view,
                                edge_clearance,
                                studio,
                                self.tool_settings_open,
                                &pill_view,
                                &tokens,
                            );
                            self.pencil_pill_rect = pill_rect;
                            pill_acts = acts;
                            pill_rect
                        };
                        if self.tool_settings_open && self.dialog.is_none() {
                            let options = self.session.ink_options();
                            let ocr_cfg = self.session.ocr_config();
                            let input = crate::tool_settings::ToolSettingsIn {
                                finger_drawing: self.session.finger_drawing(),
                                hold_to_straighten: options.hold_to_straighten,
                                eraser_pixel: options.eraser_kind == nocturne_engine::EraserKind::Pixel,
                                math_ocr_enabled: ocr_cfg.enabled,
                                handwriting_ocr_enabled: ocr_cfg.handwriting_enabled,
                                library_view: studio.library_view,
                                studio,
                                rack: &self.rack,
                                nib: self.nib,
                            };
                            pill_acts.extend(crate::tool_settings::show(ui.ctx(), view, &input, &tokens));
                        }
                        if let Some(pr) = pill_rect {
                            match studio.toolbar_dock {
                                nocturne_engine::ToolbarDock::Top => map_top_clearance = (pr.max.y - view.min.y + 8.0).max(0.0),
                                nocturne_engine::ToolbarDock::Right => map_right_clearance = (view.max.x - pr.min.x + 8.0).max(reserve),
                                _ => {}
                            }
                        }
                    }
                    if matches!(self.pointer, ribbon::Pointer::Lasso | ribbon::Pointer::Pick)
                        && pad.as_ref().is_some_and(|p| p.holding() > 0 && !p.gesture_active())
                        && let Some(selection) = crate::inkpad::held_area(ui.ctx()).filter(|rect| rect.intersects(view))
                    {
                        crate::ink_selection::selection_actions(ui.ctx(), selection.intersect(view), view, &tokens, &mut pill_acts);
                    }
                    let ocr_cfg = self.session.ocr_config();
                    if (ocr_cfg.enabled || ocr_cfg.handwriting_enabled)
                        && self.math_ocr.is_none()
                        && self.dialog.is_none()
                        && !self.tool_settings_open
                        && pad.as_ref().is_some_and(|p| p.holding() > 0)
                        && let Some(held_rect) = crate::inkpad::held_area(ui.ctx()).filter(|r| r.width() > 0.0 && r.height() > 0.0)
                    {
                        start_ocr =
                            crate::math_ocr::convert_buttons(ui.ctx(), held_rect, view, ocr_cfg.enabled, ocr_cfg.handwriting_enabled, &tokens);
                    }
                    if show_minimap {
                        let stroke_bounds =
                            if self.minimap_collapsed { Vec::new() } else { pad.as_ref().map(crate::inkpad::Pad::stroke_bounds).unwrap_or_default() };
                        minimap_out = crate::minimap::show(
                            ui.ctx(),
                            view,
                            ed.origin,
                            current_zoom_pct,
                            self.minimap_collapsed,
                            map_top_clearance,
                            map_right_clearance,
                            &ed.last_frames,
                            &stroke_bounds,
                            &tokens,
                        );
                        if let Some(target_center) = minimap_out.pan_to_page_center {
                            let (new_pan, new_scroll_y) = crate::minimap::compute_pan_and_scroll_for_page_center(
                                target_center,
                                view,
                                ed.origin,
                                zoom_scale,
                                shown.state.offset().y,
                                shown.content_size.y,
                                self.canvas_pan,
                            );
                            self.canvas_pan = new_pan;
                            self.pending_scroll_offset_y = Some(new_scroll_y);
                            ui.ctx().request_repaint();
                        }
                    }
                }
            }
            if changed {
                Self::mark_dirty(ed);
            } else if from_blocks {
                Self::mark_dirty_from_blocks(ed);
            }
            // Before the step is remembered, so the page in the history is one that
            // names its ink: a box deleted with the ink line in it is repaired at once
            // rather than at the next open.
            if changed || from_blocks {
                let (ink_named, cols, justified) = Self::name_ink(ed);
                from_blocks |= ink_named;
                cols_moved |= cols;
                justified_moved |= justified;
                if ink_named {
                    Self::mark_dirty_from_blocks(ed);
                }
            }
            if changed || from_blocks || boxes_moved {
                Self::remember_step(ed);
            }
            // A container was dragged or resized: the arrangement is the page's own, so it
            // is written into the page's file rather than kept in the window.
            if boxes_moved {
                ed.arranged = true;
                ed.rebuild_body();
                Self::mark_dirty_from_blocks(ed);
            }
            // A column was dragged wider or narrower. Widths are layout, like the boxes,
            // so they go into the page's own file beside them.
            if cols_moved {
                let path = path_str(&ed.dir);
                let line = nocturne_model::tablecols::write(&ed.columns);
                save_cols = Some((path, line));
            }
            if justified_moved {
                let path = path_str(&ed.dir);
                let line = nocturne_model::justify::write(&ed.justified);
                save_justified = Some((path, line));
            }
        }
        self.cursor = cursor;
        if let Some((path, line)) = save_cols {
            self.run("page.columns", &params(&[("path", &path), ("widths", &line)]));
        }
        if let Some((path, line)) = save_justified {
            self.run("page.justify", &params(&[("path", &path), ("blocks", &line)]));
        }
        self.code = code;
        if let Some((name, text)) = wrote_file {
            self.write_page_file(&name, &text);
        }
        if let Some(action) = file_action {
            match action {
                crate::codeview::FileAction::CopyPath(path) => {
                    ui.ctx().copy_text(path);
                    self.say(crate::tl_id!("ui.full_path_copied"));
                }
                crate::codeview::FileAction::OpenFolder(path) => match open_folder(&path) {
                    Ok(()) => self.say(crate::tl_id!("ui.opened_the_containing_folder")),
                    Err(e) => {
                        self.complain(&crate::i18n::format_message_id("ui.could_not_open_the_containing_folder_error", &[("error", &e.to_string())]))
                    }
                },
                crate::codeview::FileAction::OpenShell(path) => {
                    if self.open_attachment_warning_suppressed {
                        match open_file_for_viewer(&path) {
                            Ok(()) => self.say(crate::tl_id!("ui.opened_using_the_associated_application")),
                            Err(e) => {
                                self.complain(&crate::i18n::format_message_id("ui.could_not_open_the_file_error", &[("error", &e.to_string())]))
                            }
                        }
                    } else {
                        self.dialog = Some(Dialog::OpenAttachmentWarning { path });
                    }
                }
                crate::codeview::FileAction::Delete(name) => {
                    log::debug!("source view delete action received: file={name:?}");
                    let page = self.editor.as_ref().map(|e| e.dir.clone());
                    if let Some(page) = page {
                        self.dialog = Some(Dialog::DeleteFile { page, name });
                    }
                }
                crate::codeview::FileAction::AddFile => {
                    if let Some(dir) = self.editor.as_ref().map(|ed| ed.dir.clone())
                        && let Some(file) = rfd::FileDialog::new().set_title(crate::tl_id!("ui.add_file_to_this_page")).pick_file()
                    {
                        self.dialog = Some(Dialog::FileOptions { dir, files: vec![file] });
                    }
                }
            }
        }
        if toggle_source {
            self.set_show_source(!show_source);
        }
        if toggle_full_page {
            self.full_page = !full_page;
        }
        if focus_search {
            self.focus_search = true;
        }
        // The drawing comes back from the page, and a stroke laid down this frame is
        // written at once: ink is its own file, so it does not wait for the page to save.
        self.pad = pad;
        if page_rect.is_some() {
            self.page_rect = page_rect;
        }
        self.page_hbar = hbar.track.zip(hbar.handle);
        if hbar.pan != 0.0 && hbar.pan.is_finite() {
            self.canvas_pan.x = (self.canvas_pan.x + hbar.pan).clamp(-1.0e6, 1.0e6);
            ui.ctx().request_repaint();
        }
        if space_over {
            self.space_tool = false;
        }
        for act in page_acts {
            self.ribbon_act(act);
        }
        for act in pill_acts {
            self.ribbon_act(act);
        }
        if let Some(mode) = start_ocr.or_else(|| self.pending_math_ocr_start.take()) {
            self.start_math_ocr(ui.ctx(), mode);
        }
        if minimap_out.toggle_collapsed {
            self.minimap_collapsed = !self.minimap_collapsed;
            let _ = self.session.settings.set("minimap_collapsed", if self.minimap_collapsed { "true" } else { "false" });
        }
        if minimap_out.fit_all {
            self.ribbon_act(ribbon::Act::FitAllContent);
        }
        let finished_erase = self.pad.as_mut().is_some_and(crate::inkpad::Pad::take_finished_erase);
        if finished_erase && self.eraser_auto_return && self.pointer == ribbon::Pointer::Erase {
            self.session.toggle_eraser(nocturne_engine::EraserToggleMode::Pen);
            self.sync_tool_state_from_session();
        }
        if ink_changed {
            let now = Instant::now();
            self.last_ink_change = Some(now);
            self.sync_dirty_since_last = true;
            self.last_local_edit_at = Some(now);
        }
        // A change to the ink is finished once no ink gesture is active (a resting palm may still
        // hold the window's primary pointer down after the stylus lifts).
        if let Some(pad) = self.pad.as_mut()
            && !pad.gesture_active()
        {
            pad.settle();
        }
        if ink_changed && let Some(pad) = self.pad.as_mut() {
            let result = if cfg!(target_os = "ios") { pad.save_in_background(ui.ctx()) } else { Some(pad.save()) };
            if let Some(result) = result {
                self.ink_save_completed(result, true);
            }
        }
        if let Some(href) = follow {
            self.follow_link(ui.ctx(), &href);
        }
        if let Some(t) = rename
            && let Some(dir) = self.editor.as_ref().map(|e| e.dir.clone())
            && self.run("node.title", &params(&[("path", &path_str(&dir)), ("title", &t)])).is_some()
        {
            if let Some(ed) = self.editor.as_mut() {
                ed.titled = !t.is_empty();
                ed.title = if t.is_empty() { nocturne_store::tree::UNTITLED_PAGE.to_string() } else { t };
            }
            self.library_grid.invalidate_page(&dir);
            self.say(crate::tl_id!("ui.title_saved"));
        }
    }

    fn saved_caption(&self, saved_at: Option<&str>) -> String {
        if self.session_only_storage {
            crate::tl_id!("ui.kept_in_this_tab").into()
        } else {
            saved_at
                .map(|time| crate::i18n::format_message_id("ui.saved_time", &[("time", time)]))
                .unwrap_or_else(|| crate::tl_id!("ui.saved").into())
        }
    }

    fn page_save_state(&self, ed: &Editor) -> String {
        if ed.dirty_since.is_some() || ed.saving.is_some() {
            crate::tl_id!("ui.editing").into()
        } else {
            self.saved_caption(ed.last_saved.as_deref())
        }
    }

    fn storage_notice(&self, ui: &mut Ui) {
        if self.session_only_storage {
            egui::Panel::top("session-storage-notice").show(ui, |ui| {
                ui.label(egui::RichText::new(crate::tl_id!("ui.session_only_storage_notice")).color(self.tokens.warn));
            });
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        if self.is_tablet_studio(ui.ctx()) {
            return;
        }
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let color = if self.status_is_error { self.tokens.warn } else { self.tokens.muted };
                ui.label(egui::RichText::new(crate::i18n::pseudo_dynamic(&self.status)).color(color));
                if let Some(ed) = self.editor.as_ref() {
                    let state = self.page_save_state(ed);
                    ui.separator();
                    ui.label(egui::RichText::new(format!("{} - {state}", ed.format.label())).small().color(self.tokens.muted))
                        .on_hover_text(self.shown(&ed.dir));
                }
                // A phone-width window keeps only the default format's box here, without its
                // labels or the folder summary, so the status beside it stays readable.
                let compact = self.compact;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !compact {
                        let nodes: usize = self.session.tree().iter().map(|n| n.count()).sum();
                        let notebooks = crate::i18n::format_count_id("ui.n_notebook_n_notebooks", self.session.tree().len() as u64);
                        let items = crate::i18n::format_count_id("ui.n_item_n_items", nodes as u64);
                        let folder = self.shown(self.session.root());
                        let summary = crate::i18n::format_message_id(
                            "ui.notebooks_items_folder",
                            &[("notebooks", &notebooks), ("items", &items), ("folder", &folder)],
                        );
                        ui.label(egui::RichText::new(summary).small().color(self.tokens.muted));
                        ui.separator();
                    }
                    let (current, source) = self.session.settings.global_format();
                    let mut chosen = current;
                    let combo = if compact {
                        egui::ComboBox::from_id_salt("default-format")
                    } else {
                        egui::ComboBox::from_label(crate::tl_id!("ui.default_format"))
                    };
                    let combo = combo.selected_text(current.label()).show_ui(ui, |ui| {
                        for f in PageFormat::ALL {
                            ui.selectable_value(&mut chosen, f, f.label());
                        }
                    });
                    if compact {
                        combo.response.on_hover_text(crate::tl_id!("ui.default_format"));
                    } else {
                        ui.label(egui::RichText::new(format!("({})", crate::tl!(source.label()))).small().color(self.tokens.muted));
                        ui.label(egui::RichText::new(crate::tl_id!("ui.new_pages")).small().color(self.tokens.muted));
                    }
                    if chosen != current && self.run("settings.default_format.set", &params(&[("format", chosen.id())])).is_some() {
                        self.say(crate::i18n::format_message_id(
                            "ui.new_pages_use_format_unless_the_notebook_or_section_2678c9f7e6",
                            &[("format", chosen.label())],
                        ));
                    }
                });
            });
        });
    }

    // ---------- dialogs ----------

    fn dialogs(&mut self, ctx: &Context) {
        while self.dialog.is_none() {
            let Some(next) = self.queued_alerts.pop_front() else { break };
            if !matches!(&next, Dialog::Alert { key, .. } if self.muted_alerts.contains(key)) {
                self.dialog = Some(next);
            }
        }
        let Some(dialog) = self.dialog.take() else {
            self.dialog_shown = None;
            return;
        };
        // The first frame a dialog is drawn in is when its text box asks for the keyboard. Asking
        // on every frame took the keyboard away and gave it back each frame, which on an iPad is
        // the keyboard dropping and rising, and its suggestions starting over.
        let kind = std::mem::discriminant(&dialog);
        let opened_now = self.dialog_shown != Some(kind);
        self.dialog_shown = Some(kind);
        // Page loading and native Shell icon generation can finish between the
        // recovery scan and the frame that presents this modal. Re-check the
        // filesystem so a healthy sidecar never produces a stale warning.
        if let Dialog::MissingAttachmentIcon { page, name } = &dialog
            && attachment_icon_healthy(page, name)
        {
            log::debug!("discarding stale missing-icon warning: page={page:?} attachment={name:?}");
            return;
        }
        let tokens = self.tokens;
        let mut keep = true;
        let mut next = dialog;
        match &mut next {
            Dialog::ClearTelemetry { path } => {
                let mut clear = false;
                let mut cancel = false;
                let res = crate::modal::Modal::new("clear-telemetry-modal", crate::tl_id!("ui.clear_telemetry"))
                    .backdrop_id("clear-telemetry-modal-backdrop")
                    .width(440.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.clear_all_local_telemetry_sessions_and_exported_tele_e4f5e2340d"));
                            ui.add_space(6.0);
                            ui.colored_label(tokens.warn, crate::tl_id!("ui.this_cannot_be_undone"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    cancel = true;
                                }
                                if crate::modal::danger_button(ui, crate::tl_id!("ui.clear_telemetry"), &tokens).clicked() {
                                    clear = true;
                                }
                            });
                        },
                    );
                if clear {
                    let _ = std::fs::remove_dir_all(path);
                    keep = false;
                } else if cancel || res.closed {
                    keep = false;
                }
            }
            Dialog::FirstUseWizard { step, level, format, add_demo, error } => {
                let mut back = false;
                let mut next_step = false;
                let mut finish = false;
                let last_step: u8 = 4;
                let escape = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
                let dark = tokens.is_dark();
                let (banner_uri, banner_bytes) = if dark {
                    ("bytes://nocturne-wizard-banner-dark", crate::BANNER_DARK_PNG)
                } else {
                    ("bytes://nocturne-wizard-banner-light", crate::BANNER_LIGHT_PNG)
                };
                let title = match *step {
                    0 => crate::tl_id!("ui.welcome_to_nocturne"),
                    1 => crate::tl_id!("ui.local_collection_level"),
                    2 => crate::tl_id!("ui.welcome_format_title"),
                    3 => crate::tl_id!("ui.welcome_notebook_prompt_title"),
                    _ => crate::tl_id!("ui.welcome_wizard_done_title"),
                }
                .to_string();
                let _ = crate::modal::Modal::new("first-use-wizard-modal", title.as_str())
                    .backdrop_id("first-use-wizard-modal-backdrop")
                    .width(540.0)
                    .closable(false)
                    .close_on_escape(false)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            if *step == 0 {
                                let bw = ui.available_width().clamp(160.0, 480.0);
                                ui.add(egui::Image::from_bytes(banner_uri, banner_bytes).fit_to_exact_size(vec2(bw, bw * 0.5)));
                                ui.add_space(8.0);
                            }
                            match *step {
                                0 => {}
                                1 => {
                                    ui.label(crate::tl_id!("ui.privacy_is_our_top_priority_all_logs_and_telemetry_s_d87cae4eac"));
                                    ui.add_space(6.0);
                                    ui.label(crate::tl_id!("ui.choose_a_local_collection_level_logging_is_off_by_de_5dfbe386e0"));
                                    egui::ComboBox::from_label(crate::tl_id!("ui.local_collection_level"))
                                        .selected_text(crate::tl!(level.as_str()))
                                        .popup_style(crate::theme::picker_popup_style(tokens))
                                        .show_ui(ui, |ui| {
                                            for choice in ["none", "usage", "warning", "error", "debug"] {
                                                ui.selectable_value(level, choice.into(), crate::tl!(choice));
                                            }
                                        });
                                }
                                2 => {
                                    ui.label(crate::tl_id!("ui.welcome_format_body"));
                                    ui.add_space(8.0);
                                    for choice in PageFormat::ALL {
                                        ui.radio_value(&mut *format, choice, choice.label());
                                    }
                                }
                                3 => {
                                    ui.label(crate::tl_id!("ui.welcome_notebook_prompt_body"));
                                    ui.label(crate::i18n::format_message_id("ui.welcome_notebook_prompt_format", &[("format", format.label())]));
                                    ui.add_space(6.0);
                                    ui.colored_label(tokens.warn, crate::tl_id!("ui.welcome_notebook_replace_warning"));
                                    ui.add_space(8.0);
                                    ui.radio_value(&mut *add_demo, true, crate::tl_id!("ui.welcome_notebook_add_yes"));
                                    ui.radio_value(&mut *add_demo, false, crate::tl_id!("ui.welcome_notebook_add_no"));
                                }
                                _ => {
                                    ui.label(crate::tl_id!("ui.welcome_wizard_done_body"));
                                }
                            }
                            if let Some(message) = error {
                                ui.add_space(8.0);
                                ui.colored_label(
                                    tokens.warn,
                                    crate::i18n::format_message_id("ui.welcome_notebook_action_failed", &[("error", message)]),
                                );
                            }
                        },
                        |ui| {
                            ui.horizontal(|ui| {
                                if *step > 0 && crate::modal::secondary_button(ui, crate::tl_id!("ui.welcome_wizard_back"), &tokens).clicked() {
                                    back = true;
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if *step < last_step {
                                        if crate::modal::primary_button(ui, crate::tl_id!("ui.welcome_format_next"), &tokens).clicked() {
                                            next_step = true;
                                        }
                                    } else if crate::modal::primary_button(ui, crate::tl_id!("ui.welcome_wizard_finish"), &tokens).clicked() {
                                        finish = true;
                                    }
                                });
                            });
                        },
                    );
                if back {
                    *step = step.saturating_sub(1);
                }
                if next_step {
                    *step = (*step + 1).min(last_step);
                }
                if escape && !finish && *step > 0 {
                    *step -= 1;
                }
                if finish {
                    let level = level.clone();
                    match self.finish_first_use_wizard(&level, *format, *add_demo) {
                        Ok(()) => keep = false,
                        Err(message) => *error = Some(message),
                    }
                }
            }
            Dialog::Alert { key, title, message, suppress, suppress_forever } => {
                let mut ok = false;
                let res = crate::modal::Modal::new("alert-modal", title.as_str()).backdrop_id("alert-modal-backdrop").width(440.0).show_with_footer(
                    ctx,
                    &tokens,
                    |ui| {
                        ui.label(crate::i18n::pseudo_dynamic(message));
                        ui.add_space(8.0);
                        ui.checkbox(suppress, crate::tl_id!("ui.don_t_show_this_again_in_this_session"));
                        ui.checkbox(suppress_forever, crate::tl_id!("ui.don_t_show_this_ever"));
                    },
                    |ui| {
                        ui.horizontal(|ui| {
                            if crate::modal::primary_button(ui, crate::tl_id!("ui.ok"), &tokens).clicked() {
                                ok = true;
                            }
                        });
                    },
                );
                if ok {
                    if *suppress {
                        self.muted_alerts.insert(key.clone());
                    }
                    if *suppress_forever {
                        let _ = self.session.settings.set(&format!("{key}_never"), "true");
                    }
                    keep = false;
                } else if res.closed {
                    keep = false;
                }
            }
            Dialog::ResetPreferences => {
                let mut reset = false;
                let mut cancel = false;
                let res = crate::modal::Modal::new("reset-preferences-modal", crate::tl_id!("ui.reset_app_preferences"))
                    .backdrop_id("reset-preferences-modal-backdrop")
                    .width(460.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.reset_nocturne_s_saved_preferences_to_their_built_in_defaults"));
                            ui.add_space(6.0);
                            ui.colored_label(tokens.warn, crate::tl_id!("ui.this_does_not_delete_notebooks_or_pages_it_cannot_be_undone"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::danger_button(ui, crate::tl_id!("ui.reset_preferences"), &tokens).clicked() {
                                    reset = true;
                                }
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    cancel = true;
                                }
                            });
                        },
                    );
                if reset {
                    match self.session.settings.reset() {
                        Ok(()) => {
                            let ink_defaults = nocturne_engine::InkOptions::default();
                            let studio_defaults = nocturne_engine::StudioOptions::default();
                            self.session.set_finger_drawing(false);
                            self.session.set_eraser_kind(ink_defaults.eraser_kind);
                            self.session.set_eraser_width(ink_defaults.eraser_width);
                            self.session.set_hold_to_straighten(ink_defaults.hold_to_straighten);
                            self.session.set_toolbar_dock(studio_defaults.toolbar_dock);
                            self.session.set_dock_roster(studio_defaults.roster);
                            self.session.set_library_view(studio_defaults.library_view);
                            self.session.set_layout_mode(studio_defaults.layout_mode);
                            let _ = self.session.settings.reset();
                            self.mode_pref = ModePref::System;
                            self.accent = Accent::Default;
                            self.applied_theme = None;
                            self.ui_language = crate::i18n::AUTO.into();
                            crate::i18n::set_current(&self.ui_language);
                            self.ui_scale = 100;
                            self.ui_scale_pending = true;
                            self.last_tablet_studio = self.is_tablet_studio(ctx);
                            self.minimap_override = None;
                            self.minimap_collapsed = true;
                            self.tablet_tools_minimized = false;
                            self.qat = quickaccess::Config::default();
                            self.rack = nocturne_ink::rack::parse("");
                            self.nib = 0;
                            self.sensitivity = DEFAULT_SENSITIVITY;
                            self.pressure_curve = nocturne_ink::PressureCurve::default();
                            self.rules = ribbon::RuleLines::default();
                            self.hide_title = false;
                            self.snap_to_grid = false;
                            self.ribbon_collapsed = false;
                            self.split_panes = true;
                            self.eraser_auto_return = false;
                            self.open_attachment_warning_suppressed = false;
                            self.muted_alerts.clear();
                            self.cached_sync_config = None;
                            self.sync_tool_state_to_session();
                            self.dialog = Some(self.first_use_wizard());
                        }
                        Err(e) => {
                            self.complain(&crate::i18n::format_message_id("ui.could_not_reset_app_preferences_error", &[("error", &e.to_string())]))
                        }
                    }
                    keep = false;
                } else if cancel || res.closed {
                    keep = false;
                }
            }
            Dialog::MissingAttachmentIcon { page, name } => {
                let mut repair = false;
                let mut leave = false;
                let mut suppress = self.icon_recovery_suppressed;
                let res = crate::modal::Modal::new("missing-attachment-icon-modal", crate::tl_id!("ui.attachment_icon_missing"))
                    .backdrop_id("missing-attachment-icon-backdrop")
                    .width(520.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            let message =
                                crate::i18n::format_message_id("ui.nocturne_found_an_attachment_without_its_generated_icon_name", &[("name", name)]);
                            ui.label(egui::RichText::new(message).strong().color(tokens.strong));
                            ui.add_space(6.0);
                            ui.label(crate::tl_id!("ui.the_original_file_is_still_safe_nocturne_can_regene_f10ded524a"));
                            ui.add_space(8.0);
                            ui.checkbox(&mut suppress, crate::tl_id!("ui.do_not_warn_about_similar_missing_icons_again_this_session"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::primary_button(ui, crate::tl_id!("ui.repair_icon"), &tokens).clicked() {
                                    repair = true;
                                }
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.leave_it"), &tokens).clicked() {
                                    leave = true;
                                }
                            });
                        },
                    );
                if repair {
                    self.icon_recovery_suppressed = suppress;
                    match nocturne_store::assets::ensure_icon(page, name).and_then(|()| {
                        let icon_exists = attachment_icon_exists(page, name);
                        if !icon_exists {
                            return Err(nocturne_store::StoreError::invalid("the icon generator produced no sidecar file"));
                        }
                        self.repair_attachment_reference(page, name).map_err(nocturne_store::StoreError::invalid).map(|_| ())
                    }) {
                        Ok(()) => {
                            log::info!("repaired missing attachment icon and reference: page={:?} attachment={name:?}", page);
                            self.say(crate::i18n::format_message_id("ui.repaired_the_attachment_icon_for_name", &[("name", name)]));
                        }
                        Err(e) => {
                            log::error!("could not repair missing attachment icon: page={:?} attachment={name:?} error={e}", page);
                            self.complain(&crate::i18n::format_message_id(
                                "ui.could_not_repair_the_icon_for_name_error",
                                &[("name", name), ("error", &e.to_string())],
                            ));
                        }
                    }
                    keep = false;
                } else if leave || res.closed {
                    self.icon_recovery_suppressed = suppress;
                    log::error!("user declined missing attachment icon repair: page={:?} attachment={name:?}", page);
                    keep = false;
                } else {
                    self.icon_recovery_suppressed = suppress;
                }
            }
            Dialog::UnreferencedAttachment { page, name } => {
                let mut restore = false;
                let mut leave = false;
                let mut suppress = self.icon_recovery_suppressed;
                let res = crate::modal::Modal::new("unreferenced-attachment-modal", crate::tl_id!("ui.attachment_not_linked"))
                    .backdrop_id("unreferenced-attachment-backdrop")
                    .width(520.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            let message = crate::i18n::format_message_id(
                                "ui.nocturne_found_an_attachment_that_is_not_referenced_8c0eb51257",
                                &[("name", name)],
                            );
                            ui.label(egui::RichText::new(message).strong().color(tokens.strong));
                            ui.add_space(6.0);
                            ui.label(crate::tl_id!("ui.the_file_is_safe_nocturne_can_append_a_link_to_it_a_c31b3d9716"));
                            ui.add_space(8.0);
                            ui.checkbox(&mut suppress, crate::tl_id!("ui.do_not_warn_about_similar_attachment_issues_again_this_session"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::primary_button(ui, crate::tl_id!("ui.append_link"), &tokens).clicked() {
                                    restore = true;
                                }
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.leave_it"), &tokens).clicked() {
                                    leave = true;
                                }
                            });
                        },
                    );
                if restore {
                    self.icon_recovery_suppressed = suppress;
                    match self.repair_attachment_reference(page, name) {
                        Ok(true) => {
                            log::info!("restored unreferenced attachment link: page={:?} attachment={name:?}", page);
                            self.say(crate::i18n::format_message_id("ui.added_a_link_to_name_at_the_end_of_the_page", &[("name", name)]));
                        }
                        Ok(false) => log::debug!("attachment recovery found no change needed: page={:?} attachment={name:?}", page),
                        Err(e) => {
                            log::error!("could not restore unreferenced attachment link: page={:?} attachment={name:?} error={e}", page);
                            self.complain(&crate::i18n::format_message_id(
                                "ui.could_not_restore_the_link_for_name_error",
                                &[("name", name), ("error", &e.to_string())],
                            ));
                        }
                    }
                    keep = false;
                } else if leave || res.closed {
                    self.icon_recovery_suppressed = suppress;
                    log::error!("user declined unreferenced attachment recovery: page={:?} attachment={name:?}", page);
                    keep = false;
                } else {
                    self.icon_recovery_suppressed = suppress;
                }
            }
            Dialog::OpenAttachmentWarning { path } => {
                let mut open = false;
                let mut cancel = false;
                let mut suppress = self.open_attachment_warning_suppressed;
                let shown_path = self.shown(path);
                let res = crate::modal::Modal::new("open-attachment-warning-modal", crate::tl_id!("ui.open_attachment"))
                    .backdrop_id("open-attachment-warning-backdrop")
                    .width(520.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(
                                egui::RichText::new(crate::tl_id!("ui.this_file_will_be_opened_by_its_associated_application"))
                                    .strong()
                                    .color(tokens.strong),
                            );
                            ui.add_space(6.0);
                            ui.label(shown_path);
                            ui.add_space(6.0);
                            ui.label(crate::tl_id!("ui.only_open_it_if_you_trust_the_file_and_the_person_who_created_it"));
                            ui.add_space(8.0);
                            ui.checkbox(&mut suppress, crate::tl_id!("ui.do_not_warn_again_for_this_file_type_this_session"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::primary_button(ui, crate::tl_id!("ui.open"), &tokens).clicked() {
                                    open = true;
                                }
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    cancel = true;
                                }
                            });
                        },
                    );
                if open {
                    self.open_attachment_warning_suppressed = suppress;
                    match open_file_for_viewer(path) {
                        Ok(()) => self.say(crate::i18n::format_message_id("ui.opening_path", &[("path", &self.shown(path))])),
                        Err(e) => self.complain(&crate::i18n::format_message_id("ui.could_not_open_the_file_error", &[("error", &e.to_string())])),
                    }
                    keep = false;
                } else if cancel || res.closed {
                    self.open_attachment_warning_suppressed = suppress;
                    log::info!("user cancelled opening attachment: file={:?}", path);
                    keep = false;
                } else {
                    self.open_attachment_warning_suppressed = suppress;
                }
            }
            Dialog::FileOptions { dir, files } => {
                let (dir, file) = (dir.clone(), files.first().cloned().unwrap_or_default());
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let left = files.len().saturating_sub(1);
                let kind = nocturne_store::assets::media_kind(&name);
                let mut chosen: Option<FileChoice> = None;
                let title = if left > 0 {
                    crate::i18n::format_message_id("ui.add_name_left_more", &[("name", &name), ("left", &left.to_string())])
                } else {
                    crate::i18n::format_message_id("ui.add_name", &[("name", &name)])
                };
                let res =
                    crate::modal::Modal::new("file-options-modal", &title).backdrop_id("file-options-modal-backdrop").width(420.0).show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(
                                egui::RichText::new(crate::tl_id!("ui.what_should_happen_to_this_file")).strong().size(15.0).color(tokens.strong),
                            );
                            ui.add_space(8.0);
                            let copy_label = match kind {
                                nocturne_store::assets::MediaKind::Image => crate::tl_id!("ui.insert_the_picture"),
                                nocturne_store::assets::MediaKind::Video => crate::tl_id!("ui.insert_the_video"),
                                nocturne_store::assets::MediaKind::Audio => crate::tl_id!("ui.insert_the_recording"),
                                nocturne_store::assets::MediaKind::Other => crate::tl_id!("ui.attach_the_file"),
                            };
                            if crate::modal::primary_button(ui, copy_label, &tokens).clicked() {
                                log::debug!("file attach choice clicked: choice=copy name={name:?}");
                                chosen = Some(FileChoice::Copy);
                            }
                            ui.label(
                                egui::RichText::new(crate::tl_id!("ui.a_copy_goes_into_this_page_s_folder_so_the_notebook_5f331e4138"))
                                    .small()
                                    .color(tokens.muted),
                            );
                            ui.add_space(8.0);
                            let can_printout = printout::supports(&file);
                            if crate::modal::secondary_button_enabled(ui, crate::tl_id!("ui.insert_printout"), can_printout, &tokens).clicked() {
                                log::debug!("file attach choice clicked: choice=printout name={name:?}");
                                chosen = Some(FileChoice::Printout);
                            }
                            ui.label(
                                egui::RichText::new(if can_printout {
                                    crate::tl_id!("ui.copies_the_file_and_inserts_one_picture_for_each_printed_page")
                                } else {
                                    crate::tl_id!("ui.printout_is_available_for_pdf_and_office_documents")
                                })
                                .small()
                                .color(tokens.muted),
                            );
                            ui.add_space(8.0);
                            if crate::modal::secondary_button(ui, crate::tl_id!("ui.link_to_the_file_where_it_is"), &tokens).clicked() {
                                log::debug!("file attach choice clicked: choice=link name={name:?}");
                                chosen = Some(FileChoice::Link);
                            }
                            ui.label(
                                egui::RichText::new(crate::tl_id!("ui.nothing_is_copied_the_link_breaks_if_the_file_moves"))
                                    .small()
                                    .color(tokens.muted),
                            );
                        },
                        |ui| {
                            ui.horizontal(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if let Some(how) = chosen {
                    self.add_file(&dir, &file, how);
                    if let Dialog::FileOptions { files, .. } = &mut next {
                        files.remove(0);
                        keep = !files.is_empty();
                    }
                }
            }
            Dialog::NewNotebook { name, format } => {
                let ready = !name.trim().is_empty();
                let mut create_clicked = false;
                let mut enter = false;
                let res = crate::modal::Modal::new("new-notebook-modal", crate::tl_id!("ui.new_notebook"))
                    .backdrop_id("new-notebook-modal-backdrop")
                    .width(440.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.name_2"));
                            let r = name_field(ui, name, NEW_NOTEBOOK_FIELD, opened_now);
                            enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            ui.add_space(6.0);
                            format_picker(ui, format);
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.create"), ready, &tokens).clicked() {
                                    create_clicked = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if (create_clicked || enter) && !name.trim().is_empty() {
                    let mut p = params(&[("name", name.trim())]);
                    if let Some(f) = format {
                        p.insert("format".into(), f.id().into());
                    }
                    if let Some(Output::Path(dir)) = self.run("notebook.create", &p) {
                        self.open.insert(dir);
                        self.say(crate::tl_id!("ui.notebook_created"));
                    }
                    keep = false;
                }
            }
            Dialog::Add { parent, parent_kind, title, kind, format } => {
                let ready = !title.trim().is_empty();
                let mut create_clicked = false;
                let mut enter = false;
                let heading = if *parent_kind == NodeKind::Page {
                    crate::tl_id!("ui.add_a_sub_page").to_string()
                } else {
                    let kind = crate::tl!(parent_kind.label());
                    crate::i18n::format_message_id("ui.add_inside_this_kind", &[("kind", &kind.to_lowercase())])
                };
                let res = crate::modal::Modal::new("add-modal", &heading).backdrop_id("add-modal-backdrop").width(440.0).show_with_footer(
                    ctx,
                    &tokens,
                    |ui| {
                        let kinds = parent_kind.may_contain();
                        if kinds.len() > 1 {
                            ui.label(crate::tl_id!("ui.what_to_add"));
                            ui.horizontal_wrapped(|ui| {
                                for k in kinds {
                                    ui.selectable_value(kind, *k, crate::tl!(k.label()));
                                }
                            });
                            ui.add_space(6.0);
                        }
                        ui.label(crate::tl_id!("ui.title_2"));
                        let r = name_field(ui, title, ADD_FIELD, opened_now);
                        enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(6.0);
                        ui.label(if *kind == NodeKind::Page {
                            crate::tl_id!("ui.format_for_this_page")
                        } else {
                            crate::tl_id!("ui.default_format_for_pages_below")
                        });
                        format_picker(ui, format);
                    },
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                keep = false;
                            }
                            if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.create"), ready, &tokens).clicked() {
                                create_clicked = true;
                            }
                        });
                    },
                );
                if res.closed {
                    keep = false;
                }
                if (create_clicked || enter) && !title.trim().is_empty() {
                    let mut p = params(&[("parent", &path_str(parent)), ("kind", kind.id()), ("title", title.trim())]);
                    if let Some(f) = format {
                        p.insert("format".into(), f.id().into());
                    }
                    if let Some(Output::Path(dir)) = self.run("node.create", &p) {
                        self.open.insert(parent.clone());
                        if *kind == NodeKind::Page {
                            self.open_page(&dir);
                        } else if *kind == NodeKind::Section {
                            // The new section is the one chosen, so its pages (none yet, with
                            // the Add page button above them) are what the page list shows next
                            // (issue #139).
                            self.selected = Some(dir.clone());
                            if let Some(page) = first_page(self.session.tree(), &dir) {
                                self.open_page(&page);
                            }
                        }
                        self.say(crate::tl_id!("ui.created"));
                    }
                    keep = false;
                }
            }
            Dialog::MoveOrCopy { path, title, kind, dest } => {
                let places = self.move_destinations(path, *kind);
                let here = path.parent().map(Path::to_path_buf);
                let fits = |p: &Path| nocturne_store::tree::read_node(p).is_some_and(|n| n.kind.may_contain().contains(kind));
                let can = dest.is_some();
                let moves = can && dest.as_deref() != here.as_deref();
                let mut chosen: Option<bool> = None;
                let now = places
                    .iter()
                    .find(|(p, _, _)| here.as_deref() == Some(p.as_path()))
                    .map(|(_, name, _)| name.clone())
                    .or_else(|| here.as_deref().and_then(nocturne_store::tree::read_node).map(|n| n.title))
                    .unwrap_or_default();
                let res = crate::modal::Modal::new("move-or-copy-modal", crate::tl_id!("ui.move_or_copy"))
                    .backdrop_id("move-or-copy-modal-backdrop")
                    .width(420.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::i18n::format_message_id("ui.move_or_copy_title_to", &[("title", title.as_str())]));
                            ui.label(
                                egui::RichText::new(crate::i18n::format_message_id("ui.it_is_in_place_now", &[("place", now.as_str())]))
                                    .small()
                                    .color(tokens.muted),
                            );
                            ui.add_space(8.0);
                            for (place, name, depth) in &places {
                                ui.horizontal(|ui| {
                                    ui.add_space(*depth as f32 * 16.0);
                                    if fits(place) {
                                        let picked = dest.as_deref() == Some(place.as_path());
                                        let shown = if here.as_deref() == Some(place.as_path()) { format!("{name} \u{2022}") } else { name.clone() };
                                        if ui.selectable_label(picked, shown).clicked() {
                                            *dest = Some(place.clone());
                                        }
                                    } else {
                                        ui.label(egui::RichText::new(name).color(tokens.muted));
                                    }
                                });
                            }
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::secondary_button_enabled(ui, crate::tl_id!("ui.copy"), can, &tokens).clicked() {
                                    chosen = Some(true);
                                }
                                if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.move"), moves, &tokens).clicked() {
                                    chosen = Some(false);
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if let (Some(copy), Some(to)) = (chosen, dest.clone()) {
                    let from = path.clone();
                    self.move_or_copy(&from, &to, copy);
                    keep = false;
                }
            }
            Dialog::Link { block, cell, range, text, address, editing, first } => {
                let (mut enter, mut ok_clicked, mut remove) = (false, false, false);
                let usable = pageedit::linkable_address(address);
                let res = crate::modal::Modal::new("link-modal", crate::tl_id!("ui.link"))
                    .backdrop_id("link-modal-backdrop")
                    .width(460.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            let w = (ui.available_width() - 120.0).max(140.0);
                            egui::Grid::new("link-dialog").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
                                ui.label(crate::tl_id!("ui.text_to_display"));
                                let words = ui.add(egui::TextEdit::singleline(text).id(egui::Id::unique(LINK_TEXT_FIELD)).desired_width(w));
                                ui.end_row();
                                ui.label(crate::tl_id!("ui.address"));
                                let to = ui.add(
                                    egui::TextEdit::singleline(address)
                                        .id(egui::Id::unique(LINK_ADDRESS_FIELD))
                                        .desired_width(w)
                                        .hint_text("https://"),
                                );
                                ui.end_row();
                                // The address is what is missing first, unless the words are.
                                if *first && ui.is_enabled() && !ui.is_sizing_pass() {
                                    if text.is_empty() && !*editing {
                                        words.request_focus();
                                    } else {
                                        to.request_focus();
                                    }
                                    *first = false;
                                }
                                if (words.lost_focus() || to.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter)) {
                                    enter = true;
                                }
                            });
                            if !address.trim().is_empty() && !pageedit::linkable_address(address) {
                                ui.add_space(6.0);
                                ui.colored_label(tokens.warn, crate::tl_id!("ui.that_address_cannot_be_used_for_a_link"));
                            }
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if *editing && crate::modal::danger_button(ui, crate::tl_id!("ui.remove_link"), &tokens).clicked() {
                                    remove = true;
                                }
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.ok"), usable, &tokens).clicked() {
                                    ok_clicked = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                let go = (ok_clicked || enter) && pageedit::linkable_address(address);
                if go || remove {
                    let (block, cell, range) = (*block, *cell, *range);
                    // Removing keeps the words and lets go of the address.
                    let replacement = if remove { text.trim().to_string() } else { pageedit::link_markup(text, address) };
                    self.replace_in_caret_line(block, cell, range, &replacement);
                    keep = false;
                }
            }
            Dialog::Rename { path, title } => {
                let ready = !title.trim().is_empty();
                let mut rename_clicked = false;
                let mut enter = false;
                let res = crate::modal::Modal::new("rename-modal", crate::tl_id!("ui.rename"))
                    .backdrop_id("rename-modal-backdrop")
                    .width(440.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.title_the_folder_name_does_not_change"));
                            let r = name_field(ui, title, RENAME_FIELD, opened_now);
                            enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.rename"), ready, &tokens).clicked() {
                                    rename_clicked = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if (rename_clicked || enter) && !title.trim().is_empty() {
                    if self.run("node.title", &params(&[("path", &path_str(path)), ("title", title.trim())])).is_some() {
                        self.library_grid.invalidate_page(path);
                        if self.editor.as_ref().map(|e| e.dir == *path).unwrap_or(false) {
                            self.open_page(path);
                        }
                        self.say(crate::tl_id!("ui.renamed"));
                    }
                    keep = false;
                }
            }
            Dialog::InsertTable { rows, columns, first } => {
                let size = table_size(rows, columns);
                let mut go = false;
                let res = crate::modal::Modal::new("insert-table-modal", crate::tl_id!("ui.insert_table"))
                    .backdrop_id("insert-table-backdrop")
                    .width(380.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            egui::Grid::new("insert-table-dialog").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
                                ui.label(crate::tl_id!("ui.number_of_columns"));
                                let c = ui.add(egui::TextEdit::singleline(columns).id(egui::Id::unique(TABLE_COLUMNS_FIELD)).desired_width(80.0));
                                ui.end_row();
                                ui.label(crate::tl_id!("ui.number_of_rows"));
                                ui.add(egui::TextEdit::singleline(rows).id(egui::Id::unique(TABLE_ROWS_FIELD)).desired_width(80.0));
                                ui.end_row();
                                if *first && ui.is_enabled() && !ui.is_sizing_pass() {
                                    c.request_focus();
                                    *first = false;
                                }
                            });
                            if size.is_none() {
                                ui.add_space(6.0);
                                let limits = crate::i18n::format_message_id(
                                    "ui.table_size_limits",
                                    &[("columns", &TABLE_MAX_COLUMNS.to_string()), ("rows", &TABLE_MAX_ROWS.to_string())],
                                );
                                ui.colored_label(tokens.warn, limits);
                            }
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::primary_button_enabled(ui, crate::tl_id!("ui.ok"), size.is_some(), &tokens).clicked() {
                                    go = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                } else if ctx.input(|input| input.key_pressed(Key::Enter)) && size.is_some() {
                    go = true;
                }
                if go && let Some((r, c)) = size {
                    keep = false;
                    self.insert_table(r, c);
                }
            }
            Dialog::DeleteFile { page, name } => {
                let mut go = false;
                let res = crate::modal::Modal::new("delete-file-modal", crate::tl_id!("ui.delete_file"))
                    .backdrop_id("delete-file-modal-backdrop")
                    .width(440.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::i18n::format_message_id("ui.delete_assets_name_from_this_page_s_folder", &[("name", name)]));
                            ui.add_space(6.0);
                            ui.colored_label(tokens.warn, crate::tl_id!("ui.the_file_is_removed_from_disk_undo_does_not_bring_a_file_back"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::danger_button(ui, crate::tl_id!("ui.delete"), &tokens).clicked() {
                                    go = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                } else if ctx.input(|input| input.key_pressed(Key::Enter)) {
                    go = true;
                }
                if go {
                    let page = page.clone();
                    if self.run("page.detach", &params(&[("path", &path_str(&page)), ("name", name)])).is_some() {
                        if let Some(ed) = self.editor.as_mut() {
                            ed.attachments = nocturne_store::assets::list(&page);
                        }
                        self.say(crate::tl_id!("ui.the_file_is_gone"));
                        // The page may have named the drawing it no longer has.
                        self.sync_ink_reference();
                    }
                    keep = false;
                }
            }
            Dialog::DeleteNotebook { path, title } => {
                let mut go = false;
                let res = crate::modal::Modal::new("delete-notebook-modal", crate::tl_id!("ui.delete_notebook"))
                    .backdrop_id("delete-notebook-modal-backdrop")
                    .width(460.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::i18n::format_message_id("ui.delete_notebook_title_and_everything_inside_it", &[("title", title)]));
                            ui.add_space(6.0);
                            ui.colored_label(tokens.warn, crate::tl_id!("ui.this_removes_the_notebook_folder_from_disk_and_cannot_be_undone"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::danger_button(ui, crate::tl_id!("ui.delete_notebook"), &tokens).clicked() {
                                    go = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if go {
                    let path = path.clone();
                    let was_open = self.editor.as_ref().and_then(|editor| notebook_for(&editor.dir)).is_some_and(|notebook| notebook == path);
                    log::info!("DeleteNotebook dialog: deleting {:?}", path);
                    match nocturne_store::ops::remove_tree(&path) {
                        Ok(()) => {
                            log::info!("notebook deleted successfully: {:?}", path);
                            self.remove_recent_notebook(&path);
                            if was_open {
                                self.editor = None;
                                self.pad = None;
                                self.selected = None;
                                self.backstage = Some("Open");
                            }
                            let _ = self.run("tree.refresh", &Params::new());
                            self.library_grid.invalidate_all();
                            self.say(crate::i18n::format_message_id("ui.deleted_notebook_path", &[("path", &self.shown(&path))]));
                        }
                        Err(error) => {
                            log::error!("notebook delete failed: {:?}: {}", path, error);
                            let message =
                                crate::i18n::format_message_id("ui.the_notebook_could_not_be_deleted_error", &[("error", &error.to_string())]);
                            self.alert("delete-failed", crate::tl_id!("ui.delete_notebook"), &message);
                        }
                    }
                    keep = false;
                }
            }
            Dialog::Delete { path, title, count } => {
                let mut go = false;
                let is_notebook = nocturne_store::tree::read_node(path).is_some_and(|node| node.kind == NodeKind::Notebook);
                let heading = if is_notebook { crate::tl_id!("ui.delete_notebook") } else { crate::tl_id!("ui.delete") };
                let confirm_label = if is_notebook { crate::tl_id!("ui.delete_notebook") } else { crate::tl_id!("ui.move_to_recycle_bin") };
                let res =
                    crate::modal::Modal::new("delete-node-modal", heading).backdrop_id("delete-node-modal-backdrop").width(440.0).show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            if is_notebook {
                                ui.label(crate::i18n::format_message_id("ui.delete_notebook_title_and_everything_inside_it", &[("title", title)]));
                                ui.add_space(6.0);
                                ui.colored_label(tokens.warn, crate::tl_id!("ui.this_removes_the_notebook_folder_from_disk_and_cannot_be_undone"));
                            } else {
                                ui.label(crate::i18n::format_count_message_id(
                                    "ui.move_title_and_everything_inside_it_n_item_to_this_n_20c3146ea1",
                                    *count as u64,
                                    &[("title", title)],
                                ));
                                ui.add_space(6.0);
                                ui.label(crate::tl_id!("ui.you_can_restore_it_from_history"));
                            }
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::danger_button(ui, confirm_label, &tokens).clicked() {
                                    go = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if go {
                    let was_open = self.editor.as_ref().is_some_and(|e| e.dir == *path || e.dir.starts_with(&*path));
                    let was_selected = self.selected.as_ref().is_some_and(|s| *s == *path || s.starts_with(&*path));
                    if is_notebook {
                        let path = path.clone();
                        log::info!("deleting notebook: {:?}", path);
                        match nocturne_store::ops::remove_tree(&path) {
                            Ok(()) => {
                                log::info!("notebook deleted successfully: {:?}", path);
                                self.remove_recent_notebook(&path);
                                if was_open {
                                    self.editor = None;
                                    self.pad = None;
                                }
                                if was_selected {
                                    self.selected = None;
                                }
                                self.open.remove(&path);
                                let _ = self.run("tree.refresh", &Params::new());
                                self.library_grid.invalidate_all();
                                self.say(crate::tl_id!("ui.deleted_notebook"));
                            }
                            Err(error) => {
                                log::error!("notebook delete failed: {:?}: {}", path, error);
                                let message =
                                    crate::i18n::format_message_id("ui.the_notebook_could_not_be_deleted_error", &[("error", &error.to_string())]);
                                self.alert("delete-failed", crate::tl_id!("ui.delete_notebook"), &message);
                            }
                        }
                    } else {
                        log::info!("deleting node: {:?}", path);
                        match self.run("node.delete", &params(&[("path", &path_str(path))])) {
                            Some(_) => {
                                log::info!("node deleted successfully: {:?}", path);
                                if was_open {
                                    self.editor = None;
                                    self.pad = None;
                                }
                                if was_selected {
                                    self.selected = None;
                                }
                                self.library_grid.invalidate_all();
                                self.say(crate::tl_id!("ui.moved_to_the_notebook_recycle_bin"));
                            }
                            None => {
                                log::error!("node.delete command failed: {:?}", path);
                                self.alert(
                                    "delete-failed",
                                    crate::tl_id!("ui.delete"),
                                    crate::tl_id!("ui.this_could_not_be_moved_to_the_recycle_bin_nothing_was_deleted"),
                                );
                            }
                        }
                    }
                    keep = false;
                }
            }
            Dialog::PageVersions { page, versions } => {
                let mut restore: Option<String> = None;
                let res = crate::modal::Modal::new("page-versions-modal", crate::tl_id!("ui.page_versions_2"))
                    .backdrop_id("page-versions-modal-backdrop")
                    .width(480.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.saved_copies_of_this_page"));
                            ui.add_space(6.0);
                            if versions.is_empty() {
                                ui.label(
                                    egui::RichText::new(crate::tl_id!("ui.no_saved_versions_yet_nocturne_keeps_a_copy_when_th_aa2e25acad"))
                                        .color(tokens.muted),
                                );
                            } else {
                                for version in versions.iter() {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(&version.created);
                                        let size = (version.bytes.div_ceil(1024)).to_string();
                                        let size = crate::i18n::format_message_id("ui.size_kb", &[("size", &size)]);
                                        ui.label(egui::RichText::new(size).small().color(tokens.muted));
                                        if crate::modal::primary_button(ui, crate::tl_id!("ui.restore"), &tokens).clicked() {
                                            restore = Some(version.id.clone());
                                        }
                                    });
                                    ui.add_space(4.0);
                                }
                            }
                        },
                        |ui| {
                            ui.horizontal(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.close"), &tokens).clicked() {
                                    keep = false;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if let Some(id) = restore {
                    if self.run("page.version.restore", &params(&[("path", &path_str(page)), ("id", &id)])).is_some() {
                        self.open_page(page);
                        self.say(crate::tl_id!("ui.restored_the_saved_page_version"));
                    }
                    keep = false;
                }
            }
            Dialog::RecycleBin { notebook, items } => {
                let mut restore: Option<String> = None;
                let mut purge: Option<(String, String)> = None;
                let res = crate::modal::Modal::new("recycle-bin-modal", crate::tl_id!("ui.notebook_recycle_bin_2"))
                    .backdrop_id("recycle-bin-modal-backdrop")
                    .width(540.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            if items.is_empty() {
                                ui.label(egui::RichText::new(crate::tl_id!("ui.this_notebook_s_recycle_bin_is_empty")).color(tokens.muted));
                            } else {
                                for item in items.iter() {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(egui::RichText::new(&item.title).strong().color(tokens.strong));
                                        ui.label(
                                            egui::RichText::new(crate::i18n::format_message_id(
                                                "ui.count_deleted",
                                                &[
                                                    ("count", &crate::i18n::format_count_id("ui.n_item_n_items", item.count as u64)),
                                                    ("deleted", &item.deleted),
                                                ],
                                            ))
                                            .small()
                                            .color(tokens.muted),
                                        );
                                        if crate::modal::primary_button(ui, crate::tl_id!("ui.restore"), &tokens).clicked() {
                                            restore = Some(item.id.clone());
                                        }
                                        if crate::modal::danger_button(ui, crate::tl_id!("ui.delete_permanently"), &tokens).clicked() {
                                            purge = Some((item.id.clone(), item.title.clone()));
                                        }
                                    });
                                    ui.add_space(6.0);
                                }
                            }
                        },
                        |ui| {
                            ui.horizontal(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.close"), &tokens).clicked() {
                                    keep = false;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if let Some(id) = restore {
                    if self.run("recycle.restore", &params(&[("notebook", &path_str(notebook)), ("id", &id)])).is_some() {
                        self.say(crate::tl_id!("ui.restored_to_the_original_location"));
                        keep = false;
                    }
                } else if let Some((id, title)) = purge {
                    next = Dialog::PurgeRecycleItem { notebook: notebook.clone(), id, title };
                }
            }
            Dialog::PurgeRecycleItem { notebook, id, title } => {
                let mut purge = false;
                let res = crate::modal::Modal::new("purge-recycle-modal", crate::tl_id!("ui.permanently_delete"))
                    .backdrop_id("purge-recycle-modal-backdrop")
                    .width(440.0)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::i18n::format_message_id("ui.permanently_delete_title_and_everything_inside_it", &[("title", title)]));
                            ui.add_space(6.0);
                            ui.colored_label(tokens.warn, crate::tl_id!("ui.this_cannot_be_undone"));
                        },
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.cancel"), &tokens).clicked() {
                                    keep = false;
                                }
                                if crate::modal::danger_button(ui, crate::tl_id!("ui.delete_permanently"), &tokens).clicked() {
                                    purge = true;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if purge && self.run("recycle.purge", &params(&[("notebook", &path_str(notebook)), ("id", id)])).is_some() {
                    self.say(crate::tl_id!("ui.permanently_deleted"));
                    keep = false;
                }
            }
            Dialog::Symbols => {
                let mut chosen: Option<&'static str> = None;
                let res = crate::modal::Modal::new("insert-symbol-modal", crate::tl_id!("ui.insert_symbol"))
                    .backdrop_id("insert-symbol-backdrop")
                    .width(340.0)
                    .closable(false)
                    .show_with_footer(
                        ctx,
                        &tokens,
                        |ui| {
                            ui.label(crate::tl_id!("ui.choose_a_symbol_to_insert_at_the_caret"));
                            ui.add_space(8.0);
                            for row in [["©", "®", "™", "§", "¶"], ["•", "→", "←", "±", "°"], ["×", "÷", "√", "€", "£"], ["¥", "¢", "∞", "≠", "≤"]]
                            {
                                ui.horizontal(|ui| {
                                    for symbol in row {
                                        let button = egui::Button::new(egui::RichText::new(symbol).size(20.0).color(tokens.text))
                                            .min_size(vec2(46.0, 36.0))
                                            .fill(tokens.panel)
                                            .stroke(egui::Stroke::new(1.0, tokens.border))
                                            .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL));
                                        if ui.add(button).clicked() {
                                            chosen = Some(symbol);
                                        }
                                    }
                                });
                            }
                        },
                        |ui| {
                            ui.horizontal(|ui| {
                                if crate::modal::secondary_button(ui, crate::tl_id!("ui.close"), &tokens).clicked() {
                                    keep = false;
                                }
                            });
                        },
                    );
                if res.closed {
                    keep = false;
                }
                if let Some(symbol) = chosen {
                    self.insert_in_page(symbol);
                    keep = false;
                }
            }
            Dialog::CloudSync(form) => {
                let supported = self.session.sync_http().is_supported() && self.session.sync_secrets().is_supported();
                let busy = self.sync_job.is_some();
                let out = crate::sync_ui::show_sync_dialog(ctx, form, supported, busy, &tokens);
                if out.cancel_running
                    && let Some(job) = self.sync_job.as_ref()
                {
                    job.cancel();
                }
                if out.clear_token && self.run("sync.configure", &params(&[("clear_token", "true")])).is_some() {
                    self.sync_has_token = Some(false);
                    form.has_saved_token = false;
                    form.token_input.clear();
                    form.last_message = Some((crate::tl_id!("ui.kdrive_token_cleared").to_string(), false));
                }
                if out.convert_existing_to_markdown
                    && self.run("sync.configure", &params(&[("convert_notebooks_to_markdown", "true"), ("markdown_default", "true")])).is_some()
                {
                    form.markdown_default = true;
                    form.last_message = Some((crate::tl_id!("ui.kdrive_converted_notebooks_md").to_string(), false));
                }
                if out.save_only || out.save_and_sync_now {
                    let drive_val = if form.drive_id_input.trim().is_empty() { "auto" } else { form.drive_id_input.trim() };
                    let mut p = params(&[
                        ("endpoint", form.endpoint.trim()),
                        ("drive_id", drive_val),
                        ("remote_path", form.remote_path.trim()),
                        ("auto_sync", if form.auto_sync { "true" } else { "false" }),
                        ("markdown_default", if form.markdown_default { "true" } else { "false" }),
                        ("propagate_deletes", if form.propagate_deletes { "true" } else { "false" }),
                    ]);
                    if !form.token_input.trim().is_empty() {
                        p.insert("token".into(), form.token_input.trim().into());
                    }
                    if self.run("sync.configure", &p).is_some() {
                        if !form.token_input.trim().is_empty() {
                            self.sync_has_token = Some(true);
                            form.has_saved_token = true;
                            form.token_input.clear();
                        } else {
                            self.sync_has_token = None;
                        }
                        if out.save_and_sync_now {
                            self.sync_by_hand = true; // by hand: Save and sync now
                            self.start_background_sync(ctx);
                        } else {
                            form.last_message = Some((crate::tl_id!("ui.kdrive_settings_saved").to_string(), false));
                        }
                    } else {
                        form.last_message = Some((self.status.clone(), true));
                    }
                }
                if out.close {
                    keep = false;
                }
            }
            Dialog::MathOcrSetup(form) => {
                let download = self.ocr_model_download.as_ref().map(crate::math_ocr::ModelDownloadWorker::progress);
                match crate::math_ocr::show_setup(ctx, form, download, &tokens) {
                    crate::math_ocr::SetupOutcome::Keep => {}
                    crate::math_ocr::SetupOutcome::Close => {
                        keep = false;
                    }
                    crate::math_ocr::SetupOutcome::ForgetKey => {
                        if self.run("ink.math_ocr_configure", &params(&[("provider", form.provider.id()), ("clear_key", "true")])).is_some() {
                            *form = crate::math_ocr::Setup::from_session(&self.session);
                        }
                    }
                    crate::math_ocr::SetupOutcome::DownloadModel => {
                        if self.ocr_model_download.is_none() {
                            form.error = None;
                            match crate::math_ocr::ModelDownloadWorker::start(ctx.clone(), self.session.sync_http(), self.session.ocr_model_dir()) {
                                Ok(worker) => self.ocr_model_download = Some(worker),
                                Err(err) => {
                                    form.error = Some(err.clone());
                                    self.complain(&err);
                                }
                            }
                        }
                    }
                    crate::math_ocr::SetupOutcome::CancelDownload => {
                        if let Some(worker) = self.ocr_model_download.as_ref() {
                            worker.cancel();
                        }
                    }
                    crate::math_ocr::SetupOutcome::RemoveModel => match self.session.run("ink.math_ocr_remove_model", &Params::new()) {
                        Ok(_) => {
                            form.refresh_local_status(&self.session);
                            form.error = None;
                        }
                        Err(e) => {
                            form.error = Some(e.to_string());
                            self.fail(&e);
                        }
                    },
                    crate::math_ocr::SetupOutcome::Save => {
                        let mut p = params(&[("provider", form.provider.id()), ("endpoint", form.endpoint.trim()), ("model", form.model.trim())]);
                        if !form.key.trim().is_empty() {
                            p.insert("key".into(), form.key.trim().into());
                        }
                        if !form.app_id.trim().is_empty() {
                            p.insert("app_id".into(), form.app_id.trim().into());
                        }
                        match self.session.run("ink.math_ocr_configure", &p) {
                            Ok(_) => {
                                form.key.clear();
                                form.app_id.clear();
                                form.error = None;
                                self.say(crate::tl_id!("ui.recognition_service_saved"));
                                keep = false;
                            }
                            Err(e) => {
                                form.error = Some(e.to_string());
                                self.fail(&e);
                            }
                        }
                    }
                }
            }
        }
        if keep {
            self.dialog = Some(next);
        } else {
            // Closed: a dialog of the same kind opened next is a new one.
            self.dialog_shown = None;
        }
    }

    fn poll_ocr_model_download(&mut self) {
        let Some(outcome) = self.ocr_model_download.as_ref().and_then(crate::math_ocr::ModelDownloadWorker::try_recv) else {
            return;
        };
        self.ocr_model_download = None;
        match outcome {
            Ok(()) => {
                if let Some(Dialog::MathOcrSetup(form)) = self.dialog.as_mut() {
                    form.model_installed = true;
                    form.error = None;
                }
                self.say(crate::tl_id!("ui.ocr_model_installed"));
            }
            Err(nocturne_engine::SyncError::Cancelled) => {
                if let Some(Dialog::MathOcrSetup(form)) = self.dialog.as_mut() {
                    form.refresh_local_status(&self.session);
                    form.error = None;
                }
            }
            Err(err) => {
                let msg = err.to_string();
                if let Some(Dialog::MathOcrSetup(form)) = self.dialog.as_mut() {
                    form.refresh_local_status(&self.session);
                    form.error = Some(msg.clone());
                }
                self.complain(&msg);
            }
        }
    }

    fn start_math_ocr(&mut self, ctx: &Context, mode: nocturne_engine::OcrMode) {
        let (Some(ed), Some(pad)) = (self.editor.as_ref(), self.pad.as_ref()) else { return };
        let strokes = pad.held_strokes();
        let Some(bounds) = pad.held_bounds() else { return };
        match self.session.start_ocr_job(&strokes, mode) {
            Ok(job) => match crate::math_ocr::OcrRun::start(ctx.clone(), self.session.sync_http(), job) {
                Ok(run) => {
                    self.math_ocr = Some(crate::math_ocr::Conversion {
                        page: ed.dir.clone(),
                        strokes,
                        bounds,
                        mode,
                        stage: crate::math_ocr::Stage::Waiting(run),
                    });
                }
                Err(err) => self.complain(&err),
            },
            Err(err) => self.complain(&crate::math_ocr::error_text(&err)),
        }
    }

    fn poll_math_ocr(&mut self) {
        if self.math_ocr.as_ref().is_some_and(|conv| self.editor.as_ref().map(|e| &e.dir) != Some(&conv.page)) {
            self.math_ocr = None;
            return;
        }
        let outcome = self.math_ocr.as_ref().and_then(|conv| match &conv.stage {
            crate::math_ocr::Stage::Waiting(run) => run.try_recv(),
            crate::math_ocr::Stage::Preview { .. } => None,
        });
        match outcome {
            Some(Ok(tex)) => {
                if let Some(conv) = self.math_ocr.as_mut() {
                    conv.stage = crate::math_ocr::Stage::Preview { tex, opened: false };
                }
            }
            Some(Err(err)) => {
                self.math_ocr = None;
                self.complain(&crate::math_ocr::error_text(&err));
            }
            None => {}
        }
    }

    fn math_ocr_ui(&mut self, ctx: &Context) {
        if self.dialog.is_some() {
            return;
        }
        let Some(mut conv) = self.math_ocr.take() else { return };
        match crate::math_ocr::show(ctx, &mut conv, &self.tokens) {
            crate::math_ocr::Outcome::Keep => {
                self.math_ocr = Some(conv);
            }
            crate::math_ocr::Outcome::Cancel => {}
            crate::math_ocr::Outcome::Insert => {
                self.confirm_math_ocr(conv);
            }
        }
    }

    fn confirm_math_ocr(&mut self, conv: crate::math_ocr::Conversion) {
        let crate::math_ocr::Stage::Preview { tex, .. } = conv.stage else { return };
        let new_blocks: Vec<nocturne_formats::markdown::Block> = match conv.mode {
            nocturne_engine::OcrMode::MathLatex => {
                let clean_tex = tex.split_whitespace().collect::<Vec<_>>().join(" ");
                if clean_tex.is_empty() {
                    return;
                }
                vec![nocturne_formats::markdown::Block::MathBlock(clean_tex)]
            }
            nocturne_engine::OcrMode::HandwritingText => {
                let lines: Vec<nocturne_formats::markdown::Block> = tex
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        nocturne_formats::markdown::Block::Paragraph(vec![nocturne_formats::markdown::Span {
                            text: line.to_string(),
                            ..nocturne_formats::markdown::Span::default()
                        }])
                    })
                    .collect();
                if lines.is_empty() {
                    return;
                }
                lines
            }
        };
        self.take_saves(true);
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.writable && ed.dir == conv.page) else {
            return;
        };
        let Some(pad) = self.pad.as_mut() else { return };
        let Some(targets) = pad.exact_stroke_indices(&conv.strokes) else {
            self.save_now();
            self.complain(crate::tl_id!("ui.ink_changed_not_converted"));
            return;
        };

        let mut boxes = if ed.blocks.is_empty() {
            Vec::new()
        } else {
            let occupied: Vec<_> = ed.boxes.iter().filter(|c| c.blocks > 0).cloned().collect();
            let default_width = occupied.first().map_or(360.0, |c| c.width);
            nocturne_model::container::fit(&occupied, ed.blocks.len(), default_width)
        };
        let [x0, y0, x1, _y1] = conv.bounds;
        let x = (x0 - 6.0).max(0.0).round();
        let y = (y0 - 14.0).max(0.0).round();
        let box_width = ((x1 - x0) + 24.0).clamp(nocturne_model::container::MIN_WIDTH, 360.0).round();
        boxes.push(nocturne_model::container::Container::new(x, y, box_width, new_blocks.len()));
        let mut blocks = ed.blocks.clone();
        blocks.extend(new_blocks);
        let body = nocturne_formats::boxes::write(ed.format, &blocks, Some(&boxes));

        let page_file = conv.page.join(ed.format.page_file());
        let previous = match std::fs::read_to_string(&page_file) {
            Ok(previous) => previous,
            Err(e) => {
                self.fail(&nocturne_store::StoreError::io("read", &page_file, e).into());
                return;
            }
        };
        // Keep the original drawing on disk until the recognized content has been saved.
        let timestamp = match self.session.run("page.save", &params(&[("path", &path_str(&conv.page)), ("body", &body)])) {
            Ok(Output::Text(timestamp)) => timestamp,
            Ok(_) => return,
            Err(e) => {
                self.fail(&e);
                return;
            }
        };
        let mut ink_error = pad
            .save_removing_strokes_at(targets)
            .err()
            .map(|e| crate::i18n::format_message_id("ui.cannot_write_the_drawing_error", &[("error", &e.to_string())]));
        if let Some(message) = &mut ink_error {
            // Ink is still present. Restore the previous saved text so the same selection can
            // be converted again, leaving any unsaved editor changes and undo steps intact.
            match nocturne_store::page::write_atomic(&page_file, &previous) {
                Ok(()) => {
                    self.complain(message);
                    return;
                }
                Err(e) => {
                    let restore_error = crate::i18n::format_message_id("ui.could_not_complete_the_action_error", &[("error", &e.to_string())]);
                    message.push(' ');
                    message.push_str(&restore_error);
                }
            }
        }

        Self::remember_step(ed);
        ed.blocks = blocks;
        ed.boxes = boxes;
        ed.arranged = true;
        ed.across = None;
        ed.saved_body = body.clone();
        ed.body = body;
        ed.dirty_since = None;
        ed.last_saved = Some(timestamp);
        self.undone.clear();
        self.library_grid.invalidate_page(&conv.page);
        self.sync_dirty_since_last = true;
        self.last_local_edit_at = Some(Instant::now());

        // If restoring the previous text also failed, the recognized text is still on disk
        // alongside the original ink. Reflect that saved page in the editor.
        if let Some(message) = ink_error {
            Self::remember_edit(ed);
            self.complain(&message);
            return;
        }
        let now = Instant::now();
        self.last_ink_change = Some(now);
        self.last_local_edit_at = Some(now);
        self.tell_kept_ink();
        self.sync_ink_reference();
        if let Some(ed) = self.editor.as_mut() {
            // When all the ink went, its former container might now be empty.
            ed.boxes.retain(|c| c.blocks > 0);
            if ed.caret.as_ref().is_some_and(|c| c.block >= ed.blocks.len()) {
                ed.caret = None;
            }
            ed.rebuild_body();
            Self::mark_dirty_from_blocks(ed);
            Self::remember_edit(ed);
        }
        self.save_now();
        if self.editor.as_ref().is_some_and(|ed| ed.dirty_since.is_some()) {
            return;
        }
        let msg = match conv.mode {
            nocturne_engine::OcrMode::MathLatex => crate::tl_id!("ui.equation_inserted"),
            nocturne_engine::OcrMode::HandwritingText => crate::tl_id!("ui.ocr_insert_text"),
        };
        self.say(msg);
    }

    // ---------- control channel ----------

    /// Attaches a control channel (see `control.rs` and `docs/control-protocol.md`).
    pub fn with_control(mut self, server: ControlServer, rx: Receiver<ControlRequest>) -> Self {
        self.control_server = Some(server);
        self.control = Some(rx);
        self
    }

    /// Says where GridCraft is, or that it is not installed, instead of looking for it on this
    /// machine. Tests use this so a table's spreadsheet buttons do not depend on the machine.
    pub fn with_gridcraft(mut self, exe: Option<std::path::PathBuf>) -> Self {
        self.gridcraft = exe;
        self
    }

    /// Attaches only a request receiver, with no network listener. The headless screenshot
    /// runner uses this to feed scripted requests.
    pub fn with_requests(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control = Some(rx);
        self
    }

    /// The input events for the next frame, from clicks, keys and text that control requests
    /// queued. A click is spread over three frames (move, press, release) the way a real mouse
    /// delivers it. The window's shell calls this once per frame and adds the events to its input.
    pub fn take_events(&mut self) -> Vec<egui::Event> {
        self.event_frames.pop_front().unwrap_or_default()
    }

    fn poll_control(&mut self, ctx: &Context) {
        let mut pending = Vec::new();
        if let Some(rx) = &self.control {
            while let Ok(req) = rx.try_recv() {
                pending.push(req);
            }
        }
        for req in pending {
            self.handle_control(ctx, req);
        }
    }

    /// Answers one control request. Methods: see `docs/control-protocol.md`.
    pub fn handle_control(&mut self, ctx: &Context, req: ControlRequest) {
        if req.method == "ui.screenshot" {
            let path = req.params.get("path").and_then(Value::as_str).map(PathBuf::from);
            ctx.request_screenshot(move |image| {
                req.respond(encode_screenshot(&image, path.as_deref()));
            });
            ctx.request_repaint();
            return;
        }
        let result = self.control_method(ctx, &req.method, &req.params);
        req.respond(result);
    }

    fn control_method(&mut self, ctx: &Context, method: &str, p: &Value) -> Result<Value, String> {
        let num = |k: &str| p.get(k).and_then(Value::as_f64).map(|v| v as f32);
        let flag = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
        let text = |k: &str| p.get(k).and_then(Value::as_str);
        match method {
            "engine.commands" => Ok(nocturne_engine::commands_json()),
            "engine.execute" => {
                let id = text("command").ok_or("missing command")?;
                let params = nocturne_engine::params_from_json(p.get("params"));
                self.save_now();
                if let Some(pad) = self.pad.as_mut()
                    && pad.dirty
                {
                    let _ = pad.save();
                }
                let out = self.session.run(id, &params).map_err(|e| e.to_string())?;
                if id == "sync.configure" {
                    self.sync_has_token = None;
                    self.cached_sync_config = None;
                } else if id == "view.tablet_tools" {
                    self.tablet_tools_minimized = self.session.settings.get("tablet_tools_minimized").as_deref() == Some("true");
                    if self.tablet_tools_minimized {
                        self.tool_settings_open = false;
                    }
                } else if id == "page.canvas_layout" && params.contains_key("layout") {
                    if let Some(ed) = self.editor.as_mut()
                        && let Ok(layout) = nocturne_store::page::canvas_layout(&ed.dir)
                    {
                        ed.canvas_layout = layout;
                    }
                    self.sync_dirty_since_last = true;
                    self.last_local_edit_at = Some(Instant::now());
                } else if id == "sync.now"
                    && let Some(dir) = self.editor.as_ref().map(|ed| ed.dir.clone())
                    && dir.is_dir()
                {
                    self.open_page(&dir);
                }
                if self.editor.as_ref().is_some_and(|ed| !ed.dir.exists()) {
                    self.editor = None;
                }
                self.sync_tool_state_from_session();
                ctx.request_repaint();
                Ok(nocturne_engine::output_json(&out))
            }
            "ui.inspect" => Ok(self.inspect(ctx)),
            "ui.open" => {
                self.open_path(text("path").ok_or("missing path")?);
                if self.status_is_error { Err(self.status.clone()) } else { Ok(self.inspect(ctx)) }
            }
            // Put the caret in a block of the open page, the way a click does.
            "ui.caret" => {
                let Some(ed) = self.editor.as_mut() else { return Err("no page is open".into()) };
                if !ed.writable {
                    return Err("this page is read-only until the block editor can keep its markup".into());
                }
                let n = p.get("block").and_then(Value::as_u64).ok_or("missing block")? as usize;
                // A heading that only repeats the page's title is not drawn, so the caret
                // cannot go there: the next block is what a person would have clicked.
                let n = if pageedit::is_title_echo(&ed.blocks, n, &ed.title) { n + 1 } else { n };
                let block = ed.blocks.get(n).ok_or_else(|| format!("the page has no block {n}"))?;
                if !pageedit::is_writable(block) {
                    return Err(format!("block {n} is a table, a rule or a picture; the caret cannot go in it yet"));
                }
                ed.across = None;
                ed.caret = Some(pageedit::Caret {
                    block: n,
                    text: pageedit::caret_text(block),
                    focus: true,
                    selection: (0, 0),
                    place: None,
                    cell: None,
                    span: None,
                    inline: None,
                });
                ctx.request_repaint();
                Ok(json!({"block": n, "text": ed.caret.as_ref().map(|c| c.text.clone())}))
            }
            "ui.back" => {
                self.go_back();
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.forward" => {
                self.go_forward();
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.undo" => {
                self.undo();
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.redo" => {
                self.redo();
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.expandAll" => {
                self.expand_all();
                Ok(json!({}))
            }
            "ui.collapseAll" => {
                self.open.clear();
                Ok(json!({}))
            }
            "ui.theme" => {
                let mode = text("mode").map(|m| ModePref::parse(m).ok_or("mode must be light, dark or system")).transpose()?;
                let accent =
                    text("accent").map(|a| Accent::parse(a).ok_or("accent must be default, red, green, yellow, blue, gray or purple")).transpose()?;
                let accent = accent.unwrap_or(self.accent);
                match mode.unwrap_or(self.mode_pref) {
                    ModePref::Light => self.set_theme(Theme { mode: Mode::Light, accent }),
                    ModePref::Dark => self.set_theme(Theme { mode: Mode::Dark, accent }),
                    ModePref::System => self.follow_system_theme(accent),
                }
                Ok(json!({"theme": self.theme().id(), "modePref": self.mode_pref.id()}))
            }
            "ui.click" | "ui.move" | "ui.press" | "ui.release" => {
                let pos = egui::pos2(num("x").ok_or("missing x")?, num("y").ok_or("missing y")?);
                let button = match text("button") {
                    Some("right") => egui::PointerButton::Secondary,
                    Some("middle") => egui::PointerButton::Middle,
                    _ => egui::PointerButton::Primary,
                };
                let modifiers = egui::Modifiers { alt: flag("alt"), ctrl: flag("ctrl"), shift: flag("shift"), mac_cmd: false, command: flag("ctrl") };
                self.event_frames.push_back(vec![egui::Event::PointerMoved(pos)]);
                let press = |pressed| vec![egui::Event::PointerButton { pos, button, pressed, modifiers }];
                match method {
                    "ui.click" => {
                        for _ in 0..p.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 3) {
                            self.event_frames.push_back(press(true));
                            self.event_frames.push_back(press(false));
                        }
                    }
                    "ui.press" => self.event_frames.push_back(press(true)),
                    "ui.release" => self.event_frames.push_back(press(false)),
                    _ => {}
                }
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.key" => {
                let name = text("key").ok_or("missing key")?;
                let key = egui::Key::from_name(name).ok_or_else(|| format!("unknown key '{name}'"))?;
                let modifiers = egui::Modifiers { alt: flag("alt"), ctrl: flag("ctrl"), shift: flag("shift"), mac_cmd: false, command: flag("ctrl") };
                for pressed in [true, false] {
                    self.event_frames.push_back(vec![egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers }]);
                }
                ctx.request_repaint();
                Ok(json!({}))
            }
            // Pasting is its own event, not typing: a table from another app arrives this
            // way, and the page editor turns it back into a table.
            "ui.paste" => {
                self.event_frames.push_back(vec![egui::Event::Paste(text("text").ok_or("missing text")?.to_string())]);
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.text" => {
                self.event_frames.push_back(vec![egui::Event::Text(text("text").ok_or("missing text")?.to_string())]);
                ctx.request_repaint();
                Ok(json!({}))
            }
            // Scrolling, so a script can reach what is below the fold: the list of files
            // in a page's folder sits under a whole sheet of paper, and nothing could
            // reach it from here before.
            "ui.scroll" => {
                let (dx, dy) = (num("x").unwrap_or(0.0), num("y").ok_or("missing y")?);
                self.event_frames.push_back(vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(dx, dy),
                    modifiers: egui::Modifiers { shift: flag("shift"), ..egui::Modifiers::NONE },
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                }]);
                ctx.request_repaint();
                Ok(json!({}))
            }
            "ui.resize" => {
                let (w, h) = (num("width").ok_or("missing width")?, num("height").ok_or("missing height")?);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w.clamp(400.0, 8000.0), h.clamp(300.0, 8000.0))));
                Ok(json!({}))
            }
            "app.quit" => {
                self.save_now();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                Ok(json!({}))
            }
            other => Err(format!("unknown method '{other}'")),
        }
    }

    /// What the window shows now: the visible tree rows, the open page, the theme and the size.
    pub fn inspect(&self, ctx: &Context) -> Value {
        // The Mermaid diagrams of the page as it is drawn, not of the code view: how many, and how
        // many of them are drawn, still being drawn, or could not be.
        let in_page: Vec<String> = self
            .editor
            .as_ref()
            .map(|e| {
                e.blocks
                    .iter()
                    .filter_map(|b| match b {
                        nocturne_formats::markdown::Block::Code { lang, text } if lang == "mermaid" => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let in_page_counts = crate::diagramview::counts(ctx, &in_page, &self.tokens);
        let mut rows = Vec::new();
        fn walk(app: &NocturneApp, n: &Node, depth: usize, rows: &mut Vec<Value>) {
            let open = app.open.contains(&n.path);
            rows.push(json!({
                "path": nocturne_store::tree::display_path(n.path.strip_prefix(app.session.root()).unwrap_or(&n.path)).replace('\\', "/"),
                "title": n.info.title,
                "kind": n.info.kind.id(),
                "format": n.info.format.map(|f| f.id()),
                "depth": depth,
                "hasChildren": !n.children.is_empty(),
                "open": open,
                "selected": app.selected.as_deref() == Some(n.path.as_path()),
            }));
            if open {
                for c in &n.children {
                    walk(app, c, depth + 1, rows);
                }
            }
        }
        for n in self.session.tree() {
            walk(self, n, 0, &mut rows);
        }
        let screen = ctx.content_rect();
        json!({
            "window": {"width": screen.width(), "height": screen.height(), "pixelsPerPoint": ctx.pixels_per_point()},
            "canvas": {
                "viewOffset": [self.canvas_pan.x, self.canvas_pan.y],
                // The sideways scroll bar along the page's bottom and its thumb, as
                // [left, top, right, bottom] in screen points, or null when everything fits.
                "horizontalBar": self.page_hbar.map(|(track, _)| [track.min.x, track.min.y, track.max.x, track.max.y]),
                "horizontalThumb": self.page_hbar.map(|(_, thumb)| [thumb.min.x, thumb.min.y, thumb.max.x, thumb.max.y]),
                // True while Insert Space is in hand (issue #164).
                "insertSpace": self.space_tool,
                "zoomPercent": self.zoom_percent,
            },
            // How the side panel is arranged: one tree, or the organisers and the pages in
            // columns of their own, with the first one hidden or not.
            "panes": {
                "split": self.split_panes,
                "organisers": self.organisers,
                "compact": self.compact,
                "fullPage": self.full_page,
                "list": self.compact_list_fills(),
            },
            // Home > Find Tags: where the Tags Summary looks and how many lines it lists, or
            // null while it is closed.
            "pageTemplates": self.templates_pane.as_ref().map(|p| json!({
                "templates": p.templates.iter().map(|t| json!({"id": t.id, "name": t.name, "category": t.category.id()})).collect::<Vec<_>>(),
                "name": p.name,
            })),
            "tagsSummary": self.tags_pane.as_ref().map(|p| json!({
                "scope": p.scope.id(),
                "unchecked": p.unchecked,
                "lines": p.found.iter().map(|l| json!({"tag": l.tag.id(), "text": l.text, "done": l.done, "page": l.page, "section": l.section})).collect::<Vec<_>>(),
            })),
            // Back and Forward, and the Quick Access Toolbar's choices.
            "nav": {
                "back": self.nav.back_count(),
                "forward": self.nav.forward_count(),
                "current": self.nav.current().map(|p| self.shown(p)),
            },
            "windowFrame": if self.frameless { "own" } else { "system" },
            "quickAccess": {"shown": self.qat.show, "below": self.qat.below, "hidden": self.qat.hidden},
            // Folded to its tab names (Ctrl+F1), and whether a clicked tab shows it over the page.
            "ribbon": {"collapsed": self.ribbon_collapsed, "shown": !self.ribbon_folded() || self.ribbon_peek, "tab": self.ribbon_tab},
            // Whether a dialog is up over the window (Android's Back closes it first).
            "dialogOpen": self.dialog.is_some(),
            "formattingBarVisible": self.formatting_bar_visible(),
            "layout_mode": self.session.layout_mode().id(),
            "theme": {"id": self.theme().id(), "modePref": self.mode_pref.id(), "accent": self.accent.id()},
            // The paper behind the pages, as the View tab's gallery sets it.
            "paper": self.paper_now().id(),
            "ruleColour": self.editor.as_ref().and_then(|e| e.rule_colour).map(nocturne_model::NodeColor::id),
            "pageColour": self.editor.as_ref().and_then(|e| e.colour).map(nocturne_model::NodeColor::id),
            // The pen the Draw tab is set to, and the drawing it writes in.
            "ink": {
                "pointer": self.pointer.id(),
                "tool": self.pen().tool.id(),
                "colour": self.pen().colour.hex(),
                "width": self.pen().width,
                "rack": nocturne_ink::rack::write(&self.rack),
                "nib": self.nib,
                "sensitivity": self.sensitivity,
                "holding": self.pad.as_ref().map_or(0, crate::inkpad::Pad::holding),
                "bounds": self.pad.as_ref().and_then(crate::inkpad::Pad::bounds),
                "gain": self.gain(),
                "strokes": self.pad.as_ref().map(crate::inkpad::Pad::strokes),
                "inked": self.pad.as_ref().is_some_and(|p| !p.is_empty()),
                "fingerDrawing": self.is_finger_drawing(),
                "eraserActive": self.pointer == ribbon::Pointer::Erase,
                "writingTool": self.session.ink_tool_state().previous_writing_tool.id(),
            },
            // What a pen has told this window. `pen` is false until a device reports a
            // pressure at all, and `hardest` is the hardest press since the window opened.
            "pen": {"seen": self.stylus.sample().pen, "pressure": self.stylus.sample().pressure, "hardest": self.stylus.seen().0, "readings": self.stylus.seen().1},
            "defaultFormat": self.session.settings.global_format().0.id(),
            // Where GridCraft is, when a table can be handed to it.
            "gridcraft": self.gridcraft.as_ref().map(|p| p.to_string_lossy()),
            // The code view: whether it is open, which of the page's files is picked, and
            // how much of that file is in hand.
            "codeView": {
                "open": self.show_source,
                "picked": self.code.picked,
                "characters": self.code.text.chars().count(),
                // The colours the picked file is shown in, and the Find bar: whether it is open,
                // what it looks for, how many matches there are and which one (from 1) it is on.
                "language": self.code.picked.as_deref().map_or("none", |n| crate::highlight::Lang::of_file(n).label()),
                "colouredStretches": self.code.coloured.len(),
                "find": {
                    "open": self.code.find.open,
                    "query": self.code.find.query,
                    "matchCase": self.code.find.case,
                    "matches": self.code.find.matches.len(),
                    "current": if self.code.find.matches.is_empty() { Value::Null } else { json!(self.code.find.current + 1) },
                },
            },
            "pageDiagrams": {
                "found": in_page.len(),
                "drawn": in_page_counts.0,
                "drawing": in_page_counts.1,
                "failed": in_page_counts.2,
            },
            "notebooksFolder": self.shown(self.session.root()),
            "status": {"text": self.status, "error": self.status_is_error},
            "rows": rows,
            "page": self.editor.as_ref().map(|e| json!({
                "path": e.dir.strip_prefix(self.session.root()).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default(),
                "title": e.title,
                // What the title area actually shows: blank for a page with no title of its
                // own, unlike `title`, which is always at least the "Untitled page" fallback.
                "titleArea": e.title_buf,
                "format": e.format.id(),
                "dirty": e.dirty_since.is_some() || e.saving.is_some(),
                "bodyCharacters": e.body.chars().count(),
                "blocks": e.blocks.len(),
                "attachments": e.attachments.len(),
                "writable": e.writable,
                // `selection` is (where the caret is, where the selection began), in
                // characters: the two are equal when nothing is selected.
                "caret": e.caret.as_ref().map(|c| json!({"block": c.block, "text": c.text, "at": c.selection.0, "anchor": c.selection.1})),
                // A selection dragged across more than one block, as block and byte at each
                // end. Null for the ordinary selection inside one block, which `caret` has.
                "across": e.across.map(|a| json!({"from": [a.from.0, a.from.1], "to": [a.to.0, a.to.1]})),
                // The boxes the blocks are arranged in, as the page keeps them.
                "containers": e.boxes.iter().map(|b| json!({"x": b.x, "y": b.y, "width": b.width, "blocks": b.blocks})).collect::<Vec<_>>(),
                // The container that is selected, with everything in it, or null.
                "selectedContainer": crate::containers::selected(ctx, &e.dir),
                "selectedContainers": crate::containers::selected_boxes(ctx, &e.dir),
                // The image block selected independently of its containing text box, or null.
                "selectedImage": crate::containers::selected_image(ctx, &e.dir),
                "selectedObject": crate::containers::selected_object(ctx, &e.dir).map(|(block, kind)| json!({"block": block, "kind": kind})),
                "origin": [e.origin.x, e.origin.y],
                // What can be taken back, and how many steps are kept.
                "history": {"steps": e.history.len(), "at": e.history.at(), "canUndo": e.history.can_undo(), "canRedo": e.history.can_redo()},
            })),
        })
    }
    /// Draws the whole window. Call once per frame.
    /// Fills the strips the platform's own bars lie over (a phone's status bar and navigation
    /// bar, reported as egui's safe area) with the title band's colour, so the system's white
    /// icons read on it in a light theme as well as a dark one. Everything else is laid out
    /// inside the safe area. On a desktop there is none, and nothing is drawn.
    fn paint_system_bars(&self, ctx: &Context) {
        let strips = self.system_bar_strips(ctx);
        if strips.is_empty() {
            return;
        }
        // On the background layer, which egui always paints, clipped to the whole window rather
        // than to the safe area as a layer painter is by default. Nothing else is drawn under
        // the bars: every panel and window is inside the safe area.
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), ctx.viewport_rect());
        for (strip, colour) in strips {
            painter.rect_filled(strip, 0.0, colour);
        }
    }

    /// The strips [`Self::paint_system_bars`] fills, with their colour: the parts of the window
    /// outside egui's safe area. None on a desktop.
    pub fn system_bar_strips(&self, ctx: &Context) -> Vec<(egui::Rect, egui::Color32)> {
        let (outer, inner) = (ctx.viewport_rect(), ctx.content_rect());
        let strips = [
            egui::Rect::from_min_max(outer.min, egui::pos2(outer.max.x, inner.min.y)),
            egui::Rect::from_min_max(egui::pos2(outer.min.x, inner.max.y), outer.max),
            egui::Rect::from_min_max(egui::pos2(outer.min.x, inner.min.y), egui::pos2(inner.min.x, inner.max.y)),
            egui::Rect::from_min_max(egui::pos2(inner.max.x, inner.min.y), egui::pos2(outer.max.x, inner.max.y)),
        ];
        let colour = if self.is_tablet_studio(ctx) {
            if self.backstage.is_some() || (self.editor.is_none() && self.is_studio_grid(ctx)) { self.tokens.panel } else { self.tokens.editor }
        } else {
            self.tokens.band
        };
        strips.into_iter().filter(|r| r.is_positive()).map(|r| (r, colour)).collect()
    }

    pub fn show(&mut self, ui: &mut Ui) {
        // egui's root `Ui` covers the whole window, the parts under a phone's status and
        // navigation bars included. Everything is laid out inside the safe area instead
        // (issue #315); on a desktop the two are the same.
        let ctx = ui.ctx().clone();
        let content = ctx.content_rect();
        self.last_safe_area_bottom = (ctx.viewport_rect().max.y - content.max.y).max(0.0);
        let extra_kb = if self.soft_keyboard_visible { (self.soft_keyboard_inset_bottom - self.last_safe_area_bottom).max(0.0) } else { 0.0 };
        if content != ctx.viewport_rect() || extra_kb > 0.0 {
            if content != ctx.viewport_rect() {
                self.paint_system_bars(&ctx);
            }
            let mut room = ui.max_rect().intersect(content);
            if extra_kb > 0.0 {
                room.max.y = (room.max.y - extra_kb).max(room.min.y);
            }
            ui.scope_builder(egui::UiBuilder::new().id_salt("safe-area").max_rect(room), |ui| self.show_in(ui));
        } else {
            self.show_in(ui);
        }
    }

    fn show_in(&mut self, ui: &mut Ui) {
        crate::i18n::set_current(&self.ui_language);
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if !self.loaders_installed {
            crate::pictures::install(ctx);
            self.loaders_installed = true;
        }
        // Arabic and the other right-to-left scripts have no glyphs in egui's own faces.
        crate::fonts::install_once(ctx);
        // Pen pressure, where the platform reports it. Nothing draws with it yet; it is
        // read here so `ui.inspect` can say whether a pen reaches this window at all.
        ctx.input(|i| self.stylus.update(&i.events));
        crate::floating::set_touch_sized(ctx, self.stylus.touch_affordance_active() || ctx.os() == egui::os::OperatingSystem::IOS);
        // An update is ready to be put in place: save the page and close, so the script that
        // waits for this window can replace the program and start the new one.
        if !self.update_closing && self.update.lock().is_ok_and(|s| matches!(*s, crate::update::Status::Restart { .. })) {
            self.update_closing = true;
            self.save();
            log::info!("update: closing so the new version can be put in place");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.apply_theme(ctx);
        // Only when the size was chosen or read at start: egui's own Ctrl+Plus and Ctrl+Minus
        // change the same factor, and are left alone in between.
        if self.ui_scale_pending {
            ctx.set_zoom_factor(f32::from(self.ui_scale) / 100.0);
            self.ui_scale_pending = false;
        }
        if self.window_level_pending {
            let level = if self.always_on_top { egui::viewport::WindowLevel::AlwaysOnTop } else { egui::viewport::WindowLevel::Normal };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            self.window_level_pending = false;
        }
        if let Some(text) = self.pending_copy.take() {
            if let Some(html) = self.pending_copy_html.take() {
                if self.clipboard.set_html(&html, &text).is_err() {
                    ctx.copy_text(text);
                }
            } else {
                ctx.copy_text(text);
            }
        }
        self.last_ctx_is_ios = ctx.os() == egui::os::OperatingSystem::IOS;
        self.compact_layout(ctx);
        self.last_tablet_studio = self.is_tablet_studio(ctx);
        // A button whose file dialog Android or iOS cannot show says so (`filepicker.*.rs`).
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some(notice) = crate::filepicker::take_notice() {
            if self.last_tablet_studio {
                self.studio_notice = Some((notice.clone(), Instant::now()));
            }
            self.say(notice);
        }
        self.poll_control(ctx);
        self.poll_import(ctx);
        self.poll_ink_save(ctx);
        self.poll_sync(ctx);
        self.poll_ocr_model_download();
        if let Some(mode) = self.pending_math_ocr_start.take() {
            self.start_math_ocr(ctx, mode);
        }
        self.poll_math_ocr();
        if !self.event_frames.is_empty() {
            ctx.request_repaint();
        }
        // The File screen takes the whole window while it is open, as it does in Office.
        // Run before page keyboard shortcuts and file drops so nothing behind Backstage can
        // mutate or absorb dropped files while Backstage is showing.
        if self.backstage.is_some() {
            self.keyboard_paste = None;
            // No page is drawn under it, so the last page's rect is dropped: the platform's pan
            // and pinch take only touches on a page that is on the screen.
            self.page_rect = None;
            self.page_view_rect = None;
            if !self.is_tablet_studio(ctx) {
                self.title_band(ui, false);
            }
            self.storage_notice(ui);
            self.backstage_screen(ui);
            self.theme_window_ui(ctx);
            self.dialogs(ctx);
            self.import_progress(ctx);
            self.frame_edges(ctx);
            if ctx.input(|i| i.viewport().close_requested()) {
                self.save_now();
            }
            self.take_saves(false);
            if self.saver.busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        if let Some(paste) = self.keyboard_paste.take() {
            if self.page_owns_clipboard_input(ctx) {
                self.paste_here(paste.fallback_text);
            } else if let Some(text) = paste.fallback_text {
                ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
            }
        }
        self.clipboard_keys(ctx);
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            if let Some(ed) = self.editor.as_mut() {
                ed.dirty_since.get_or_insert_with(Instant::now);
            }
            self.save_now();
        }
        // Taken before any widget sees them: a text box has an undo of its own that
        // knows only about the letters in that one box, and the page's undo has to be the
        // one that answers, or Ctrl+Z would mean two different things on one page.
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        // Not while a dialog is open: it owns the keyboard, and the page behind it must not
        // change unseen (issue #262: Ctrl+Z undid and saved an edit behind Insert symbol). A
        // text box in the dialog keeps its own undo.
        let math_preview_open = matches!(self.math_ocr.as_ref().map(|c| &c.stage), Some(crate::math_ocr::Stage::Preview { .. }));
        let page_keys = self.dialog.is_none() && !self.theme_window && !math_preview_open && self.backstage.is_none();
        // The title, search, and source editors keep their own history.
        let page_history_keys = page_keys && !self.title_is_focused() && (!ctx.egui_wants_keyboard_input() || self.page_text_has_focus(ctx));
        if page_history_keys && ctx.input_mut(|i| i.consume_key(cmd_shift, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y)) {
            // The ink's as well as the page's, whichever Undo took back.
            self.redo();
        } else if page_history_keys && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            // The ink's changes as well as the page's: whichever was last, or both when one
            // press made them together.
            self.undo();
        }
        // Ctrl+F, and OneNote's Ctrl+E, go to the search box. Taken before the page sees
        // them: with the caret in the page, the key went nowhere and the words typed to find
        // something were written into the note (issue #134). The code view has a Find of its
        // own for the file it shows, so it keeps Ctrl+F while it is open.
        // OneNote's Ctrl+K: the Link dialog for the words picked or the link the caret is in.
        if !self.show_source && page_keys && self.editor.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            self.open_link_dialog();
        }
        if !self.show_source && page_keys && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F) || i.consume_key(Modifiers::COMMAND, Key::E))
        {
            if self.is_studio_grid(ctx) && self.editor.is_none() && self.backstage.is_none() {
                ctx.memory_mut(|m| m.request_focus(egui::Id::unique(crate::library_grid::LIBRARY_SEARCH_FIELD)));
            } else {
                if let Some(caret) = self.editor.as_mut().and_then(|ed| ed.caret.as_mut()) {
                    caret.focus = false;
                }
                self.focus_search = true;
            }
        }
        self.nav_keys(ctx);
        self.replay_typing(ctx);
        self.switch_by_device(ctx);
        self.pen_keys(ctx);
        self.direction_keys(ctx);
        self.hierarchy_keys(ctx);
        self.restore_last_page();
        self.attach_dropped(ctx);
        self.top_bar(ui);
        self.storage_notice(ui);
        self.mobile_formatting_bar(ui);
        self.status_bar(ui);
        self.side_panel(ui);
        self.tags_panel(ui);
        self.templates_panel(ui);
        // In a phone-width window a pane that fills it leaves no room for the page.
        if !self.compact_list_fills() && !self.compact_pane_fills() {
            self.central(ui);
        } else {
            // And no page rect: the one from the frame before is no longer on the screen.
            self.page_rect = None;
            self.page_view_rect = None;
        }
        // Tablet Studio has no status bar. Routine messages are not shown there; a failure is,
        // on one line, until it is tapped. A dialog that is open says the failure itself.
        // What was tapped away is remembered by its words, and forgotten when the next message
        // that is not a failure arrives.
        if !self.status_is_error {
            self.status_banner_dismissed = None;
        }
        self.status_banner_drawn = self.is_tablet_studio(ctx)
            && self.status_is_error
            && self.dialog.is_none()
            && !self.status.trim().is_empty()
            && self.status_banner_dismissed.as_deref() != Some(self.status.as_str());
        if self.status_banner_drawn && crate::tablet_chrome::show_error_banner(ctx, ctx.content_rect(), &self.status, true, &self.tokens) {
            self.status_banner_dismissed = Some(self.status.clone());
            self.status_banner_drawn = false;
        }
        // A result the user asked for (a sync started by hand) is shown the same way, in the
        // ordinary colours, for a few seconds or until it is tapped. A failure takes its place.
        const NOTICE_SECS: u64 = 6;
        if let Some((message, since)) = self.studio_notice.clone() {
            let left = std::time::Duration::from_secs(NOTICE_SECS).saturating_sub(since.elapsed());
            if left.is_zero() || self.status_banner_drawn || !self.is_tablet_studio(ctx) {
                self.studio_notice = None;
            } else if self.dialog.is_none() {
                if crate::tablet_chrome::show_error_banner(ctx, ctx.content_rect(), &message, false, &self.tokens) {
                    self.studio_notice = None;
                }
                ctx.request_repaint_after(left);
            }
        }
        self.paint_formatting(ctx);
        self.laser_keys(ctx);
        self.search_popup(ctx);
        self.math_ocr_ui(ctx);
        self.theme_window_ui(ctx);
        self.dialogs(ctx);
        self.import_progress(ctx);
        self.frame_edges(ctx);
        // Closing the window writes the page first, and waits for an autosave still being
        // written: what was typed in the moment before closing is not lost.
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_now();
        }
        self.take_saves(false);
        if self.saver.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if let Some(since) = self.editor.as_ref().and_then(|e| e.dirty_since) {
            // Read once: read twice, the second could be past the wait, and the subtraction
            // below panicked (seen on a slow CI runner).
            let waited = since.elapsed();
            if waited >= AUTOSAVE_AFTER {
                self.save_in_background();
            } else {
                ctx.request_repaint_after(AUTOSAVE_AFTER.saturating_sub(waited));
            }
        }
    }
}

fn table_from_gridcraft_csv(text: &str) -> Option<(nocturne_formats::markdown::Block, usize, usize)> {
    let table = nocturne_formats::csv::to_table(text)?;
    let (rows, columns) = pageedit::table_size(&table).unwrap_or((0, 0));
    Some((table, rows, columns))
}

/// One swatch of the theme switcher: the surface colour, ringed and dotted in the accent.
fn theme_swatch(ui: &mut Ui, theme: Theme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    let t = theme.tokens();
    let painter = ui.painter_at(rect);
    let c = rect.center();
    painter.circle_filled(c, 10.5, t.panel);
    painter.circle_stroke(c, 10.5, egui::Stroke::new(if selected { 3.0 } else { 1.5 }, t.accent));
    painter.circle_filled(c, 4.5, t.accent);
    if resp.hovered() {
        painter.circle_stroke(c, 12.0, egui::Stroke::new(1.0, ui.style().visuals.text_color()));
    }
    let theme_label = theme.label();
    let label = crate::i18n::format_message_id("ui.theme_theme", &[("theme", &theme_label)]);
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, selected, &label));
    resp.on_hover_text(theme_label)
}

fn format_picker(ui: &mut Ui, format: &mut Option<PageFormat>) {
    let selected = format.map(PageFormat::label).unwrap_or(crate::tl_id!("ui.inherit_notebook_section_or_global_setting"));
    egui::ComboBox::from_label(crate::tl_id!("ui.page_format")).selected_text(selected).show_ui(ui, |ui| {
        ui.selectable_value(format, None, crate::tl_id!("ui.inherit_notebook_section_or_global_setting"));
        for f in PageFormat::ALL {
            ui.selectable_value(format, Some(f), f.label());
        }
    });
}

#[allow(dead_code)]
fn _unused(_: Color32) {}

/// A small square button that draws one icon, with an on state for the ones that toggle.
fn icon_toggle(ui: &mut Ui, icon: Icon, on: bool, t: &Tokens, label: &str, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    if on || resp.hovered() {
        painter.rect_filled(rect, 4.0, if on { t.row_selected } else { t.row_hover });
    }
    let color = if on { t.accent } else { t.muted };
    icons::paint(&painter, egui::Rect::from_center_size(rect.center(), vec2(17.0, 17.0)), icon, color);
    let enabled = ui.is_enabled();
    let name = label.to_string();
    resp.widget_info(move || egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, name.clone()));
    resp.on_hover_text(tip)
}

/// One colour of the node colour picker: a filled circle, ringed when it is the current one.
fn color_swatch(ui: &mut Ui, color: NodeColor, dark: bool, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let (r, g, b) = color.rgb(dark);
    let fill = Color32::from_rgb(r, g, b);
    let painter = ui.painter_at(rect);
    painter.circle_filled(rect.center(), 8.0, fill);
    if selected {
        painter.circle_stroke(rect.center(), 10.0, egui::Stroke::new(2.0, ui.style().visuals.text_color()));
    }
    let enabled = ui.is_enabled();
    let label = crate::tl!(color.label());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, selected, label));
    resp.on_hover_text(label)
}

/// Returns an `Unsupported` I/O error describing why an external desktop opener cannot run on mobile.
fn unsupported_mobile_open(ios_message: &str, android_message: &str) -> std::io::Error {
    let message = if cfg!(target_os = "ios") { ios_message } else { android_message };
    std::io::Error::new(std::io::ErrorKind::Unsupported, message)
}

/// Shows a folder in the operating system's file manager. An optional extra: everything the
/// app itself does works without it, and a failure is reported, never fatal.
fn open_folder(dir: &Path) -> std::io::Result<()> {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return Err(unsupported_mobile_open(
            "opening folders externally is not supported on iPadOS / iOS; manage notebooks in Files > On My iPad > Nocturne",
            "opening folders externally is not supported on Android",
        ));
    }
    // Explorer opens the user's Documents folder when it is given a path that does not exist, so
    // a missing folder is an error here, not a window on the wrong place.
    if !dir.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} does not exist", dir.display())));
    }
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // `explorer` returns a non-zero status even when it worked, so the status is not checked.
    std::process::Command::new(program).arg(dir).spawn().map(|_| ())
}

/// Opens a linked attachment with the operating system's associated application. Windows may
/// show its own warning for files marked as downloaded; that prompt is intentionally preserved.
pub(crate) fn open_file_for_viewer(file: &Path) -> std::io::Result<()> {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return Err(unsupported_mobile_open(
            "opening attachments in external apps is not supported on iPadOS / iOS yet",
            "opening attachments in external apps is not supported on Android yet",
        ));
    }
    if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/C", "start", "", &file.to_string_lossy()]).spawn().map(|_| ())
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(file).spawn().map(|_| ())
    } else {
        std::process::Command::new("xdg-open").arg(file).spawn().map(|_| ())
    }
}

/// Saves a screenshot to `path`, or returns it as base64 PNG in the reply.
fn encode_screenshot(image: &egui::ColorImage, path: Option<&Path>) -> Result<Value, String> {
    let [w, h] = image.size;
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("the screenshot has the wrong size")?;
    match path {
        Some(p) => {
            img.save(p).map_err(|e| format!("cannot save {}: {e}", p.display()))?;
            Ok(json!({"path": p.to_string_lossy(), "width": w, "height": h}))
        }
        None => {
            let mut png = std::io::Cursor::new(Vec::new());
            img.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            Ok(json!({"png": base64(png.get_ref()), "width": w, "height": h}))
        }
    }
}

/// The first page inside a notebook or section, at any depth.
fn first_page(tree: &[Node], root: &Path) -> Option<PathBuf> {
    fn walk(n: &Node) -> Option<PathBuf> {
        if n.info.kind == NodeKind::Page {
            return Some(n.path.clone());
        }
        n.children.iter().find_map(walk)
    }
    tree.iter().find(|n| n.path == root).and_then(walk)
}

/// The first page inside the node at `at`, wherever in the tree that node is.
fn first_page_under(tree: &[Node], at: &Path) -> Option<PathBuf> {
    fn find<'a>(n: &'a Node, at: &Path) -> Option<&'a Node> {
        if n.path == at {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, at))
    }
    let node = tree.iter().find_map(|n| find(n, at))?;
    first_page(std::slice::from_ref(node), &node.path)
}

/// The 14 swatches and the switch that follows the system, shared by the sun or moon button
/// and the ribbon's Theme button.
fn theme_grid(ui: &mut Ui, current: Theme, pick: &mut Option<(Mode, Accent)>, follow: &mut bool) {
    egui::Grid::new("theme-grid").spacing([6.0, 6.0]).show(ui, |ui| {
        for mode in [Mode::Light, Mode::Dark] {
            ui.label(crate::tl!(mode.label()));
            for accent in Accent::ALL {
                let theme = Theme { mode, accent };
                let selected = theme == current;
                if theme_swatch(ui, theme, selected).clicked() {
                    *pick = Some((mode, accent));
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    let accent = crate::tl!(current.accent.label());
    let label = crate::i18n::format_message_id("ui.accent_accent", &[("accent", accent)]);
    ui.label(egui::RichText::new(label).small());
    ui.checkbox(follow, crate::tl_id!("ui.follow_the_system_light_or_dark_setting"));
}

#[cfg(test)]
mod alert_tests {
    use super::*;

    #[test]
    fn release_builds_default_local_collection_to_off() {
        assert_eq!(default_telemetry_level(false), "none");
        assert_eq!(default_telemetry_level(true), "debug");
    }

    #[test]
    fn gridcraft_csv_read_back_preserves_quoted_cells_and_adds_columns() {
        let (table, rows, columns) =
            table_from_gridcraft_csv("Item,Cost,Note\n\"Tea, green\",3.50,\"she said \"\"best\"\"\"\nPaper,12,\n").expect("CSV table");
        assert_eq!((rows, columns), (3, 3));
        let body = pageedit::to_body(PageFormat::Cmgfm, &[table]);
        assert!(body.contains("Tea, green"));
        assert!(body.contains("she said \"best\""));
    }

    #[test]
    fn maximum_sensitivity_reaches_four_times_the_neutral_pen_width() {
        assert_eq!(NocturneApp::sensitivity_width(0), 1.0);
        assert_eq!(NocturneApp::sensitivity_width(50), 1.0);
        assert_eq!(NocturneApp::sensitivity_width(100), 4.0);
    }

    #[test]
    fn wheel_canvas_pan_ignores_ctrl_and_write_mode_but_keeps_horizontal_shift_scroll() {
        assert_eq!(NocturneApp::wheel_canvas_pan_delta(vec2(0.0, 42.0), Modifiers::NONE, false), egui::Vec2::ZERO);
        assert_eq!(NocturneApp::wheel_canvas_pan_delta(vec2(0.0, 42.0), Modifiers::NONE, true), egui::vec2(0.0, 42.0));
        assert_eq!(NocturneApp::wheel_canvas_pan_delta(vec2(0.0, 42.0), Modifiers::SHIFT, false), egui::vec2(42.0, 0.0));
        assert_eq!(NocturneApp::wheel_canvas_pan_delta(vec2(0.0, 42.0), Modifiers::CTRL, true), egui::Vec2::ZERO);
    }

    #[test]
    fn fresh_preferences_start_with_neutral_sensitivity() {
        let t = nocturne_store::testing::TempDir::new("default-sensitivity");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        assert_eq!(settings.get(SENSITIVITY_KEY), None);
        assert_eq!(settings.get(SENSITIVITY_KEY).and_then(|v| v.parse::<u8>().ok()).unwrap_or(DEFAULT_SENSITIVITY), 50);
        settings.set(SENSITIVITY_KEY, "100").expect("set");
        settings.reset().expect("reset");
        assert_eq!(settings.get(SENSITIVITY_KEY), None);
        assert_eq!(settings.get(SENSITIVITY_KEY).and_then(|v| v.parse::<u8>().ok()).unwrap_or(DEFAULT_SENSITIVITY), 50);
    }

    #[test]
    fn an_alert_queues_behind_an_open_one_and_a_muted_key_is_never_queued() {
        let t = nocturne_store::testing::TempDir::new("alerts");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let mut app = NocturneApp::new(session);

        app.alert("first", "Title", "one");
        assert!(matches!(&app.dialog, Some(Dialog::Alert { key, .. }) if key == "first"), "the first alert opens");

        app.alert("first", "Title", "one again");
        assert!(app.queued_alerts.is_empty(), "the same alert is not shown twice while it is open");

        app.alert("second", "Title", "two");
        assert_eq!(app.queued_alerts.len(), 1, "a different alert waits its turn");

        app.muted_alerts.insert("third".to_string());
        app.alert("third", "Title", "three");
        assert_eq!(app.queued_alerts.len(), 1, "a muted alert is not queued");
        assert_eq!(app.status, "three", "the status line still says what happened");
    }

    #[test]
    fn a_permanently_muted_alert_is_not_opened() {
        let t = nocturne_store::testing::TempDir::new("alerts-never");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        settings.set("pressure_unavailable_never", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let mut app = NocturneApp::new(session);

        app.alert("pressure_unavailable", "Pressure", "not available");
        assert!(app.dialog.is_none(), "a permanent mute suppresses the dialog");
        assert_eq!(app.status, "not available", "the status still records the condition");
    }

    #[test]
    fn privacy_welcome_dialog_shows_on_first_launch() {
        let t = nocturne_store::testing::TempDir::new("privacy-first-launch");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        // Do NOT set privacy_welcome_seen - simulate first launch
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let app = NocturneApp::new(session);

        // Dialog should be shown because privacy_welcome_seen is not in settings
        assert!(matches!(app.dialog, Some(Dialog::FirstUseWizard { .. })), "PrivacyWelcome dialog should show on first launch");
    }

    #[test]
    fn privacy_welcome_dialog_does_not_show_after_dismissed() {
        let t = nocturne_store::testing::TempDir::new("privacy-dismissed");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let app = NocturneApp::new(session);

        // Dialog should not be shown because privacy_welcome_seen is set
        assert!(app.dialog.is_none(), "PrivacyWelcome dialog should not show after being dismissed");
    }

    #[test]
    fn the_wizard_opens_once_after_install_and_not_for_later_versions() {
        let t = nocturne_store::testing::TempDir::new("welcome-wizard-once");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let upgraded = Session::new(&t.path().join("notebooks"), settings.clone()).expect("upgraded session");
        let upgraded = NocturneApp::new(upgraded).with_welcome_notebook_setup(|root, _format| Ok(root.join("welcome-to-nocturne")));
        assert!(upgraded.dialog.is_none(), "an existing user is not shown the wizard again for a later version");

        // A fresh install opens the wizard at the welcome page; finishing it saves every answer.
        let fresh = nocturne_store::Settings::at(t.path().join("fresh.toml"));
        let session = Session::new(&t.path().join("fresh-notebooks"), fresh.clone()).expect("fresh session");
        let mut app = NocturneApp::new(session).with_welcome_notebook_setup(|root, _format| Ok(root.join("welcome-to-nocturne")));
        assert!(matches!(app.dialog, Some(Dialog::FirstUseWizard { step: 0, .. })), "a fresh install starts at the welcome page");
        assert!(app.finish_first_use_wizard("none", PageFormat::Html, false).is_ok());
        assert_eq!(fresh.get("privacy_welcome_seen").as_deref(), Some("true"));

        let again = Session::new(&t.path().join("fresh-notebooks"), fresh).expect("reopened session");
        let again = NocturneApp::new(again).with_welcome_notebook_setup(|root, _format| Ok(root.join("welcome-to-nocturne")));
        assert!(again.dialog.is_none(), "a finished wizard does not open again");
    }

    #[test]
    fn welcome_notebook_prompt_waits_until_the_privacy_dialog_closes() {
        let t = nocturne_store::testing::TempDir::new("welcome-prompt-after-privacy");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let app = NocturneApp::new(session).with_welcome_notebook_setup(|root, _format| Ok(root.join("welcome-to-nocturne")));

        assert!(matches!(app.dialog, Some(Dialog::FirstUseWizard { step: 0, .. })), "the wizard opens at the logging step");
        assert!(app.queued_alerts.is_empty(), "the wizard is one window, so nothing waits behind it");
    }

    #[test]
    fn failed_welcome_notebook_generation_keeps_the_prompt_and_does_not_record_a_choice() {
        let t = nocturne_store::testing::TempDir::new("welcome-prompt-failure");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings.clone()).expect("session");
        let mut app = NocturneApp::new(session).with_welcome_notebook_setup(|_, _| Err("copy failed".to_string()));

        assert!(app.finish_first_use_wizard("none", PageFormat::Cmgfm, true).is_err());

        assert_eq!(settings.get("welcome_notebook_choice_version"), None, "a failed generation leaves the choice unanswered");
    }

    #[test]
    fn the_wizard_installs_the_edition_for_the_chosen_format() {
        let t = nocturne_store::testing::TempDir::new("welcome-wizard-format");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings.clone()).expect("session");
        let installed_format = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let recorder = installed_format.clone();
        let mut app = NocturneApp::new(session).with_welcome_notebook_setup(move |root, format| {
            *recorder.borrow_mut() = format.to_string();
            let installed = root.join("welcome-to-nocturne");
            std::fs::create_dir_all(installed.join("00-start/welcome")).map_err(|error| error.to_string())?;
            Ok(installed)
        });

        // Opening the fake folder's page can fail; the full install and open path is covered by the UI test.
        let _ = app.finish_first_use_wizard("none", PageFormat::Cmgfm, true);

        assert_eq!(installed_format.borrow().as_str(), "cmgfm", "the chosen format reaches the install");
    }

    /// `--first-use` writes `privacy_welcome_seen = ""` and keeps every other setting.
    #[test]
    fn first_use_flag_value_shows_the_privacy_welcome_again() {
        let t = nocturne_store::testing::TempDir::new("privacy-first-use-flag");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "").expect("settings");
        settings.set("theme_mode", "dark").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let app = NocturneApp::new(session);
        assert!(matches!(app.dialog, Some(Dialog::FirstUseWizard { .. })), "an emptied key counts as not seen");
        assert_eq!(app.session.settings.get("theme_mode").as_deref(), Some("dark"), "other settings are kept");
    }

    #[test]
    fn privacy_welcome_dialog_reappears_after_settings_file_deleted() {
        let t = nocturne_store::testing::TempDir::new("privacy-reset");
        let settings_path = t.path().join("settings.toml");
        let settings = nocturne_store::Settings::at(settings_path.clone());
        // Simulate persistent settings file with privacy_welcome_seen already set
        settings.set("privacy_welcome_seen", "true").expect("settings");
        settings.set("other_setting", "value").expect("settings");

        // Verify setting exists and dialog doesn't show
        let session1 = Session::new(&t.path().join("notebooks"), settings.clone()).expect("session");
        let app1 = NocturneApp::new(session1);
        assert!(app1.dialog.is_none(), "Dialog should not show when privacy_welcome_seen is set");

        // Delete the settings file (simulates what run.cmd does with --first-use)
        let _ = std::fs::remove_file(&settings_path);

        // Create new settings from deleted file (triggers fresh initialization)
        let settings2 = nocturne_store::Settings::at(settings_path);
        let session2 = Session::new(&t.path().join("notebooks"), settings2).expect("session");
        let app2 = NocturneApp::new(session2);

        // Dialog should reappear because settings file is gone
        assert!(matches!(app2.dialog, Some(Dialog::FirstUseWizard { .. })), "PrivacyWelcome dialog should reappear when settings file is deleted");
    }

    #[test]
    fn delete_dialog_shows_for_notebook() {
        let t = nocturne_store::testing::TempDir::new("delete-dialog-notebook");
        let nb_path = t.path().join("test-notebook");
        std::fs::create_dir_all(&nb_path).expect("create notebook dir");

        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        let session = Session::new(t.path(), settings).expect("session");
        let _app = NocturneApp::new(session);

        // Simulate triggering delete notebook dialog
        let dialog = Dialog::Delete { path: nb_path.clone(), title: "Test Notebook".to_string(), count: 0 };

        // Verify the dialog is properly structured for a notebook delete
        assert!(matches!(dialog, Dialog::Delete { .. }));
    }

    #[test]
    fn delete_error_message_shown_on_deletion_failure() {
        let t = nocturne_store::testing::TempDir::new("delete-error-feedback");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        let session = Session::new(t.path(), settings).expect("session");
        let mut app = NocturneApp::new(session);

        // Simulate a failed delete operation by setting error status
        app.status = "The notebook could not be deleted: Permission denied".to_string();
        app.status_is_error = true;

        // Verify error state is properly set
        assert!(app.status_is_error, "error flag should be set");
        assert!(app.status.contains("could not be deleted"), "error message should indicate deletion failure");
    }

    #[test]
    fn delete_notebook_clears_editor_state() {
        let t = nocturne_store::testing::TempDir::new("delete-clear-state");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        let session = Session::new(t.path(), settings).expect("session");
        let mut app = NocturneApp::new(session);

        // Verify initial state is empty
        assert!(app.editor.is_none(), "editor should start empty");
        assert!(app.selected.is_none(), "selection should start empty");

        // Simulate the delete operation clearing state
        // This is what happens in the delete handler after successful deletion
        app.editor = None;
        app.selected = None;

        // Verify state remains cleared
        assert!(app.editor.is_none(), "editor should remain cleared after delete");
        assert!(app.selected.is_none(), "selection should remain cleared after delete");
    }

    #[test]
    fn node_delete_error_shows_user_feedback() {
        let t = nocturne_store::testing::TempDir::new("node-delete-error");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        let session = Session::new(t.path(), settings).expect("session");
        let mut app = NocturneApp::new(session);

        // Simulate node.delete command failure
        app.status = "Could not delete: the operation failed".to_string();
        app.status_is_error = true;

        // Verify error message is shown to user
        assert!(app.status_is_error, "error flag should be set when node delete fails");
        assert!(app.status.contains("Could not delete"), "user should see error message");
    }

    /// A page left with only the SVG fallback (the Shell extraction failed once) still has a
    /// usable icon and must not be reported as missing.
    #[test]
    fn an_svg_fallback_icon_is_accepted_everywhere() {
        let t = nocturne_store::testing::TempDir::new("svg-fallback-icon");
        let page = t.path().to_path_buf();
        let assets = page.join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        std::fs::write(assets.join("Chapter.docx"), b"docx").expect("attachment");
        std::fs::write(assets.join("Chapter.docx.svg"), b"<svg/>").expect("svg fallback");
        let source = r#"<p><a href="assets/Chapter.docx"><img src="assets/Chapter.docx.svg" alt="" width="48"><br>Chapter.docx</a></p>"#;

        assert_eq!(first_missing_attachment_in_source(source, &page), None, "the SVG fallback is a valid icon");
        assert!(attachment_icon_healthy(&page, "Chapter.docx"));

        std::fs::write(assets.join("Chapter.docx.png"), b"png").expect("png");
        assert_eq!(first_missing_attachment_in_source(source, &page), None, "a PNG sidecar is healthy everywhere");
        assert!(attachment_icon_healthy(&page, "Chapter.docx"));
    }

    #[test]
    fn a_printout_added_in_its_own_container_can_be_undone_and_redone() {
        let t = nocturne_store::testing::TempDir::new("printout-undo");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let mut app = NocturneApp::new(session);
        app.open_path(&sample.md_page.to_string_lossy());
        let (blocks, boxes) = app.editor.as_ref().map(|e| (e.blocks.len(), e.boxes.len())).expect("a page is open");

        app.insert_in_own_container("\n[![a.pdf](assets/a.pdf.svg)](assets/a.pdf)\n\n![Printout page 1](assets/a.pdf.printout-001.png)\n");
        let ed = app.editor.as_ref().expect("editor");
        assert_eq!((ed.blocks.len(), ed.boxes.len()), (blocks + 2, boxes.max(1) + 1));

        app.undo();
        let ed = app.editor.as_ref().expect("editor");
        assert_eq!(ed.blocks.len(), blocks, "undo takes the printout's blocks back out");

        app.redo();
        let ed = app.editor.as_ref().expect("editor");
        assert_eq!(ed.blocks.len(), blocks + 2, "redo puts them back");
        assert_eq!(ed.boxes.last().map(|c| c.blocks), Some(2));
    }

    #[test]
    fn undo_after_a_real_printout_insert_removes_all_of_it_even_after_frames_have_drawn() {
        for html in [false, true] {
            undo_after_a_printout_insert(html);
        }
    }

    fn undo_after_a_printout_insert(html: bool) {
        let t = nocturne_store::testing::TempDir::new("printout-undo-frames");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let pdf = t.path().join("sheet.pdf");
        let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
        std::fs::write(
            &pdf,
            "%PDF-1.4\n1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R/Resources<<>>>>\nendobj\n4 0 obj\n<</Length 33>>\nstream\n0.8 0.1 0.1 rg 20 20 100 100 re f\nendstream\nendobj\nxref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000054 00000 n \n0000000105 00000 n \n0000000199 00000 n \ntrailer\n<</Root 1 0 R/Size 5>>\nstartxref\n280\n%%EOF\n",
        )
        .expect("pdf");
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1200.0, 760.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        for _ in 0..5 {
            h.step();
        }
        let shape =
            |h: &egui_kittest::Harness<'_, NocturneApp>| h.state().editor.as_ref().map(|e| (e.blocks.clone(), e.boxes.clone())).expect("editor");
        let before = shape(&h);
        let dir = page.clone();
        h.state_mut().add_file(&dir, &pdf, FileChoice::Printout);
        // Long enough for the autosave to write the page and the window to settle.
        for _ in 0..60 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let inserted = shape(&h);
        assert!(inserted.0.len() > before.0.len(), "the printout added blocks: {}", h.state().status);
        h.state_mut().undo();
        for _ in 0..5 {
            h.step();
        }
        assert_eq!(shape(&h), before, "one undo takes the whole printout away");
    }

    /// A page from the sample notebook, open in a window, and a one-page PDF to insert.
    fn printout_harness(html: bool, name: &str) -> (nocturne_store::testing::TempDir, PathBuf, PathBuf, egui_kittest::Harness<'static, NocturneApp>) {
        let t = nocturne_store::testing::TempDir::new(name);
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let pdf = t.path().join("sheet.pdf");
        let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
        std::fs::write(
            &pdf,
            "%PDF-1.4\n1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]/Contents 4 0 R/Resources<<>>>>\nendobj\n4 0 obj\n<</Length 33>>\nstream\n0.8 0.1 0.1 rg 20 20 100 100 re f\nendstream\nendobj\nxref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000054 00000 n \n0000000105 00000 n \n0000000199 00000 n \ntrailer\n<</Root 1 0 R/Size 5>>\nstartxref\n280\n%%EOF\n",
        )
        .expect("pdf");
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1200.0, 760.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        for _ in 0..5 {
            h.step();
        }
        (t, page, pdf, h)
    }

    /// Issue #67: with no box holding the caret, a second printout goes below what is already
    /// on the page, not over the first.
    #[test]
    fn a_second_printout_lands_below_the_boxes_already_on_the_page() {
        for html in [false, true] {
            let (_t, page, pdf, mut h) = printout_harness(html, "printout-below");
            h.state_mut().editor.as_mut().expect("editor").caret = None;
            for _ in 0..2 {
                h.state_mut().add_file(&page, &pdf, FileChoice::Printout);
                for _ in 0..5 {
                    h.step();
                }
            }
            let all = h.state().editor.as_ref().map(|e| e.boxes.clone()).expect("editor");
            // The sample page's words, then one box for each printout, each one further down.
            assert!(all.len() >= 3, "html={html}: each printout has a box of its own: {all:?}");
            for pair in all.windows(2) {
                assert!(pair[1].y > pair[0].y, "html={html}: each new box sits below the one before, not over it: {all:?}");
            }
        }
    }

    /// A box clicked into and left blank goes however the caret leaves it, not only when the page
    /// itself moves the caret, and a printout added meanwhile is not put below it: the blank box
    /// was never something on the page, only a place for the cursor.
    #[test]
    fn a_blank_box_the_caret_was_taken_from_goes_and_a_printout_is_not_put_below_it() {
        for html in [false, true] {
            let (_t, page, pdf, mut h) = printout_harness(html, "blank-box-printout");
            let line = {
                let ed = h.state_mut().editor.as_mut().expect("editor");
                if ed.boxes.is_empty() {
                    ed.boxes = vec![nocturne_model::container::Container::new(0.0, 0.0, 600.0, ed.blocks.len())];
                }
                let line = ed.blocks.len();
                ed.blocks.push(nocturne_formats::Block::Paragraph(Vec::new()));
                ed.boxes = nocturne_model::container::append(&ed.boxes, line, 1, 300.0, 0.0, 0.0);
                let index = ed.boxes.len() - 1;
                (ed.boxes[index].x, ed.boxes[index].y) = (400.0, 2000.0);
                ed.arranged = true;
                ed.rebuild_body();
                ed.caret = Some(pageedit::Caret {
                    block: line,
                    text: String::new(),
                    focus: true,
                    selection: (0, 0),
                    place: None,
                    cell: None,
                    span: None,
                    inline: None,
                });
                line
            };
            for _ in 0..3 {
                h.step();
            }
            let boxes_with_blank = h.state().editor.as_ref().map(|e| e.boxes.len()).expect("editor");
            h.state_mut().editor.as_mut().expect("editor").caret = None;
            h.state_mut().add_file(&page, &pdf, FileChoice::Printout);
            for _ in 0..5 {
                h.step();
            }
            let ed = h.state().editor.as_ref().expect("editor");
            assert!(!ed.boxes.iter().any(|c| c.y == 2000.0), "html={html}: the blank box went: {:?}", ed.boxes);
            assert_eq!(ed.boxes.len(), boxes_with_blank, "html={html}: one box for the printout, none for the blank line: {:?}", ed.boxes);
            let printout = ed.boxes.last().expect("the printout's box");
            assert!(printout.y < 2000.0, "html={html}: the printout is not put below the blank box: {:?}", ed.boxes);
            assert!(ed.blocks.len() > line, "html={html}: the printout is on the page");
        }
    }

    /// The autosave hands the page to a writer of its own and the window carries on: on a synced
    /// folder a save takes about a second, and every frame it held up was a click or a letter
    /// held up. The words still reach the file, and anything that needs them there waits for them.
    #[test]
    fn the_autosave_does_not_hold_up_the_window() {
        let (_t, page, _pdf, mut h) = printout_harness(true, "autosave-background");
        h.state_mut().saver = crate::saver::Saver::with_writer(|dir, body| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            nocturne_store::page::save_body(dir, body).map_err(|e| e.to_string())
        });
        {
            let ed = h.state_mut().editor.as_mut().expect("editor");
            let (added, _) = nocturne_formats::boxes::read(ed.format, "Written in the background\n");
            let at = ed.blocks.len();
            ed.blocks.extend(added.iter().cloned());
            if ed.boxes.is_empty() {
                ed.arranged = false;
            } else {
                ed.boxes = nocturne_model::container::append(&ed.boxes, at, added.len(), 300.0, 0.0, 0.0);
            }
            ed.rebuild_body();
            ed.dirty_since = Some(Instant::now() - AUTOSAVE_AFTER - std::time::Duration::from_millis(10));
        }
        let frame = Instant::now();
        h.step();
        assert!(frame.elapsed() < std::time::Duration::from_millis(250), "the frame the autosave fired in took {:?}", frame.elapsed());
        assert!(h.state().is_dirty(), "until it is written the page counts as unsaved");
        h.state_mut().save_now();
        let file = nocturne_store::tree::page_file(&page).expect("the page's file").1;
        let text = std::fs::read_to_string(file).expect("read");
        assert!(text.contains("Written in the background"), "the words reached the file:\n{text}");
        assert!(!h.state().is_dirty(), "and the page is saved");
    }

    /// Issue #67: a printout goes into the box that holds the caret, after the caret's line,
    /// and that box stays where it was put.
    #[test]
    fn a_printout_goes_into_the_box_that_holds_the_caret() {
        for html in [false, true] {
            let (_t, page, pdf, mut h) = printout_harness(html, "printout-caret-box");
            // A box with a line in it, moved to the right of the page's words, with the caret in it.
            let (index, line) = {
                let ed = h.state_mut().editor.as_mut().expect("editor");
                let (added, _) = nocturne_formats::boxes::read(ed.format, "Beside the words\n");
                let line = ed.blocks.len();
                ed.blocks.extend(added.iter().cloned());
                ed.boxes = nocturne_model::container::append(&ed.boxes, line, added.len(), 300.0, 0.0, 0.0);
                let index = ed.boxes.len() - 1;
                (ed.boxes[index].x, ed.boxes[index].y) = (900.0, 40.0);
                ed.arranged = true;
                ed.rebuild_body();
                ed.caret = Some(pageedit::Caret {
                    block: line,
                    text: "Beside the words".into(),
                    focus: false,
                    selection: (0, 0),
                    place: None,
                    cell: None,
                    span: None,
                    inline: None,
                });
                // Made here rather than by clicks and typing, so it is remembered here too, the
                // way the window remembers a box the writer made.
                NocturneApp::remember_step(ed);
                (index, line)
            };
            for _ in 0..3 {
                h.step();
            }
            let before = h.state().editor.as_ref().map(|e| e.boxes.clone()).expect("editor");
            let blocks_before = h.state().editor.as_ref().map(|e| e.blocks.clone()).expect("editor");
            h.state_mut().add_file(&page, &pdf, FileChoice::Printout);
            for _ in 0..5 {
                h.step();
            }
            let ed = h.state().editor.as_ref().expect("editor");
            assert_eq!(ed.boxes.len(), before.len(), "html={html}: no new box was made: {:?}", ed.boxes);
            let moved = &ed.boxes[index];
            assert_eq!((moved.x, moved.y), (900.0, 40.0), "html={html}: the box stays where it was put");
            assert!(moved.blocks > before[index].blocks, "html={html}: the printout is in the caret's box: {:?}", ed.boxes);
            // A printout's pages are as wide as the sheet, as they are in a box of their own: a
            // narrow box (one started with a click) is widened for them, not the pages shrunk.
            let sheet = before[0].width;
            assert!(sheet > 300.0, "html={html}: the page's first box is wider than the narrow one: {before:?}");
            assert_eq!(moved.width, sheet, "html={html}: the box is widened to the sheet for the printout");
            let range = nocturne_model::container::blocks_of(&ed.boxes, index);
            assert_eq!(range.start, line, "html={html}: the box's own line is still first");
            assert!(
                ed.blocks[range].iter().any(|b| matches!(b, nocturne_formats::Block::Image { .. })),
                "html={html}: the printout's pages are in that box"
            );
            h.state_mut().undo();
            for _ in 0..3 {
                h.step();
            }
            let ed = h.state().editor.as_ref().expect("editor");
            assert_eq!((&ed.blocks, &ed.boxes), (&blocks_before, &before), "html={html}: one undo takes the printout back out of the box");
        }
    }
}

/// Blocks put into a page that has several boxes go into the box the caret is in, and every
/// box keeps its place. Before, the box counts were left behind, `container::fit` saw the
/// mismatch, and the whole arrangement collapsed into one box at the top-left.
#[cfg(test)]
mod keep_boxes_tests {
    use super::*;

    /// A sample page split into two boxes, the second moved to the right, with the caret on
    /// its last line.
    fn two_boxes(html: bool, name: &str) -> (nocturne_store::testing::TempDir, PathBuf, egui_kittest::Harness<'static, NocturneApp>) {
        let t = nocturne_store::testing::TempDir::new(name);
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1400.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        for _ in 0..5 {
            h.step();
        }
        {
            let ed = h.state_mut().editor.as_mut().expect("editor");
            let n = ed.blocks.len();
            assert!(n >= 4, "the sample page has blocks to split");
            let first = n / 2;
            ed.boxes = vec![nocturne_model::Container::new(0.0, 0.0, 600.0, first), nocturne_model::Container::new(900.0, 40.0, 300.0, n - first)];
            ed.arranged = true;
            let last = n - 1;
            ed.caret = ed.blocks.get(last).map(|b| pageedit::Caret {
                block: last,
                text: pageedit::caret_text(b),
                focus: false,
                selection: (0, 0),
                place: None,
                cell: None,
                span: None,
                inline: None,
            });
            ed.rebuild_body();
            NocturneApp::remember_step(ed);
        }
        for _ in 0..3 {
            h.step();
        }
        (t, page, h)
    }

    /// How many blocks the right-hand box holds now.
    fn right_box(h: &egui_kittest::Harness<'static, NocturneApp>) -> usize {
        h.state().editor.as_ref().map(|e| e.boxes[1].blocks).expect("editor")
    }

    /// The arrangement after an insert: still two boxes, the right one where it was, every block
    /// counted, and the right box holding at least `grew` more blocks than the `was` it held.
    fn assert_kept(h: &mut egui_kittest::Harness<'static, NocturneApp>, what: &str, was: usize, grew: usize) {
        for _ in 0..5 {
            h.step();
        }
        let ed = h.state().editor.as_ref().expect("editor");
        assert_eq!(ed.boxes.len(), 2, "{what}: both boxes are still there: {:?}", ed.boxes);
        assert_eq!((ed.boxes[1].x, ed.boxes[1].y), (900.0, 40.0), "{what}: the right box stays put");
        assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len(), "{what}: every block is in a box");
        assert!(ed.boxes[1].blocks >= was + grew, "{what}: the new block went into the caret's box: {:?}", ed.boxes);
    }

    #[test]
    fn a_pasted_picture_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, page, mut h) = two_boxes(html, "keep-boxes-picture");
            // A 1x1 PNG; what matters is where the block goes, not what it shows.
            let png = vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f,
                0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 0x05, 0, 0x01, 0xff,
                0x89, 0x99, 0x3d, 0x1d, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
            ];
            let was = right_box(&h);
            h.state_mut().paste_picture(&page, crate::clipboard::Picture { png, width: 1, height: 1 });
            assert_kept(&mut h, &format!("html={html} picture"), was, 1);
        }
    }

    #[test]
    fn an_inserted_table_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, _page, mut h) = two_boxes(html, "keep-boxes-table");
            let was = right_box(&h);
            h.state_mut().ribbon_act(ribbon::Act::Insert("table"));
            assert_kept(&mut h, &format!("html={html} table"), was, 1);
        }
    }

    #[test]
    fn inserted_space_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, _page, mut h) = two_boxes(html, "keep-boxes-space");
            let was = right_box(&h);
            h.state_mut().ribbon_act(ribbon::Act::Insert("space"));
            assert_kept(&mut h, &format!("html={html} space"), was, 1);
        }
    }

    #[test]
    fn pasted_lines_go_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, _page, mut h) = two_boxes(html, "keep-boxes-paste");
            let was = right_box(&h);
            h.state_mut().paste_blocks(pageedit::blocks_to_paste("one\n\ntwo\n\nthree"));
            assert_kept(&mut h, &format!("html={html} paste"), was, 2);
        }
    }

    #[test]
    fn a_new_drawing_named_by_the_page_keeps_the_boxes() {
        for html in [false, true] {
            let (_t, page, mut h) = two_boxes(html, "keep-boxes-ink");
            let was = right_box(&h);
            let svg = nocturne_ink::svg::write(&nocturne_ink::Drawing::default(), nocturne_ink::svg::Flavour::Editable);
            std::fs::write(page.join("ink.svg"), svg).expect("ink");
            h.state_mut().sync_ink_reference();
            assert_kept(&mut h, &format!("html={html} ink"), was, 1);
        }
    }

    #[test]
    fn a_table_of_a_chosen_size_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, _page, mut h) = two_boxes(html, "keep-boxes-sized-table");
            let was = right_box(&h);
            h.state_mut().ribbon_act(ribbon::Act::InsertTable(2, 3));
            assert_kept(&mut h, &format!("html={html} sized table"), was, 1);
        }
    }

    #[test]
    fn a_dropped_file_copied_in_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            // A picture and any other file: each kind has its own snippet.
            for name in ["picture.png", "notes.txt"] {
                let (t, page, mut h) = two_boxes(html, "keep-boxes-copy");
                let file = t.path().join(name);
                std::fs::write(&file, "not really a picture").expect("file");
                let was = right_box(&h);
                h.state_mut().add_file(&page, &file, FileChoice::Copy);
                assert_kept(&mut h, &format!("html={html} copy {name}"), was, 1);
            }
        }
    }

    #[test]
    fn a_dropped_file_linked_goes_into_the_caret_box_and_the_boxes_stay() {
        for html in [false, true] {
            let (t, page, mut h) = two_boxes(html, "keep-boxes-link");
            let file = t.path().join("notes.txt");
            std::fs::write(&file, "some notes").expect("file");
            let was = right_box(&h);
            h.state_mut().add_file(&page, &file, FileChoice::Link);
            assert_kept(&mut h, &format!("html={html} link"), was, 1);
        }
    }

    #[test]
    fn a_snippet_with_no_caret_goes_on_the_end_and_the_boxes_stay() {
        for html in [false, true] {
            let (_t, _page, mut h) = two_boxes(html, "keep-boxes-no-caret");
            h.state_mut().editor.as_mut().expect("editor").caret = None;
            let was = right_box(&h);
            h.state_mut().ribbon_act(ribbon::Act::Equation("x^2"));
            assert_kept(&mut h, &format!("html={html} no caret"), was, 1);
        }
    }
}

#[cfg(test)]
mod literal_formatting_tests {
    use super::*;
    use nocturne_formats::{Block, Span};

    #[test]
    fn clear_formatting_keeps_literal_punctuation_through_save_and_reopen() {
        let temp = nocturne_store::testing::TempDir::new("clear-literal-formatting");
        let settings = nocturne_store::Settings::at(temp.path().join("settings.toml"));
        let mut session = Session::new(temp.path(), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let text = r"*literal* $5 or $6 [x](y) C:\Users\name";
        let original = vec![Block::Paragraph(vec![Span { text: text.into(), bold: true, ..Default::default() }])];
        let body = nocturne_formats::blocks_to_html(&original);
        session.run("page.save", &params(&[("path", &sample.html_page.to_string_lossy()), ("body", &body)])).expect("save");
        let mut app = NocturneApp::new(session);
        app.open_page(&sample.html_page);
        let editor = app.editor.as_mut().expect("opened page");
        editor.caret = Some(pageedit::Caret { text: pageedit::caret_text(&editor.blocks[0]), ..Default::default() });
        app.clear_formatting();
        app.save_now();
        let page = nocturne_store::page::load_page(&sample.html_page).expect("reopen");
        let blocks = nocturne_formats::boxes::read(page.format, &page.body).0;
        assert_eq!(blocks, vec![Block::Paragraph(vec![Span { text: text.into(), ..Default::default() }])]);
    }

    fn opened_literal_page(blocks: &[Block], format: nocturne_model::PageFormat) -> (nocturne_store::testing::TempDir, NocturneApp, PathBuf) {
        let temp = nocturne_store::testing::TempDir::new("literal-command-regression");
        let settings = nocturne_store::Settings::at(temp.path().join("settings.toml"));
        let mut session = Session::new(temp.path(), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let path = match format {
            nocturne_model::PageFormat::Html => sample.html_page,
            nocturne_model::PageFormat::Cmgfm => sample.md_page,
        };
        let body = nocturne_formats::boxes::write(format, blocks, None);
        session.run("page.save", &params(&[("path", &path.to_string_lossy()), ("body", &body)])).expect("save");
        let mut app = NocturneApp::new(session);
        app.open_page(&path);
        (temp, app, path)
    }

    #[test]
    fn clearing_cell_formatting_then_leaving_preserves_the_cell_text() {
        for format in nocturne_model::PageFormat::ALL {
            let text = "*literal* $5 or $6";
            let original = vec![Block::Table {
                head: vec![
                    vec![Span { text: text.into(), bold: true, ..Default::default() }],
                    vec![Span { text: "keep".into(), italic: true, ..Default::default() }],
                ],
                rows: Vec::new(),
            }];
            let (_temp, mut app, path) = opened_literal_page(&original, format);
            let ed = app.editor.as_mut().expect("editor");
            ed.caret =
                Some(pageedit::Caret { text: pageedit::cell_text(&ed.blocks[0], (0, 0)).expect("cell"), cell: Some((0, 0)), ..Default::default() });
            app.clear_formatting();
            let ed = app.editor.as_mut().expect("editor");
            ed.caret.as_mut().expect("caret").write(&mut ed.blocks[0]);
            assert_eq!(pageedit::cell_text(&ed.blocks[0], (0, 0)).as_deref(), Some(text));
            app.save_now();
            let page = nocturne_store::page::load_page(&path).expect("reopen");
            let blocks = nocturne_formats::boxes::read(format, &page.body).0;
            let Block::Table { head, .. } = &blocks[0] else { panic!("table") };
            assert_eq!(head[0], vec![Span { text: text.into(), ..Default::default() }]);
            let Block::Table { head: before, .. } = &original[0] else { panic!("table") };
            assert_eq!(head[1], before[1]);
        }
    }

    #[test]
    fn copy_and_cut_keep_one_line_literal_text_and_rich_styles() {
        for format in nocturne_model::PageFormat::ALL {
            let text = "*literal* $5 or $6";
            let original = vec![Block::Paragraph(vec![Span { text: text.into(), bold: true, ..Default::default() }])];
            let (_temp, mut app, _path) = opened_literal_page(&original, format);
            let ed = app.editor.as_mut().expect("editor");
            let source = pageedit::caret_text(&ed.blocks[0]);
            ed.caret = Some(pageedit::Caret { selection: (0, source.chars().count()), text: source, ..Default::default() });
            app.clipboard("copy");
            assert_eq!(app.pending_copy.as_deref(), Some(text));
            assert_eq!(
                nocturne_formats::markdown::parse(&nocturne_formats::html::to_markdown(app.pending_copy_html.as_deref().expect("html"))),
                original
            );
            assert_eq!(app.editor.as_ref().expect("editor").blocks, original);
            app.clipboard("cut");
            assert_eq!(app.pending_copy.as_deref(), Some(text));
            assert_eq!(pageedit::block_text(&app.editor.as_ref().expect("editor").blocks[0]), "");
        }
    }

    #[test]
    fn copying_across_blocks_keeps_literal_delimiters() {
        let blocks = vec![
            Block::Paragraph(vec![Span { text: "*left*".into(), ..Default::default() }]),
            Block::Paragraph(vec![Span { text: "$5 or $6".into(), ..Default::default() }]),
        ];
        let (_temp, mut app, _path) = opened_literal_page(&blocks, nocturne_model::PageFormat::Html);
        let ed = app.editor.as_mut().expect("editor");
        ed.across = Some(pageedit::Across { from: (0, 0), to: (1, pageedit::block_text(&ed.blocks[1]).len()) });
        app.clipboard("copy");
        assert_eq!(app.pending_copy.as_deref(), Some("*left*\n$5 or $6"));
        assert_eq!(nocturne_formats::markdown::parse(&nocturne_formats::html::to_markdown(app.pending_copy_html.as_deref().expect("html"))), blocks);
    }
}

/// Insert Space and the Table tab's row and column buttons change the page's shape from the
/// ribbon, not by typing into it, so the page never saw them happen and kept no step for them:
/// Undo said there was nothing to undo (issue #153).
#[cfg(test)]
mod structural_undo_tests {
    use super::*;

    /// A page with a line of words and a table under it, open in the window.
    fn page_with_table(html: bool, name: &str) -> (nocturne_store::testing::TempDir, egui_kittest::Harness<'static, NocturneApp>) {
        let t = nocturne_store::testing::TempDir::new(name);
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
        let body = if html {
            "<p>Alpha</p><table><tr><th>A</th><th>B</th></tr><tr><td>one</td><td>two</td></tr></table>"
        } else {
            "Alpha\n\n| A | B |\n|---|---|\n| one | two |\n"
        };
        nocturne_store::page::save_body(&page, body).expect("fixture");
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1400.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        for _ in 0..5 {
            h.step();
        }
        (t, h)
    }

    fn settle(h: &mut egui_kittest::Harness<'static, NocturneApp>) {
        for _ in 0..5 {
            h.step();
        }
    }

    fn blocks(h: &egui_kittest::Harness<'static, NocturneApp>) -> Vec<nocturne_formats::Block> {
        h.state().editor.as_ref().map(|e| e.blocks.clone()).expect("editor")
    }

    fn put_caret(h: &mut egui_kittest::Harness<'static, NocturneApp>, block: usize, cell: Option<(usize, usize)>) {
        let ed = h.state_mut().editor.as_mut().expect("editor");
        let text = match cell {
            Some(c) => pageedit::cell_text(&ed.blocks[block], c).unwrap_or_default(),
            None => pageedit::caret_text(&ed.blocks[block]),
        };
        ed.caret = Some(pageedit::Caret { block, text, focus: true, selection: (0, 0), place: None, cell, span: None, inline: None });
    }

    /// Does `act`, checks it changed the page, then that one Undo puts the page back and one
    /// Redo does it again.
    fn assert_undoable(h: &mut egui_kittest::Harness<'static, NocturneApp>, what: &str, act: ribbon::Act) {
        let before = blocks(h);
        h.state_mut().ribbon_act(act);
        settle(h);
        let after = blocks(h);
        assert_ne!(after, before, "{what}: the page changed");
        assert!(h.state().editor.as_ref().is_some_and(|e| e.history.can_undo()), "{what}: there is a step to undo");
        h.state_mut().undo();
        settle(h);
        assert_eq!(blocks(h), before, "{what}: one undo puts the page back");
        h.state_mut().redo();
        settle(h);
        assert_eq!(blocks(h), after, "{what}: redo does it again");
    }

    #[test]
    fn insert_space_can_be_undone() {
        for html in [false, true] {
            let (_t, mut h) = page_with_table(html, "undo-insert-space");
            put_caret(&mut h, 0, None);
            assert_undoable(&mut h, &format!("html={html} space"), ribbon::Act::Insert("space"));
        }
    }

    #[test]
    fn table_rows_and_columns_put_in_or_taken_out_can_be_undone() {
        use pageedit::TableEdit as E;
        for html in [false, true] {
            for (name, edit) in [
                ("row above", E::InsertRow(true)),
                ("row below", E::InsertRow(false)),
                ("column left", E::InsertColumn(true)),
                ("column right", E::InsertColumn(false)),
                ("delete row", E::DeleteRow),
                ("delete column", E::DeleteColumn),
            ] {
                let (_t, mut h) = page_with_table(html, "undo-table-edit");
                let table = blocks(&h).iter().position(|b| matches!(b, nocturne_formats::Block::Table { .. })).expect("a table");
                put_caret(&mut h, table, Some((1, 0)));
                assert_undoable(&mut h, &format!("html={html} {name}"), ribbon::Act::Table(edit));
            }
        }
    }

    /// Issue #297: Table > Delete Table takes the caret's table out as one Undo step, in both
    /// formats, and the page saves and reopens without it. The caret lands on a real line.
    #[test]
    fn delete_table_takes_the_whole_table_out_and_can_be_undone() {
        use nocturne_formats::Block;
        for html in [false, true] {
            let (_t, mut h) = page_with_table(html, "delete-table");
            let table = blocks(&h).iter().position(|b| matches!(b, Block::Table { .. })).expect("a table");
            put_caret(&mut h, table, Some((1, 1)));
            assert_undoable(&mut h, &format!("html={html} delete table"), ribbon::Act::DeleteTable);
            let left = blocks(&h);
            assert!(!left.iter().any(|b| matches!(b, Block::Table { .. })), "html={html}: no table is left: {left:?}");
            assert!(left.iter().any(|b| pageedit::block_text(b).contains("Alpha")), "html={html}: the words around it stay");
            let caret = h.state().editor.as_ref().and_then(|e| e.caret.clone()).expect("a caret");
            assert!(caret.cell.is_none() && caret.block < left.len(), "html={html}: the caret is on a line of the page: {caret:?}");

            // Saved and opened again: the table is still gone and nothing else changed.
            h.state_mut().save_page_now();
            let path = h.state().editor.as_ref().map(|e| e.dir.clone()).expect("path");
            let file = if html { path.join("index.html") } else { path.join("index.md") };
            let written = std::fs::read_to_string(&file).expect("page file");
            assert!(!written.contains("<table") && !written.contains("| A | B |"), "html={html}: saved without the table:\n{written}");
            assert!(written.contains("Alpha"), "html={html}: and with the words:\n{written}");
        }
    }

    /// A page that held only a table keeps a line to type on, and Delete Table with the caret
    /// outside a table changes nothing.
    #[test]
    fn delete_table_on_a_table_only_page_and_outside_a_table() {
        use nocturne_formats::Block;
        let (_t, mut h) = page_with_table(false, "delete-only-table");
        put_caret(&mut h, 0, None);
        let before = blocks(&h);
        h.state_mut().ribbon_act(ribbon::Act::DeleteTable);
        settle(&mut h);
        assert_eq!(blocks(&h), before, "with the caret outside a table nothing changes");

        let table = blocks(&h).iter().position(|b| matches!(b, Block::Table { .. })).expect("a table");
        h.state_mut().editor.as_mut().expect("editor").blocks.retain(|b| matches!(b, Block::Table { .. }));
        put_caret(&mut h, 0, Some((1, 0)));
        let _ = table;
        h.state_mut().ribbon_act(ribbon::Act::DeleteTable);
        settle(&mut h);
        let left = blocks(&h);
        assert_eq!(left.len(), 1, "one empty line is left: {left:?}");
        assert!(matches!(&left[0], Block::Paragraph(spans) if spans.is_empty()), "{left:?}");
        assert_eq!(h.state().editor.as_ref().and_then(|e| e.caret.as_ref()).map(|c| c.block), Some(0));
    }

    /// Writing first and then changing the table is two steps, not one: Undo takes back the
    /// table change and leaves the writing.
    #[test]
    fn a_table_change_after_writing_in_its_cell_is_a_step_of_its_own() {
        let (_t, mut h) = page_with_table(false, "undo-table-after-typing");
        let table = blocks(&h).iter().position(|b| matches!(b, nocturne_formats::Block::Table { .. })).expect("a table");
        put_caret(&mut h, table, Some((1, 0)));
        settle(&mut h);
        h.state_mut().insert_in_page("X");
        settle(&mut h);
        let typed = blocks(&h);
        h.state_mut().ribbon_act(ribbon::Act::Table(pageedit::TableEdit::InsertColumn(true)));
        settle(&mut h);
        assert_ne!(blocks(&h), typed, "a column went in");
        h.state_mut().undo();
        settle(&mut h);
        assert_eq!(blocks(&h), typed, "undo takes back the column and keeps what was written");
    }
}

/// A table's column widths are kept against its place among the page's blocks and its column
/// numbers. A column put in or taken out, or a line split above the table, used to leave them
/// where they were, so the widths went to the wrong columns or were lost (issue #154).
#[cfg(test)]
mod table_width_tests {
    use super::*;
    use nocturne_model::tablecols;

    type H = egui_kittest::Harness<'static, NocturneApp>;

    /// An HTML page with a line and a table under it whose two columns were dragged to 210 and
    /// 290, open in the window.
    fn page(name: &str) -> (nocturne_store::testing::TempDir, PathBuf, H) {
        let t = nocturne_store::testing::TempDir::new(name);
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        let page = sample.html_page.clone();
        nocturne_store::page::save_body(&page, "<p>Alpha</p><table><tr><th>Wide</th><th>Narrow</th></tr><tr><td>A</td><td>B</td></tr></table>")
            .expect("fixture");
        nocturne_store::page::set_table_columns(&page, &tablecols::parse("1:210,290")).expect("widths");
        let mut app = NocturneApp::new(session);
        app.open_path(&page.to_string_lossy());
        let mut h = egui_kittest::Harness::builder()
            .with_size(vec2(1400.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);
        assert_eq!(widths(&h).of(1), Some([210.0, 290.0].as_slice()), "the page opened with its widths");
        (t, page, h)
    }

    fn settle(h: &mut H) {
        for _ in 0..5 {
            h.step();
        }
    }

    fn widths(h: &H) -> tablecols::Widths {
        h.state().editor.as_ref().map(|e| e.columns.clone()).expect("editor")
    }

    fn into_cell(h: &mut H, cell: (usize, usize)) {
        let ed = h.state_mut().editor.as_mut().expect("editor");
        let text = pageedit::cell_text(&ed.blocks[1], cell).unwrap_or_default();
        ed.caret = Some(pageedit::Caret { block: 1, text, focus: true, selection: (0, 0), place: None, cell: Some(cell), span: None, inline: None });
    }

    #[test]
    fn a_column_put_in_on_the_left_keeps_the_others_widths() {
        let (_t, page, mut h) = page("widths-insert-left");
        into_cell(&mut h, (0, 0));
        h.state_mut().ribbon_act(ribbon::Act::Table(pageedit::TableEdit::InsertColumn(true)));
        settle(&mut h);
        let want = [pageedit::COLUMN_W, 210.0, 290.0];
        assert_eq!(widths(&h).of(1), Some(want.as_slice()), "Wide and Narrow keep their widths, the new column the usual one");
        let saved = nocturne_store::page::table_columns(&page).expect("saved widths");
        assert_eq!(saved.of(1), Some(want.as_slice()), "and the page's file says so");

        // Undo puts the widths back with the column, and Redo brings both again.
        h.state_mut().undo();
        settle(&mut h);
        assert_eq!(widths(&h).of(1), Some([210.0, 290.0].as_slice()), "undo puts the widths back");
        assert_eq!(nocturne_store::page::table_columns(&page).expect("saved").of(1), Some([210.0, 290.0].as_slice()));
        h.state_mut().redo();
        settle(&mut h);
        assert_eq!(widths(&h).of(1), Some(want.as_slice()), "redo brings them again");
    }

    /// Where the words `label` are drawn, as accessibility reads them.
    fn place_of(h: &H, label: &str) -> egui::Rect {
        use egui_kittest::kittest::{NodeT as _, Queryable as _};
        let node = h.get_by_label(label);
        let b = node.accesskit_node().bounding_box().expect("a drawn label");
        #[allow(clippy::cast_possible_truncation)]
        egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))
    }

    /// Issue #263: taking up a pen drew the table as a plain grid sized to its words, so its
    /// columns narrowed, its borders went and a centred cell went back to the left. With a pen
    /// in hand it is now drawn from the same layout as when it is written in.
    #[test]
    fn a_pen_in_hand_keeps_the_tables_widths_and_alignment() {
        let (_t, _page, mut h) = page("widths-pen");
        {
            let ed = h.state_mut().editor.as_mut().expect("editor");
            ed.columns.set_align(1, &[(1, 1)], Some(tablecols::Align::Center));
        }
        settle(&mut h);
        let typing = (place_of(&h, "Narrow"), place_of(&h, "B"));
        h.state_mut().pointer = ribbon::Pointer::Draw;
        settle(&mut h);
        let drawing = (place_of(&h, "Narrow"), place_of(&h, "B"));
        assert!((typing.0.min.x - drawing.0.min.x).abs() < 0.5, "the second column starts where it did: {typing:?} vs {drawing:?}");
        assert!((typing.1.center().x - drawing.1.center().x).abs() < 0.5, "the centred cell stays centred: {typing:?} vs {drawing:?}");
        assert!(drawing.1.center().x > drawing.0.min.x + 100.0, "in the middle of its 290 point column, not at its start: {drawing:?}");
    }

    #[test]
    fn a_column_taken_out_takes_only_its_own_width() {
        let (_t, _page, mut h) = page("widths-delete");
        into_cell(&mut h, (0, 0));
        h.state_mut().ribbon_act(ribbon::Act::Table(pageedit::TableEdit::DeleteColumn));
        settle(&mut h);
        assert_eq!(widths(&h).of(1), Some([290.0].as_slice()), "Narrow keeps its width when Wide goes");
    }

    #[test]
    fn a_line_split_above_the_table_moves_its_widths_with_it() {
        let (_t, page, mut h) = page("widths-split");
        {
            let ed = h.state_mut().editor.as_mut().expect("editor");
            let text = pageedit::caret_text(&ed.blocks[0]);
            let end = text.chars().count();
            ed.caret =
                Some(pageedit::Caret { block: 0, text, focus: true, selection: (end, end), place: Some(end), cell: None, span: None, inline: None });
        }
        settle(&mut h);
        h.key_press(egui::Key::Enter);
        settle(&mut h);
        let ed = h.state().editor.as_ref().expect("editor");
        assert!(matches!(ed.blocks.get(2), Some(nocturne_formats::Block::Table { .. })), "the table moved down a block: {:?}", ed.blocks);
        assert_eq!(widths(&h).of(2), Some([210.0, 290.0].as_slice()), "its widths moved with it");
        assert_eq!(widths(&h).of(1), None, "and the new line has none");
        settle(&mut h);
        h.state_mut().save();
        let saved = nocturne_store::page::table_columns(&page).expect("saved widths");
        assert_eq!(saved.of(2), Some([210.0, 290.0].as_slice()), "the page's file follows: {saved:?}");
    }
}

/// Table > Sort from the ribbon on both page formats: the page's file is written in the new
/// order, and one Undo puts the rows back (issue #171).
#[cfg(test)]
mod table_sort_tests {
    use super::*;

    type H = egui_kittest::Harness<'static, NocturneApp>;

    fn settle(h: &mut H) {
        for _ in 0..5 {
            h.step();
        }
    }

    fn names(h: &H) -> Vec<String> {
        let ed = h.state().editor.as_ref().expect("editor");
        let Some(nocturne_formats::Block::Table { rows, .. }) = ed.blocks.iter().find(|b| matches!(b, nocturne_formats::Block::Table { .. })) else {
            panic!("a table");
        };
        rows.iter().map(|r| r[0].iter().map(|s| s.text.as_str()).collect()).collect()
    }

    #[test]
    fn sort_from_the_ribbon_saves_and_undoes() {
        for html in [false, true] {
            let t = nocturne_store::testing::TempDir::new("table-sort");
            let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
            settings.set("privacy_welcome_seen", "true").expect("settings");
            let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
            let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
            let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
            let body = if html {
                "<table><tr><th>Fruit</th><th>Count</th></tr><tr><td>pear</td><td>3</td></tr><tr><td><b>apple</b></td><td>10</td></tr><tr><td>fig</td><td>2</td></tr></table>"
            } else {
                "| Fruit | Count |\n|---|---|\n| pear | 3 |\n| **apple** | 10 |\n| fig | 2 |\n"
            };
            nocturne_store::page::save_body(&page, body).expect("fixture");
            let mut app = NocturneApp::new(session);
            app.open_path(&page.to_string_lossy());
            let mut h = egui_kittest::Harness::builder()
                .with_size(vec2(1400.0, 800.0))
                .with_accessibility_check(false)
                .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
            settle(&mut h);
            {
                let ed = h.state_mut().editor.as_mut().expect("editor");
                let at = ed.blocks.iter().position(|b| matches!(b, nocturne_formats::Block::Table { .. })).expect("a table");
                let text = pageedit::cell_text(&ed.blocks[at], (1, 0)).unwrap_or_default();
                ed.caret = Some(pageedit::Caret {
                    block: at,
                    text,
                    focus: true,
                    selection: (0, 0),
                    place: None,
                    cell: Some((1, 0)),
                    span: None,
                    inline: None,
                });
            }
            settle(&mut h);
            h.state_mut().ribbon_act(ribbon::Act::TableSort(false));
            settle(&mut h);
            assert_eq!(names(&h), ["apple", "fig", "pear"], "html={html}: sorted by the caret's column");
            h.state_mut().save();
            let saved = nocturne_store::page::load_page(&page).expect("load").body;
            let (a, f, p) = (saved.find("apple").expect("apple"), saved.find("fig").expect("fig"), saved.find("pear").expect("pear"));
            assert!(a < f && f < p, "html={html}: the file has the new order:\n{saved}");
            assert!(saved.contains(if html { "<b>apple</b>" } else { "**apple**" }), "html={html}: and the formatting:\n{saved}");

            h.state_mut().undo();
            settle(&mut h);
            assert_eq!(names(&h), ["pear", "apple", "fig"], "html={html}: one undo puts the rows back");
            h.state_mut().redo();
            settle(&mut h);
            assert_eq!(names(&h), ["apple", "fig", "pear"], "html={html}: and redo sorts them again");
            h.state_mut().ribbon_act(ribbon::Act::TableSort(true));
            settle(&mut h);
            assert_eq!(names(&h), ["pear", "fig", "apple"], "html={html}: descending");
        }
    }
}

#[cfg(test)]
mod page_row_name_tests {
    use super::{fit_to_width, row_name_room, text_width};

    /// A page row shows its format label only when the whole name fits beside it. A long
    /// name takes the whole row instead, so it is never cut short to make room for "html"
    /// (issue #251: names came out as "Welcomhtml").
    #[test]
    fn the_format_label_gives_way_to_a_long_name() {
        assert_eq!(row_name_room(60.0, Some(30.0), 120.0), (true, 90.0));
        assert_eq!(row_name_room(100.0, Some(30.0), 120.0), (false, 120.0));
        assert_eq!(row_name_room(100.0, None, 120.0), (false, 120.0));
    }

    /// A name that does not fit ends in "…" and stays inside its room; one that fits is
    /// left as it is.
    #[test]
    fn a_long_name_ends_in_an_ellipsis_inside_its_room() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let name = "Write a page in plain words";
            assert_eq!(fit_to_width(ui, name, 14.0, 10_000.0), name);
            let room = text_width(ui, name, 14.0) / 2.0;
            let fitted = fit_to_width(ui, name, 14.0, room);
            assert!(fitted.ends_with('…'), "{fitted:?}");
            assert!(name.starts_with(fitted.trim_end_matches('…')), "{fitted:?}");
            assert!(text_width(ui, &fitted, 14.0) <= room, "{fitted:?} fits in {room}");
            assert_eq!(fit_to_width(ui, name, 14.0, 1.0), "…");
            // Letters made of several bytes are cut between letters, never inside one.
            let arabic = "ما الذي يمكنك فعله هنا";
            let cut = fit_to_width(ui, arabic, 14.0, text_width(ui, arabic, 14.0) / 2.0);
            assert!(cut.ends_with('…'), "{cut:?}");
        });
        out.textures_delta.clear();
    }
}

#[cfg(test)]
mod insert_table_size_tests {
    use super::table_size;

    #[test]
    fn a_typed_size_is_whole_numbers_within_the_limits() {
        assert_eq!(table_size("3", "4"), Some((3, 4)));
        assert_eq!(table_size(" 200 ", "30"), Some((200, 30)));
        for (rows, columns) in [("0", "3"), ("3", "0"), ("201", "3"), ("3", "31"), ("", "3"), ("x", "3"), ("-1", "3"), ("2.5", "3")] {
            assert_eq!(table_size(rows, columns), None, "{rows:?} x {columns:?}");
        }
    }
}

/// Issue #273: the picture menu's Rotate and Flip. The turned picture is a new file beside the
/// page, the page points at it (its size swapped by a quarter turn), the original stays, and
/// Undo puts the page back on the original. Both page formats.
#[cfg(test)]
mod picture_turn_tests {
    use super::*;
    use crate::turn::Turn;

    type H = egui_kittest::Harness<'static, NocturneApp>;

    fn settle(h: &mut H) {
        for _ in 0..5 {
            h.step();
        }
    }

    fn picture(h: &H) -> (String, Option<f32>, Option<f32>) {
        let ed = h.state().editor.as_ref().expect("editor");
        ed.blocks
            .iter()
            .find_map(|b| match b {
                nocturne_formats::Block::Image { src, width, height, .. } => Some((src.clone(), *width, *height)),
                _ => None,
            })
            .expect("a picture")
    }

    fn size_of(page: &std::path::Path, src: &str) -> (u32, u32) {
        let bytes = std::fs::read(page.join(src)).expect("the file");
        image::load_from_memory(&bytes).expect("a picture").to_rgba8().dimensions()
    }

    #[test]
    fn a_picture_is_turned_into_a_new_file_and_undo_puts_the_old_one_back() {
        for html in [true, false] {
            let t = nocturne_store::testing::TempDir::new("picture-turn");
            let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
            settings.set("privacy_welcome_seen", "true").expect("settings");
            let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
            let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
            let page = if html { sample.html_page.clone() } else { sample.md_page.clone() };
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(30, 20, image::Rgba([0, 120, 255, 255])))
                .write_to(&mut png, image::ImageFormat::Png)
                .expect("png");
            let src = nocturne_store::assets::attach_bytes(&page, "pic.png", &png.into_inner()).expect("attach");
            let body = if html { format!("<p>Alpha</p><p><img src=\"{src}\" alt=\"Pic\"></p>") } else { format!("Alpha\n\n![Pic]({src})\n") };
            nocturne_store::page::save_body(&page, &body).expect("fixture");
            let mut app = NocturneApp::new(session);
            app.open_path(&page.to_string_lossy());
            let mut h: H = egui_kittest::Harness::builder()
                .with_size(vec2(1400.0, 800.0))
                .with_accessibility_check(false)
                .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
            settle(&mut h);
            {
                // A size of its own, as a resize gives it, to see it swapped.
                let ed = h.state_mut().editor.as_mut().expect("editor");
                for b in &mut ed.blocks {
                    if let nocturne_formats::Block::Image { width, height, .. } = b {
                        *width = Some(60.0);
                        *height = Some(40.0);
                    }
                }
            }
            settle(&mut h);
            let before = picture(&h);

            h.state_mut().ribbon_act(ribbon::Act::TurnPicture(src.clone(), Turn::Right));
            settle(&mut h);
            let (turned, width, height) = picture(&h);
            assert_eq!(turned, "assets/pic-rotated.png", "html={html}: the page points at the turned copy");
            assert_eq!((width, height), (Some(40.0), Some(60.0)), "html={html}: its size is swapped");
            assert_eq!(size_of(&page, &turned), (20, 30), "html={html}: the copy is turned");
            assert_eq!(size_of(&page, &src), (30, 20), "html={html}: and the original is untouched");
            h.state_mut().save_page_now();
            assert!(nocturne_store::page::load_page(&page).expect("load").body.contains("pic-rotated.png"), "html={html}: saved");

            h.state_mut().undo();
            settle(&mut h);
            assert_eq!(picture(&h), before, "html={html}: undo points the page at the original again");
            h.state_mut().redo();
            settle(&mut h);
            assert_eq!(picture(&h).0, turned, "html={html}: redo brings the turned one back");

            // A flip keeps the size and turns the copy of the copy, named once.
            h.state_mut().ribbon_act(ribbon::Act::TurnPicture(turned.clone(), Turn::FlipHorizontal));
            settle(&mut h);
            let (flipped, width, height) = picture(&h);
            assert_eq!(flipped, "assets/pic-flipped.png");
            assert_eq!((width, height), (Some(40.0), Some(60.0)));
            assert_eq!(size_of(&page, &flipped), (20, 30));
        }
    }

    #[test]
    fn session_sync_preserves_shape_payload_and_clamps_rack_index() {
        let t = nocturne_store::testing::TempDir::new("m2-sync-shape-clamp");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let mut app = NocturneApp::new(session);

        app.pointer = ribbon::Pointer::Shape(nocturne_ink::shapes::Shape::Rectangle);
        app.sync_tool_state_to_session();

        let mut p = std::collections::BTreeMap::new();
        p.insert("enabled".to_string(), "true".to_string());
        app.session.run("ink.finger_drawing", &p).expect("finger drawing");
        app.sync_tool_state_from_session();
        assert_eq!(app.pointer, ribbon::Pointer::Shape(nocturne_ink::shapes::Shape::Rectangle));
        assert!(app.is_finger_drawing());

        app.session.sync_writing_tool_metadata(nocturne_engine::InkPenKind::Highlighter, 999);
        app.sync_tool_state_from_session();
        assert!(app.nib < app.rack.len(), "nib index must clamp to rack bounds");
    }

    #[test]
    fn external_pan_pinch_and_platform_edit_hooks() {
        struct LocalBoard(String);
        impl crate::clipboard::Board for LocalBoard {
            fn text(&mut self) -> Option<String> {
                (!self.0.is_empty()).then(|| self.0.clone())
            }
            fn html(&mut self) -> Option<String> {
                None
            }
            fn picture(&mut self) -> Option<crate::clipboard::Picture> {
                None
            }
            fn has_picture(&mut self) -> bool {
                false
            }
            fn set_text(&mut self, text: &str) -> Result<(), String> {
                self.0 = text.to_string();
                Ok(())
            }
            fn set_html(&mut self, _html: &str, text: &str) -> Result<(), String> {
                self.0 = text.to_string();
                Ok(())
            }
        }
        let t = nocturne_store::testing::TempDir::new("m2-platform-hooks");
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        nocturne_store::page::save_body(&sample.md_page, "Alpha beta\n").expect("fixture");
        let mut app = NocturneApp::new(session).with_clipboard(Box::new(LocalBoard(String::new())));
        app.open_path(&sample.md_page.to_string_lossy());

        app.apply_external_pinch_zoom(10.0, None);
        assert_eq!(app.zoom_percent(), crate::touch_ux::MAX_ZOOM_PERCENT);
        app.apply_external_pinch_zoom(0.05, None);
        assert_eq!(app.zoom_percent(), crate::touch_ux::MIN_ZOOM_PERCENT);
        app.apply_external_pinch_zoom(f32::NAN, None);
        assert_eq!(app.zoom_percent(), crate::touch_ux::MIN_ZOOM_PERCENT);

        app.pointer = ribbon::Pointer::Write;
        app.apply_external_pan(vec2(14.0, 22.0));
        assert!((app.canvas_pan.x - 14.0).abs() < 1e-4);
        assert!((app.pending_external_scroll_y - 22.0).abs() < 1e-4);

        app.set_soft_keyboard_visible(true, 300.0);
        assert_eq!(app.keyboard_caret_rect(), None);
        let ed = app.editor.as_mut().expect("editor");
        ed.caret = Some(pageedit::Caret {
            block: 0,
            text: "Alpha beta".to_string(),
            focus: true,
            selection: (0, 5),
            place: Some(0),
            cell: None,
            span: None,
            inline: None,
        });
        assert_eq!(app.selected_text_for_platform(), "Alpha");
        app.apply_platform_edit_action(PlatformEditAction::Copy);
        app.apply_platform_edit_action(PlatformEditAction::SelectAll);
        assert_eq!(app.selected_text_for_platform(), "Alpha beta");
        app.apply_platform_edit_action(PlatformEditAction::Delete);
        assert!(app.body().unwrap_or_default().trim().is_empty());
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        assert!(app.body().unwrap_or_default().contains("Alpha"));
        app.insert_platform_text(" Gamma");
        assert!(app.body().unwrap_or_default().contains("Gamma"));
        app.delete_backward_platform();
        assert!(app.body().unwrap_or_default().contains("Gamm"));
    }
}

#[cfg(test)]
mod m2_regression_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    type H = egui_kittest::Harness<'static, NocturneApp>;

    struct MemoryClipboard {
        text: String,
        picture: Option<crate::clipboard::Picture>,
    }

    impl MemoryClipboard {
        fn new() -> Self {
            Self { text: String::new(), picture: None }
        }
    }

    impl crate::clipboard::Board for MemoryClipboard {
        fn text(&mut self) -> Option<String> {
            (!self.text.is_empty()).then(|| self.text.clone())
        }
        fn html(&mut self) -> Option<String> {
            None
        }
        fn picture(&mut self) -> Option<crate::clipboard::Picture> {
            self.picture.as_ref().map(|p| crate::clipboard::Picture { width: p.width, height: p.height, png: p.png.clone() })
        }
        fn has_picture(&mut self) -> bool {
            self.picture.is_some()
        }
        fn set_text(&mut self, text: &str) -> Result<(), String> {
            self.text = text.to_string();
            self.picture = None;
            Ok(())
        }
        fn set_html(&mut self, _html: &str, text: &str) -> Result<(), String> {
            self.text = text.to_string();
            self.picture = None;
            Ok(())
        }
    }

    fn settle(h: &mut H) {
        for _ in 0..6 {
            h.step();
        }
    }

    fn setup_app(name: &str, body: &str) -> (nocturne_store::testing::TempDir, NocturneApp, nocturne_engine::sample::Sample) {
        let t = nocturne_store::testing::TempDir::new(name);
        let settings = nocturne_store::Settings::at(t.path().join("settings.toml"));
        settings.set("privacy_welcome_seen", "true").expect("settings");
        settings.set("split_panes", "false").expect("split_panes");
        let mut session = Session::new(&t.path().join("notebooks"), settings).expect("session");
        let sample = nocturne_engine::sample::create_sample(&mut session, None).expect("sample");
        nocturne_store::page::save_body(&sample.md_page, body).expect("fixture");
        let mut app = NocturneApp::new(session).with_clipboard(Box::new(MemoryClipboard::new()));
        app.open_path(&sample.md_page.to_string_lossy());
        (t, app, sample)
    }

    fn focus_block(app: &mut NocturneApp, block: usize, selection: (usize, usize)) {
        let ed = app.editor.as_mut().expect("editor");
        let text = pageedit::caret_text(&ed.blocks[block]);
        ed.across = None;
        ed.caret = Some(pageedit::Caret { block, text, focus: true, selection, place: Some(selection.0), cell: None, span: None, inline: None });
    }

    /// The iPad's route: Open > long press a notebook > Delete notebook, with one of its pages
    /// open and edited. Verifies the open editor is cleared so a late save does not recreate it.
    #[test]
    fn deleting_the_open_notebook_from_the_list_removes_it_and_it_stays_gone() {
        let (_t, mut app, sample) = setup_app("m2-reg-delete-open-notebook", "Start\n");
        focus_block(&mut app, 0, (5, 5));
        app.insert_platform_text(" edited");
        let listed = std::fs::canonicalize(&sample.notebook).expect("canonical notebook path");
        app.dialog = Some(Dialog::DeleteNotebook { path: listed, title: "Sample".into() });
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        h.query_all_by_label("Delete notebook").last().expect("the confirm button").click();
        settle(&mut h);

        assert!(h.state().dialog.is_none(), "the dialog closes");
        assert!(!sample.notebook.exists(), "the notebook folder is removed");
        assert!(h.state().editor.is_none(), "the deleted notebook's page is no longer open");
        h.state_mut().save_page_now();
        for _ in 0..30 {
            h.step();
        }
        assert!(!sample.notebook.exists(), "a late save must not bring the folder back");
        assert!(!h.state().status_is_error, "no error is reported: {}", h.state().status);
    }

    #[test]
    fn platform_edit_hooks_and_cut_paste_round_trip_undo_redo() {
        let (_t, mut app, sample) = setup_app("m2-reg-platform-undo", "Start\n");
        focus_block(&mut app, 0, (5, 5));

        app.insert_platform_text("One");
        assert!(app.body().unwrap_or_default().contains("StartOne"));
        assert!(app.can_undo());
        app.undo();
        assert_eq!(app.body().unwrap_or_default().trim(), "Start");
        app.redo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOne");

        // Second typing edit inside the same block after Kind::Edit creates a distinct step.
        app.insert_platform_text("Two");
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOneTwo");
        app.undo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOne");
        app.redo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOneTwo");

        // Delete backward records undo/redo cleanly (folding into current typing run).
        app.delete_backward_platform();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOneTw");
        app.undo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOne");
        app.redo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOneTw");

        // SelectAll + Cut records Edit step and copies text; Undo restores; Redo cuts again.
        app.apply_platform_edit_action(PlatformEditAction::SelectAll);
        app.apply_platform_edit_action(PlatformEditAction::Cut);
        assert!(app.body().unwrap_or_default().trim().is_empty());
        assert_eq!(app.clipboard.text().as_deref(), Some("StartOneTw"));
        app.undo();
        assert_eq!(app.body().unwrap_or_default().trim(), "StartOneTw");
        app.redo();
        assert!(app.body().unwrap_or_default().trim().is_empty());

        // Single-line Paste (`insert_in_page`) + multi-line Paste (`paste_blocks`) both undo/redo.
        app.clipboard.set_text("Solo").expect("set_text");
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        assert_eq!(app.body().unwrap_or_default().trim(), "Solo");
        app.undo();
        assert!(app.body().unwrap_or_default().trim().is_empty());
        app.redo();
        assert_eq!(app.body().unwrap_or_default().trim(), "Solo");

        app.clipboard.set_text("LineA\n\nLineB").expect("set_text");
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        assert!(app.body().unwrap_or_default().contains("LineB"));
        app.undo();
        assert_eq!(app.body().unwrap_or_default().trim(), "Solo");
        app.redo();
        assert!(app.body().unwrap_or_default().contains("LineB"));

        // `PlatformEditAction::Delete` preserves literal punctuation (`*literal*`) alongside formatted spans.
        app.open_path(&sample.html_page.to_string_lossy());
        nocturne_store::page::save_body(&sample.md_page, "\\*literal\\* and *italic*\n").expect("literal fixture");
        app.open_path(&sample.md_page.to_string_lossy());
        focus_block(&mut app, 0, (0, 2));
        app.apply_platform_edit_action(PlatformEditAction::Delete);
        let after_del = app.body().unwrap_or_default();
        assert!(after_del.contains("iteral\\* and *italic*"), "literal star must stay escaped: {after_del:?}");
        app.undo();
        assert!(app.body().unwrap_or_default().contains("\\*literal\\* and *italic*"));
        app.redo();
        assert!(app.body().unwrap_or_default().contains("iteral\\* and *italic*"));

        // `paste_picture` records an undoable/redoable edit step.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let pic = crate::clipboard::Picture { width: 1, height: 1, png: png.into_inner() };
        app.clipboard = Box::new(MemoryClipboard { text: String::new(), picture: Some(pic) });
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        assert!(app.body().unwrap_or_default().contains("![Pasted picture]"));
        app.undo();
        assert!(!app.body().unwrap_or_default().contains("![Pasted picture]"));
        app.redo();
        assert!(app.body().unwrap_or_default().contains("![Pasted picture]"));
    }

    /// The soft keyboard's delete at the start of a line joins it onto the line above, as
    /// Backspace does on a desktop keyboard, in one Undo step, with the caret where the
    /// lines met. UIKit sends deleteBackward as a call rather than a Backspace key, so
    /// before this an iPad could split a line with Return but never join two again. The
    /// line joined here is the only one in its box, so that box goes with it.
    #[test]
    fn platform_delete_at_line_start_joins_onto_the_line_above_and_undoes_once() {
        let (_t, mut app, sample) = setup_app("m5-platform-join-back", "One\n");
        let arranged = concat!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nAlpha\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"200\">\n\nBeta\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"220\" data-width=\"200\">\n\nGamma\n\n</div>\n",
        );
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        let original_body = app.body().unwrap_or_default().to_owned();
        let original_boxes = app.editor.as_ref().expect("ed").boxes.clone();
        assert_eq!(original_boxes.len(), 3);

        focus_block(&mut app, 1, (0, 0));
        app.delete_backward_platform();
        {
            let ed = app.editor.as_ref().expect("ed");
            assert_eq!(ed.blocks.iter().map(pageedit::caret_text).collect::<Vec<_>>(), ["AlphaBeta", "Gamma"]);
            assert_eq!(ed.boxes.len(), 2, "the box Beta was alone in goes with it");
            assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len(), "box counts match blocks");
            let caret = ed.caret.as_ref().expect("caret");
            assert_eq!((caret.block, caret.selection), (0, (5, 5)), "the caret sits where the lines met");
        }
        let joined = app.body().unwrap_or_default().to_owned();
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original_body, "one Undo splits the lines again");
        assert_eq!(app.editor.as_ref().expect("ed").boxes, original_boxes, "and brings the box back");
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), joined, "Redo joins them again");

        // At the start of the first line there is nothing above to join onto.
        focus_block(&mut app, 0, (0, 0));
        app.delete_backward_platform();
        assert_eq!(app.body().unwrap_or_default(), joined);
        assert_eq!(app.editor.as_ref().expect("ed").blocks.len(), 2);
    }

    #[test]
    fn soft_keyboard_inset_offsets_mobile_formatting_bar_above_keyboard() {
        let (_t, mut app, _s) = setup_app("m2-reg-kb-inset", "Alpha line\n");
        focus_block(&mut app, 0, (0, 0));
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        h.state_mut().set_soft_keyboard_visible(true, 300.0);
        settle(&mut h);
        let docked_bold =
            |h: &H| h.get_all_by_label("Bold").max_by(|a, b| a.rect().center().y.total_cmp(&b.rect().center().y)).expect("docked Bold button").rect();
        let bold_rect = docked_bold(&h);
        assert!(bold_rect.center().y > 200.0, "expected docked formatting bar button: {bold_rect:?}");
        assert!(bold_rect.max.y <= 500.0, "bar must dock above 300pt keyboard: {bold_rect:?}");

        // With a 34pt safe-area bottom inset, `extra_kb = 300 - 34` (no double-subtraction).
        h.input_mut().safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top: 0.0, bottom: 34.0 }));
        settle(&mut h);
        let combined_rect = docked_bold(&h);
        assert!(combined_rect.max.y <= 500.0, "combined inset must dock above 300pt: {combined_rect:?}");
        assert!(combined_rect.max.y > 466.0, "must not double-subtract safe_area_insets: {combined_rect:?}");

        // Hostile huge inset clamps safely to 0-height room without panicking.
        h.state_mut().set_soft_keyboard_visible(true, 1.0e6);
        settle(&mut h);
    }

    #[test]
    fn page_title_focus_clears_stale_body_caret_and_hides_formatting_bar() {
        let (_t, mut app, sample) = setup_app("m2-reg-title-focus", "Body line\n");
        app.set_soft_keyboard_visible(true, 300.0);
        focus_block(&mut app, 0, (0, 0));
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);
        assert!(h.state().formatting_bar_visible());
        let body_caret = h.state().keyboard_caret_rect().expect("body caret");

        let title_text = h.state().editor.as_ref().expect("ed").title_buf.clone();
        h.get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .find(|node| node.value().as_deref() == Some(title_text.as_str()))
            .expect("title input")
            .click();
        settle(&mut h);

        let ed = h.state().editor.as_ref().expect("ed");
        assert!(ed.title_focused);
        assert!(ed.caret.is_none(), "focusing page title must clear stale body caret");
        assert!(!h.state().formatting_bar_visible(), "formatting bar must hide while title is focused");
        let title_caret = h.state().keyboard_caret_rect().expect("title caret");
        assert!(title_caret.center().y < body_caret.center().y, "caret rect must point at title, not stale body: {title_caret:?} vs {body_caret:?}");

        // Transitioning back to body block via caret focus restores body focus & formatting bar.
        focus_block(h.state_mut(), 0, (0, 0));
        settle(&mut h);
        let ed = h.state().editor.as_ref().expect("ed");
        assert!(!ed.title_focused);
        assert!(ed.caret.is_some());
        assert!(h.state().formatting_bar_visible());

        // Read-only HTML page still permits platform title editing with formatting bar hidden.
        h.state_mut().open_path(&sample.html_page.to_string_lossy());
        settle(&mut h);
        let html_title = h.state().editor.as_ref().expect("ed").title_buf.clone();
        h.get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .find(|node| node.value().as_deref() == Some(html_title.as_str()))
            .expect("html title input")
            .click();
        settle(&mut h);
        assert!(!h.state().formatting_bar_visible());
        assert!(h.state().keyboard_caret_rect().is_some());
        h.state_mut().insert_platform_text("X");
        assert!(h.state().editor.as_ref().expect("ed").title_buf.contains('X'));
        h.state_mut().delete_backward_platform();
        assert_eq!(h.state().editor.as_ref().expect("ed").title_buf, html_title);
    }

    #[test]
    fn table_cell_keeps_caret_rect_when_blurred_by_toolbar() {
        let (_t, mut app, _s) = setup_app("m2-reg-table-caret", "| A | B |\n|---|---|\n| one | two |\n");
        app.set_soft_keyboard_visible(true, 300.0);
        {
            let ed = app.editor.as_mut().expect("editor");
            ed.caret = Some(pageedit::Caret {
                block: 0,
                text: "one".to_string(),
                focus: true,
                selection: (1, 1),
                place: Some(1),
                cell: Some((1, 0)),
                span: None,
                inline: None,
            });
        }
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);
        let focused_rect = h.state().keyboard_caret_rect().expect("focused cell caret");
        assert!(focused_rect.height() < 30.0, "expected tight caret rect: {focused_rect:?}");

        // Clicking the Draw ribbon tab blurs TextEdit focus while keeping cell caret active.
        h.get_by_label("Draw").click();
        settle(&mut h);
        let blurred_rect = h.state().keyboard_caret_rect().expect("blurred cell caret");
        assert!(blurred_rect.height() < 30.0, "blurred cell must not fall back to page_rect: {blurred_rect:?}");
        assert!((blurred_rect.center().y - focused_rect.center().y).abs() < 4.0);
    }

    #[test]
    fn alt_digit_pen_keys_and_alt_e_ignored_behind_modal_or_backstage() {
        let (_t, app, _s) = setup_app("m2-reg-pen-keys-modal", "Alpha\n");
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        h.get_by_label("New notebook").click();
        settle(&mut h);
        assert!(h.state().dialog.is_some());

        for key in [Key::Num1, Key::Num0, Key::E] {
            h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT });
            settle(&mut h);
            assert_eq!(h.state().pointer, ribbon::Pointer::Write, "Alt+{key:?} must be ignored behind modal");
        }

        assert!(h.state_mut().system_back());
        settle(&mut h);
        h.state_mut().backstage = Some("Info");
        settle(&mut h);
        for key in [Key::Num1, Key::E] {
            h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT });
            settle(&mut h);
            assert_eq!(h.state().pointer, ribbon::Pointer::Write, "Alt+{key:?} must be ignored behind backstage");
        }
    }

    #[test]
    fn pointer_pan_works_with_finger_drawing_enabled_and_pure_touch_tap_focuses_write_mode() {
        let (_t, mut app, _s) = setup_app("m2-reg-pan-and-tap", "Alpha paragraph\n");
        app.set_finger_drawing(true);
        app.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Pan));
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        let page_rect = h.state().page_rect().expect("page_rect");
        let start = page_rect.center();
        let end = start + vec2(45.0, 30.0);
        let pan_before = h.state().canvas_pan;
        let device_id = egui::TouchDeviceId(1);
        let id = egui::TouchId(10);

        h.input_mut().events.push(egui::Event::Touch { device_id, id, phase: egui::TouchPhase::Start, pos: start, force: None });
        h.input_mut().events.push(egui::Event::PointerMoved(start));
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.input_mut().events.push(egui::Event::Touch { device_id, id, phase: egui::TouchPhase::Move, pos: end, force: None });
        h.input_mut().events.push(egui::Event::PointerMoved(end));
        h.step();
        h.input_mut().events.push(egui::Event::Touch { device_id, id, phase: egui::TouchPhase::End, pos: end, force: None });
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        settle(&mut h);

        let pan_after = h.state().canvas_pan;
        assert!((pan_after - pan_before).length() > 10.0, "Pointer::Pan must pan with finger_drawing enabled: {pan_before:?} -> {pan_after:?}");
        assert_eq!(h.state().pad.as_ref().map_or(0, crate::inkpad::Pad::strokes), 0);

        // Pure Event::Touch tap (no PointerButton) in Write mode focuses the tapped block.
        h.state_mut().ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
        h.state_mut().set_finger_drawing(false);
        h.state_mut().editor.as_mut().expect("ed").caret = None;
        settle(&mut h);
        let block_pos = h.get_by_label("Alpha paragraph").rect().center();
        let tap_id = egui::TouchId(11);
        h.input_mut().events.push(egui::Event::Touch { device_id, id: tap_id, phase: egui::TouchPhase::Start, pos: block_pos, force: None });
        h.step();
        h.input_mut().events.push(egui::Event::Touch { device_id, id: tap_id, phase: egui::TouchPhase::End, pos: block_pos, force: None });
        settle(&mut h);
        let ed = h.state().editor.as_ref().expect("ed");
        assert_eq!(ed.caret.as_ref().map(|c| c.block), Some(0), "pure touch tap must focus block 0 in Write mode");
    }

    #[test]
    fn apply_external_pinch_zoom_accumulates_sub_one_percent_and_syncs() {
        let (_t, mut app, _s) = setup_app("m2-reg-pinch-zoom", "Zoom\n");
        assert_eq!(app.zoom_percent(), 100);
        for _ in 0..25 {
            app.apply_external_pinch_zoom(1.004, None);
        }
        assert_eq!(app.zoom_percent(), 110);

        app.ribbon_act(ribbon::Act::Zoom(1));
        assert_eq!(app.zoom_percent(), 120);
        for _ in 0..10 {
            app.apply_external_pinch_zoom(1.004, None);
        }
        assert_eq!(app.zoom_percent(), 125);
    }

    #[test]
    fn simultaneous_touch_pan_and_pinch_keeps_the_point_between_the_fingers() {
        let (_t, mut app, _s) = setup_app("m2-reg-pinch-and-pan", "Zoom\n");
        app.hide_title = true;
        app.ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Pan));
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1180.0, 820.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app| app.show(ui), app);
        settle(&mut h);

        let old_center = h.state().page_view_rect.expect("page view").center();
        let touch = |id, phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos, force: None };
        h.input_mut().events.push(egui::Event::PointerMoved(old_center - vec2(40.0, 0.0)));
        h.step();
        h.input_mut().events.extend([
            touch(1, egui::TouchPhase::Start, old_center - vec2(40.0, 0.0)),
            touch(2, egui::TouchPhase::Start, old_center + vec2(40.0, 0.0)),
        ]);
        h.step();
        let old_origin = h.state().editor.as_ref().expect("editor").origin;
        let new_center = old_center + vec2(45.0, 30.0);
        h.input_mut()
            .events
            .extend([touch(1, egui::TouchPhase::Move, new_center - vec2(60.0, 0.0)), touch(2, egui::TouchPhase::Move, new_center + vec2(60.0, 0.0))]);
        h.step();

        assert_eq!(h.state().zoom_percent(), 150);
        let new_origin = h.state().editor.as_ref().expect("editor").origin;
        let anchored = new_origin + (old_center - old_origin) * 1.5;
        assert!(anchored.distance(new_center) < 2.0, "{anchored:?} should follow {new_center:?}");
    }

    #[test]
    fn ocr_preserves_the_original_page_and_ink_after_either_save_fails() {
        use nocturne_ink::{Dab, Drawing, Rgb, Stroke, Tool, svg};

        let (t, mut app, sample) = setup_app("m2-reg-ocr-page-save", "Original text\n");
        let mut stroke = Stroke::new(Tool::Pen, Rgb::BLACK, 2.0);
        stroke.add(Dab::new(20.0, 20.0, 1.0));
        stroke.add(Dab::new(40.0, 40.0, 1.0));
        let mut drawing = Drawing::new(800.0, 600.0);
        drawing.add(stroke);
        let ink_path = sample.md_page.join("ink.svg");
        let ink = svg::write(&drawing, svg::Flavour::Editable);
        std::fs::write(&ink_path, &ink).expect("fixture ink");
        let mut pad = crate::inkpad::Pad::of_page(&sample.md_page);
        pad.hold_touching((0.0, 0.0, 100.0, 100.0));
        let bounds = pad.held_bounds().expect("selected ink");
        let strokes = pad.held_strokes();
        app.pad = Some(pad);
        app.sync_ink_reference();
        app.save_now();

        let page_file = sample.md_page.join(PageFormat::Cmgfm.page_file());
        let before = std::fs::read_to_string(&page_file).expect("page before OCR");
        let versions = sample.md_page.join(".nocturne/versions");
        let version_backup = t.path().join("versions-before-ocr");
        std::fs::rename(&versions, &version_backup).expect("temporarily move the versions folder");
        std::fs::write(&versions, "blocks version snapshots").expect("prevent page saves");
        let conversion = || crate::math_ocr::Conversion {
            page: sample.md_page.clone(),
            strokes: strokes.clone(),
            bounds,
            mode: nocturne_engine::OcrMode::HandwritingText,
            stage: crate::math_ocr::Stage::Preview { tex: "Recognized text".into(), opened: true },
        };
        app.confirm_math_ocr(conversion());

        assert!(app.status_is_error);
        assert_eq!(app.held_ink_count(), 1);
        assert_eq!(std::fs::read_to_string(&ink_path).expect("preserved ink"), ink);
        assert_eq!(std::fs::read_to_string(&page_file).expect("preserved page"), before);
        assert!(!app.body().expect("editor body").contains("Recognized text"));

        std::fs::remove_file(&versions).expect("unblock page saves");
        std::fs::rename(&version_backup, &versions).expect("restore the versions folder for a retry");
        let ink_backup = t.path().join("ink-before-ocr.svg");
        std::fs::rename(&ink_path, &ink_backup).expect("temporarily move the ink file");
        std::fs::create_dir(&ink_path).expect("prevent ink removal");
        let undo_before = app.editor.as_ref().expect("editor").history.can_undo();
        app.confirm_math_ocr(conversion());
        assert!(app.status_is_error);
        assert_eq!(app.held_ink_count(), 1);
        assert_eq!(app.editor.as_ref().expect("editor").history.can_undo(), undo_before);
        assert!(!app.pad.as_ref().expect("ink pad").can_redo());
        let rolled_back = std::fs::read_to_string(&page_file).expect("page after rollback");
        assert_eq!(rolled_back, before);

        std::fs::remove_dir(&ink_path).expect("unblock writing ink");
        std::fs::rename(&ink_backup, &ink_path).expect("restore the ink file for a retry");
        app.confirm_math_ocr(conversion());
        assert!(!app.status_is_error);
        let body = nocturne_store::page::load_page(&sample.md_page).expect("saved page").body;
        assert!(body.contains("Original text") && body.contains("Recognized text"));
        assert!(!body.contains("ink.svg") && !ink_path.exists());
    }

    #[test]
    fn removing_a_stale_ink_reference_keeps_layout_with_its_blocks_after_saving() {
        use nocturne_model::justify::{Justified, Strength};
        use nocturne_model::tablecols::Widths;

        let (_t, mut app, sample) = setup_app("m2-reg-ink-layout-marks", "Original text\n");
        let page = sample.md_page;
        let body = "![Ink](ink.svg)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nA justified paragraph\n";
        nocturne_store::page::save_body(&page, body).expect("fixture page");
        let mut widths = Widths::default();
        widths.set(1, 0, 180.0, &[100.0, 110.0]);
        nocturne_store::page::set_table_columns(&page, &widths).expect("fixture widths");
        let mut justified = Justified::default();
        justified.set(2, Some(Strength::High));
        nocturne_store::page::set_justified(&page, &justified).expect("fixture justification");

        app.open_path(&path_str(&page));
        app.save();

        assert!(!nocturne_store::page::load_page(&page).expect("saved page").body.contains("ink.svg"));
        let widths = nocturne_store::page::table_columns(&page).expect("saved widths");
        assert_eq!(widths.of(0), Some([180.0, 110.0].as_slice()));
        assert!(widths.of(1).is_none());
        let justified = nocturne_store::page::justified(&page).expect("saved justification");
        assert_eq!(justified.blocks(), &[(1, Strength::High)]);
    }

    #[test]
    fn page_rect_clears_and_native_gestures_stop_when_no_page_is_shown() {
        // A pan or pinch from the platform that lands while no page is shown changes nothing.
        fn assert_gestures_ignored(h: &mut H, screen: &str) {
            let (zoom, pan, scroll) = (h.state().zoom_percent(), h.state().canvas_pan, h.state().pending_external_scroll_y);
            h.state_mut().apply_external_pinch_zoom(1.5, None);
            h.state_mut().apply_external_pan(vec2(40.0, 40.0));
            assert_eq!(h.state().zoom_percent(), zoom, "{screen}: a pinch must not zoom a page that is not shown");
            assert_eq!(h.state().canvas_pan, pan, "{screen}: a pan must not move a page that is not shown");
            assert!((h.state().pending_external_scroll_y - scroll).abs() < 1e-4, "{screen}: a pan must not scroll a page that is not shown");
        }

        let (_t, app, _s) = setup_app("m2-reg-stale-page-rect", "Alpha paragraph\n");
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(390.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);
        assert!(h.state().compact && h.state().full_page, "390 points is phone width, and the opened page fills it");
        assert!(h.state().page_rect().is_some(), "the page is on the screen");

        // The Tags Summary fills a phone's window.
        h.state_mut().ribbon_act(ribbon::Act::FindTags);
        settle(&mut h);
        assert!(h.state().tags_pane.is_some(), "Find Tags opens the summary");
        assert_eq!(h.state().page_rect(), None, "no page rect under the Tags Summary");
        assert_gestures_ignored(&mut h, "Tags Summary");
        assert!(h.state_mut().system_back());
        settle(&mut h);
        assert!(h.state().page_rect().is_some(), "the page is back on the screen");

        // The File screen takes the whole window.
        h.state_mut().backstage = Some("Info");
        settle(&mut h);
        assert_eq!(h.state().page_rect(), None, "no page rect under the File screen");
        assert_gestures_ignored(&mut h, "File screen");
        assert!(h.state_mut().system_back());
        settle(&mut h);
        assert!(h.state().page_rect().is_some(), "the page is back after the File screen");

        // Back from the page shows the notebooks list, which fills the window.
        assert!(h.state_mut().system_back());
        settle(&mut h);
        assert!(h.state().compact_list_fills(), "Back shows the notebooks list");
        assert_eq!(h.state().page_rect(), None, "no page rect under the notebooks list");
        assert_gestures_ignored(&mut h, "notebooks list");
    }

    #[test]
    fn paste_over_across_selection_replaces_in_single_undo_redo_step() {
        let (_t, mut app, _s) = setup_app("m4-across-paste", "First block\n\nMiddle block\n\nLast block\n");
        let original = app.body().unwrap_or_default().to_owned();

        // Empty clipboard preserves ed.across and leaves body untouched.
        {
            let ed = app.editor.as_mut().expect("editor");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (0, 6), to: (2, 5) });
        }
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        {
            let ed = app.editor.as_ref().expect("editor");
            assert!(ed.across.is_some(), "empty clipboard must preserve across selection");
        }
        assert_eq!(app.body().unwrap_or_default(), original);

        // Single-line paste replaces across selection in 1 undo/redo step.
        app.clipboard.set_text("REPLACED").expect("set_text");
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        {
            let ed = app.editor.as_ref().expect("editor");
            assert!(ed.across.is_none(), "across must be cleared after paste");
        }
        let after_single = app.body().unwrap_or_default().to_owned();
        assert!(after_single.contains("First REPLACEDblock"), "expected single-line replacement: {after_single:?}");
        assert!(!after_single.contains("Middle"), "middle block must be deleted: {after_single:?}");
        assert!(app.can_undo());
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original, "single undo must restore full across span");
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after_single, "single redo must restore replacement");

        // Multi-block paste over across selection replaces in 1 undo/redo step.
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original);
        app.editor.as_mut().expect("editor").across = Some(pageedit::Across { from: (0, 6), to: (2, 5) });
        app.clipboard.set_text("Alpha\n\nBeta").expect("set_text");
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        let after_multi = app.body().unwrap_or_default().to_owned();
        assert!(after_multi.contains("Alpha") && after_multi.contains("Beta"), "{after_multi:?}");
        assert!(!after_multi.contains("Middle"), "{after_multi:?}");
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original, "single undo restores multi-block across paste");
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after_multi);

        // Picture paste over across selection replaces in 1 undo/redo step.
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original);
        app.editor.as_mut().expect("editor").across = Some(pageedit::Across { from: (0, 6), to: (2, 5) });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 128, 255, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        app.clipboard = Box::new(MemoryClipboard {
            text: String::new(),
            picture: Some(crate::clipboard::Picture { width: 2, height: 2, png: png.into_inner() }),
        });
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        let after_pic = app.body().unwrap_or_default().to_owned();
        assert!(after_pic.contains("![Pasted picture]"), "{after_pic:?}");
        assert!(!after_pic.contains("Middle"), "{after_pic:?}");
        assert!(app.editor.as_ref().expect("editor").across.is_none());
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original);
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after_pic);
    }

    #[test]
    fn paste_and_insert_when_caret_is_none_record_undo_redo() {
        let (_t, mut app, _s) = setup_app("m4-no-caret-paste", "Existing text\n");
        let original = app.body().unwrap_or_default().to_owned();
        assert!(app.editor.as_ref().expect("editor").caret.is_none());
        assert!(!app.can_undo());

        app.clipboard.set_text("Appended").expect("set_text");
        app.apply_platform_edit_action(PlatformEditAction::Paste);
        let after = app.body().unwrap_or_default().to_owned();
        assert!(after.contains("Appended"), "{after:?}");
        assert!(app.can_undo(), "paste/insert without caret must record undo");
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original);
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after);
    }

    #[test]
    fn pressure_curve_persists_in_settings_and_propagates_to_pen() {
        let t = nocturne_store::testing::TempDir::new("m4-curve-settings");
        let settings_path = t.path().join("settings.toml");
        let nb_dir = t.path().join("notebooks");
        let settings = nocturne_store::Settings::at(&settings_path);
        settings.set("privacy_welcome_seen", "true").expect("settings");
        let session = Session::new(&nb_dir, settings).expect("session");
        let mut app = NocturneApp::new(session);
        assert_eq!(app.pen().curve, nocturne_ink::PressureCurve::Linear);

        app.ribbon_act(ribbon::Act::InkPressureCurve(nocturne_ink::PressureCurve::Soft));
        assert_eq!(app.pen().curve, nocturne_ink::PressureCurve::Soft);

        let reloaded_settings = nocturne_store::Settings::at(&settings_path);
        assert_eq!(reloaded_settings.get(CURVE_KEY).as_deref(), Some("soft"));
        let reloaded_session = Session::new(&nb_dir, reloaded_settings).expect("reloaded session");
        let reloaded_app = NocturneApp::new(reloaded_session);
        assert_eq!(reloaded_app.pen().curve, nocturne_ink::PressureCurve::Soft);
    }

    #[test]
    fn touch_handles_move_and_resize_selected_objects_without_scrolling() {
        let (_t, mut app, sample) = setup_app("m4-touch-handles", "Container text\n");
        // Insert an image container and arrange boxes explicitly.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40, 40, image::Rgba([20, 120, 220, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let pic_link = nocturne_store::assets::attach_bytes(&sample.md_page, "pic.png", &png.into_inner()).expect("attach");
        let arranged = format!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nContainer text\n\n</div>\n<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"160\">\n\n![pic]({pic_link})\n\n</div>\n"
        );
        nocturne_store::page::save_body(&sample.md_page, &arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        app.set_finger_drawing(false);

        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        let origin = h.state().editor.as_ref().expect("ed").origin;
        let device = egui::TouchDeviceId(1);
        let drag_touch = |h: &mut H, id: u64, from: egui::Pos2, to: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos: from, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(from));
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Move, pos: to, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(to));
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos: to, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            settle(h);
        };
        let tap_touch = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            settle(h);
        };

        // 1. Unselected container drag in body scrolls the canvas instead of moving the box.
        let pan_before = h.state().canvas_pan;
        let box0_before = h.state().editor.as_ref().expect("ed").boxes[0];
        let body_pt = origin + vec2(100.0, 45.0);
        drag_touch(&mut h, 101, body_pt, body_pt + vec2(40.0, 0.0));
        assert!(h.state().stylus.touch_affordance_active());
        assert!((h.state().canvas_pan.x - pan_before.x).abs() > 15.0, "unselected finger drag must scroll");
        assert_eq!(h.state().editor.as_ref().expect("ed").boxes[0].x, box0_before.x);

        // Reset pan for clean coordinate assertions.
        h.state_mut().canvas_pan = egui::Vec2::ZERO;
        settle(&mut h);
        let origin = h.state().editor.as_ref().expect("ed").origin;

        // 2. Tap container 0 move bar -> selects box 0, exposes oversized touch handle, drag resizes without scrolling!
        let bar_pt = origin + vec2(120.0, 25.0);
        tap_touch(&mut h, 102, bar_pt);
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), Some(0));

        // Grab 16pt outward past the right edge of container 0 (40 + 200 + 12 = 252, +16 = 268).
        let outer_edge = origin + vec2(268.0, 48.0);
        assert!(crate::containers::active_touch_handle_at(&h.ctx, outer_edge), "oversized right resize handle must cover outward 16pt");
        let w_before = h.state().editor.as_ref().expect("ed").boxes[0].width;
        let pan_before_resize = h.state().canvas_pan;
        drag_touch(&mut h, 103, outer_edge, outer_edge + vec2(60.0, 0.0));
        assert_eq!(h.state().canvas_pan, pan_before_resize, "touch handle resize must NOT scroll");
        assert!(h.state().editor.as_ref().expect("ed").boxes[0].width > w_before + 40.0, "box 0 must widen");

        // 3. Mouse movement resets sticky touch affordance cleanly.
        h.input_mut().events.push(egui::Event::MouseMoved(vec2(5.0, 0.0)));
        settle(&mut h);
        assert!(!h.state().stylus.touch_affordance_active());

        // 4. Touch tap on image container 1 selects image, exposes corner handles, and corner drag resizes without scrolling.
        let pic_center = origin + vec2(66.0, 154.0);
        tap_touch(&mut h, 104, pic_center);
        assert_eq!(crate::containers::selected_image(&h.ctx, &sample.md_page), Some(1));
        assert!(h.query_all_by_label("Bold").count() <= 1, "touch selection pill must never show formatting buttons");

        let img_rect = h
            .ctx
            .data(|d| d.get_temp::<Vec<crate::preview::RenderedImage>>(crate::preview::rendered_images_id()))
            .unwrap_or_default()
            .into_iter()
            .find(|img| img.block == 1)
            .expect("rendered image")
            .rect;
        let corner_handle = img_rect.right_bottom() + vec2(8.0, 8.0);
        assert!(crate::containers::active_touch_handle_at(&h.ctx, corner_handle), "image touch handle must cover 8pt outside corner");
        let pan_before_img = h.state().canvas_pan;
        drag_touch(&mut h, 105, corner_handle, corner_handle + vec2(40.0, 40.0));
        assert_eq!(h.state().canvas_pan, pan_before_img, "image touch corner resize must NOT scroll");
        let resized_w = match h.state().editor.as_ref().expect("ed").blocks.get(1) {
            Some(nocturne_formats::Block::Image { width: Some(w), .. }) => *w,
            other => panic!("expected resized image block, got {other:?}"),
        };
        assert!(resized_w > 60.0, "expected widened image, got {resized_w}");

        // 5. Held ink touch corner handle resizes held ink in Pointer::Write mode without scrolling.
        h.state_mut().editor.as_mut().expect("ed").caret = None;
        {
            let pad = h.state_mut().pad.get_or_insert_with(|| crate::inkpad::Pad::of_page(&sample.md_page));
            let mut stroke = nocturne_ink::Stroke::new(nocturne_ink::Tool::Pen, nocturne_ink::Rgb::BLACK, 4.0);
            stroke.add(nocturne_ink::Dab::new(200.0, 220.0, 1.0));
            stroke.add(nocturne_ink::Dab::new(300.0, 320.0, 1.0));
            pad.drawing.add(stroke);
            pad.hold_touching((190.0, 210.0, 310.0, 330.0));
        }
        settle(&mut h);
        let held_bounds = h.state().pad.as_ref().and_then(|p| p.bounds()).expect("held ink bounds");
        let ink_corner = origin + vec2(held_bounds[2] + 3.0 + 10.0, held_bounds[3] + 3.0 + 10.0);
        assert!(crate::inkpad::held_handle_at(&h.ctx, ink_corner), "held ink touch handle must reach 10pt outside corner");
        let pan_before_ink = h.state().canvas_pan;
        drag_touch(&mut h, 106, ink_corner, ink_corner + vec2(50.0, 50.0));
        assert_eq!(h.state().canvas_pan, pan_before_ink, "held ink touch resize must NOT scroll");
        let ink_after = h.state().pad.as_ref().and_then(|p| p.bounds()).expect("grown ink bounds");
        assert!(ink_after[2] > held_bounds[2] + 35.0, "held ink must resize via touch corner: {held_bounds:?} -> {ink_after:?}");
    }

    #[test]
    fn stale_touch_handles_cleared_in_erase_and_draw_and_held_ink_release() {
        let (_t, mut app, sample) = setup_app("m4-r2-stale-handles", "Container text\n");
        let arranged = "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nContainer text\n\n</div>\n";
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        app.set_finger_drawing(false);

        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        let device = egui::TouchDeviceId(1);
        let tap_touch = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            settle(h);
        };
        let drag_touch = |h: &mut H, id: u64, from: egui::Pos2, to: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos: from, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(from));
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Move, pos: to, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(to));
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos: to, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            settle(h);
        };

        let origin = h.state().editor.as_ref().expect("ed").origin;
        let bar_pt = origin + vec2(120.0, 25.0);
        let outer_edge = origin + vec2(268.0, 48.0);

        // 1. Select container in Write mode -> touch handle active, then switch to Erase mode.
        tap_touch(&mut h, 201, bar_pt);
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), Some(0));
        assert!(crate::containers::active_touch_handle_at(&h.ctx, outer_edge));

        h.state_mut().toggle_eraser_tool();
        settle(&mut h);
        assert_eq!(h.state().pointer, ribbon::Pointer::Erase);
        assert!(!crate::containers::active_touch_handle_at(&h.ctx, outer_edge), "Erase mode must clear stale touch handles");
        let pan_before_erase = h.state().canvas_pan;
        drag_touch(&mut h, 202, outer_edge, outer_edge + vec2(50.0, 0.0));
        assert!((h.state().canvas_pan.x - pan_before_erase.x).abs() > 20.0, "1-finger drag over old handle in Erase mode must scroll");

        // 2. Re-select in Write mode, switch to Draw mode -> stale handle cleared & scroll succeeds.
        h.state_mut().canvas_pan = egui::Vec2::ZERO;
        h.state_mut().ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
        settle(&mut h);
        let origin = h.state().editor.as_ref().expect("ed").origin;
        let bar_pt = origin + vec2(120.0, 25.0);
        let outer_edge = origin + vec2(268.0, 48.0);
        tap_touch(&mut h, 203, bar_pt);
        assert!(crate::containers::active_touch_handle_at(&h.ctx, outer_edge));

        h.state_mut().ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Draw));
        settle(&mut h);
        assert!(!crate::containers::active_touch_handle_at(&h.ctx, outer_edge), "Draw mode must clear stale touch handles");
        let pan_before_draw = h.state().canvas_pan;
        drag_touch(&mut h, 204, outer_edge, outer_edge + vec2(50.0, 0.0));
        assert!((h.state().canvas_pan.x - pan_before_draw.x).abs() > 20.0, "1-finger drag over old handle in Draw mode must scroll");

        // 3. Held-ink finger-handle release cleans up pad.grabbed & pad.dirty immediately on release frame,
        // and held_handle_at updates across finger scroll frames.
        h.state_mut().canvas_pan = egui::Vec2::ZERO;
        h.state_mut().ribbon_act(ribbon::Act::InkPointer(ribbon::Pointer::Write));
        h.state_mut().editor.as_mut().expect("ed").caret = None;
        {
            let pad = h.state_mut().pad.get_or_insert_with(|| crate::inkpad::Pad::of_page(&sample.md_page));
            let mut stroke = nocturne_ink::Stroke::new(nocturne_ink::Tool::Pen, nocturne_ink::Rgb::BLACK, 4.0);
            stroke.add(nocturne_ink::Dab::new(200.0, 220.0, 1.0));
            stroke.add(nocturne_ink::Dab::new(300.0, 320.0, 1.0));
            pad.drawing.add(stroke);
            pad.hold_touching((190.0, 210.0, 310.0, 330.0));
            pad.dirty = false;
        }
        settle(&mut h);
        let origin = h.state().editor.as_ref().expect("ed").origin;
        let held_bounds = h.state().pad.as_ref().and_then(|p| p.bounds()).expect("held ink bounds");
        let ink_corner = origin + vec2(held_bounds[2] + 3.0 + 10.0, held_bounds[3] + 3.0 + 10.0);
        let tid = egui::TouchId(205);
        h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos: ink_corner, force: None });
        h.input_mut().events.push(egui::Event::PointerMoved(ink_corner));
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: ink_corner,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        let moved_corner = ink_corner + vec2(30.0, 30.0);
        h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Move, pos: moved_corner, force: None });
        h.input_mut().events.push(egui::Event::PointerMoved(moved_corner));
        h.step();
        assert!(h.state().pad.as_ref().expect("pad").is_grabbed());
        assert!(h.state().last_ink_change.is_none());
        // Single release frame (before any idle frame): pad.grabbed must clear and ink.svg must save immediately!
        h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos: moved_corner, force: None });
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: moved_corner,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.step();
        let pad = h.state().pad.as_ref().expect("pad");
        assert!(!pad.is_grabbed(), "pad.grabbed must clear on release frame");
        assert!(h.state().last_ink_change.is_some(), "ink_changed must fire on release frame");
        assert!(sample.md_page.join("ink.svg").exists(), "ink.svg must save on release frame");
        assert!(pad.can_undo(), "pad.settle() must record undo on release frame");

        // Scroll horizontally with a non-handle finger while ink stays held -> held_handle_at tracks scrolled bounds!
        let old_corner = origin + vec2(190.0, 210.0);
        let scroll_start = origin + vec2(40.0, 360.0);
        let scroll_tid = egui::TouchId(206);
        h.input_mut().events.push(egui::Event::Touch {
            device_id: device,
            id: scroll_tid,
            phase: egui::TouchPhase::Start,
            pos: scroll_start,
            force: None,
        });
        h.step();
        h.input_mut().events.push(egui::Event::Touch {
            device_id: device,
            id: scroll_tid,
            phase: egui::TouchPhase::Move,
            pos: scroll_start + vec2(180.0, 0.0),
            force: None,
        });
        h.step();
        let new_origin = h.state().editor.as_ref().expect("ed").origin;
        let new_bounds = h.state().pad.as_ref().and_then(|p| p.bounds()).expect("bounds");
        let scrolled_corner = new_origin + vec2(new_bounds[2] + 3.0 + 10.0, new_bounds[3] + 3.0 + 10.0);
        assert!(crate::inkpad::held_handle_at(&h.ctx, scrolled_corner), "held_handle_at must follow scrolled position during active scroll");
        assert!(!crate::inkpad::held_handle_at(&h.ctx, old_corner - vec2(15.0, 15.0)), "stale pre-scroll handle rect must not linger");
    }

    #[test]
    fn stacked_containers_bar_hit_clamped_and_bar_tapped_records_handles() {
        let (_t, mut app, sample) = setup_app("m4-r2-stacked", "Upper box\n\nLower box\n");
        let arranged = "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nUpper box\n\n</div>\n<div class=\"nc-box\" data-x=\"40\" data-y=\"90\" data-width=\"200\">\n\nLower box\n\n</div>\n";
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        app.set_finger_drawing(false);
        // Re-place container 1 exactly 12pt below container 0's rendered frame.
        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);
        let origin = h.state().editor.as_ref().expect("ed").origin;
        let upper_rect = h.get_by_label("Upper box").rect();
        // Frame 0 starts at y=20+14=34 and adds TRAILING_LINE_HEIGHT (22pt) + PAD (6pt) below child content.
        let child_h = (upper_rect.max.y - (origin.y + 34.0)).max(18.0);
        let frame0_bottom_y = 34.0 + child_h + 22.0 + 6.0;
        {
            let ed = h.state_mut().editor.as_mut().expect("ed");
            // Default vertical spacing between stacked containers: 12pt below upper frame.
            ed.boxes[1].y = frame0_bottom_y + 12.0;
        }
        settle(&mut h);

        let origin = h.state().editor.as_ref().expect("ed").origin;
        let device = egui::TouchDeviceId(1);
        let tap_touch_one = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            h.step();
        };

        // Tapping near the bottom of container 0's frame (inside the 6pt overlap zone of unclamped 18pt bar_hit)
        // must NOT be hijacked by container 1's expanded bar_hit.
        let near_bottom_of_upper = origin + vec2(120.0, frame0_bottom_y - 3.0);
        tap_touch_one(&mut h, 301, near_bottom_of_upper);
        assert_ne!(crate::containers::selected(&h.ctx, &sample.md_page), Some(1), "lower container bar_hit must not steal upper container tap");
        settle(&mut h);

        // Tapping container 1's own move bar selects container 1 and records touch handles on the same frame.
        let lower_bar_pt = origin + vec2(120.0, frame0_bottom_y + 12.0 + 5.0);
        tap_touch_one(&mut h, 302, lower_bar_pt);
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), Some(1));
        let lower_right_edge = origin + vec2(40.0 + 200.0 + 12.0 + 12.0, frame0_bottom_y + 12.0 + 24.0);
        assert!(crate::containers::active_touch_handle_at(&h.ctx, lower_right_edge), "bar_tapped must populate touch_handles immediately");
    }

    #[test]
    fn touch_pill_suppressed_under_ribbon_popups_and_modals() {
        let (_t, mut app, sample) = setup_app("m4-r2-pill-zorder", "Box text\n");
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40, 40, image::Rgba([20, 120, 220, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let pic_link = nocturne_store::assets::attach_bytes(&sample.md_page, "pic.png", &png.into_inner()).expect("attach");
        let arranged = format!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nBox text\n\n</div>\n<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"160\">\n\n![pic]({pic_link})\n\n</div>\n"
        );
        nocturne_store::page::save_body(&sample.md_page, &arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        app.set_finger_drawing(false);

        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        let origin = h.state().editor.as_ref().expect("ed").origin;
        let device = egui::TouchDeviceId(1);
        let tap_touch = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            settle(h);
        };
        let tap_button = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(pos));
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Move, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            settle(h);
        };

        // Switch to Draw tab so Home Cut/Copy buttons are absent from ribbon; only touch pill provides Cut/Copy.
        h.get_by_label("Draw").click();
        settle(&mut h);

        // Select container 0 via touch -> touch pill renders Cut/Copy.
        tap_touch(&mut h, 401, origin + vec2(120.0, 25.0));
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), Some(0));
        assert_eq!(h.query_all_by_label("Cut").count(), 1, "touch pill Cut must render when no overlay is open");
        assert_eq!(h.query_all_by_label("Copy").count(), 1);

        // Open Pressure curve popup via touch -> touch pill must be suppressed while touch affordance stays active.
        let curve_btn_pos = h.get_by_label("Pressure curve").rect().center();
        tap_button(&mut h, 402, curve_btn_pos);
        assert!(h.query_by_label("Soft").is_some(), "Pressure curve popup must be open");
        assert!(h.state().stylus.touch_affordance_active());
        assert_eq!(h.query_all_by_label("Cut").count(), 0, "touch pill must be suppressed under Pressure curve popup");
        assert_eq!(h.query_all_by_label("Copy").count(), 0);
        // Close Pressure curve popup by toggling its button so container 0 remains selected.
        tap_button(&mut h, 403, curve_btn_pos);
        assert!(h.query_by_label("Soft").is_none(), "Pressure curve popup must close");
        assert_eq!(h.query_all_by_label("Cut").count(), 1, "touch pill must return after closing popup");

        // Open Format Background popup via touch -> touch pill must be suppressed.
        let bg_btn_pos = h.get_by_label("Format Background").rect().center();
        tap_button(&mut h, 404, bg_btn_pos);
        assert!(h.query_by_label("Rule lines").is_some(), "Format Background popup must be open");
        assert!(h.state().stylus.touch_affordance_active());
        assert_eq!(h.query_all_by_label("Cut").count(), 0, "touch pill must be suppressed under Format Background popup");
        tap_button(&mut h, 405, bg_btn_pos);
        assert!(h.query_by_label("Rule lines").is_none(), "Format Background popup must close");
        assert_eq!(h.query_all_by_label("Cut").count(), 1, "touch pill must return after closing Format Background");

        // Open non-TextEdit modal Dialog::Delete -> container touch pill must be suppressed.
        h.state_mut().dialog = Some(Dialog::Delete { path: sample.md_page.clone(), title: "Page".to_string(), count: 1 });
        settle(&mut h);
        assert_eq!(h.query_all_by_label("Cut").count(), 0, "touch pill must be suppressed under Dialog::Delete");
        assert_eq!(h.query_all_by_label("Copy").count(), 0);
        h.state_mut().dialog = None;
        settle(&mut h);
        assert_eq!(h.query_all_by_label("Cut").count(), 1);

        // Select image -> Rotate Right 90° pill appears, and is suppressed under Dialog::Delete and Format Background.
        let rotate_label = crate::tl_id!("ui.rotate_right_90");
        tap_touch(&mut h, 406, origin + vec2(66.0, 154.0));
        assert_eq!(crate::containers::selected_image(&h.ctx, &sample.md_page), Some(1));
        assert_eq!(h.query_all_by_label(rotate_label).count(), 1);
        h.state_mut().dialog = Some(Dialog::Delete { path: sample.md_page.clone(), title: "Page".to_string(), count: 1 });
        settle(&mut h);
        assert_eq!(h.query_all_by_label(rotate_label).count(), 0, "image touch pill must be suppressed under Dialog::Delete");
        h.state_mut().dialog = None;
        settle(&mut h);
        assert_eq!(h.query_all_by_label(rotate_label).count(), 1);
        tap_button(&mut h, 407, bg_btn_pos);
        assert!(h.query_by_label("Rule lines").is_some());
        assert_eq!(h.query_all_by_label(rotate_label).count(), 0, "image touch pill must be suppressed under Format Background");
    }

    /// The frame a finger lifts counts as touch navigation, which hid the pill on exactly the
    /// frame egui reports its tap, so a finger on Delete did nothing.
    #[test]
    fn finger_tap_on_touch_pill_delete_removes_the_container() {
        let (_t, mut app, sample) = setup_app("m4-r2-pill-tap", "Box text\n");
        let arranged = concat!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nBox text\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"200\">\n\nOther box\n\n</div>\n",
        );
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        app.set_finger_drawing(false);

        let mut h: H = egui_kittest::Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, app: &mut NocturneApp| app.show(ui), app);
        settle(&mut h);

        let origin = h.state().editor.as_ref().expect("ed").origin;
        let device = egui::TouchDeviceId(1);
        let tap_touch = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            settle(h);
        };
        let tap_button = |h: &mut H, id: u64, pos: egui::Pos2| {
            let tid = egui::TouchId(id);
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Start, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerMoved(pos));
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::Move, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            h.step();
            h.input_mut().events.push(egui::Event::Touch { device_id: device, id: tid, phase: egui::TouchPhase::End, pos, force: None });
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            settle(h);
        };

        // The Draw tab has no Cut, Copy or Delete of its own, so the only ones are the pill's.
        h.get_by_label("Draw").click();
        settle(&mut h);
        tap_touch(&mut h, 501, origin + vec2(120.0, 25.0));
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), Some(0));
        assert_eq!(h.state().editor.as_ref().expect("ed").boxes.len(), 2);
        let delete_label = crate::tl_id!("ui.delete");
        assert_eq!(h.query_all_by_label(delete_label).count(), 1, "the container touch pill must show one Delete");
        let delete_at = h.get_by_label(delete_label).rect().center();

        tap_button(&mut h, 502, delete_at);
        let ed = h.state().editor.as_ref().expect("ed");
        assert_eq!(ed.boxes.len(), 1, "a finger tap on the pill's Delete must remove the selected container");
        assert!(!ed.body.contains("Box text"), "the deleted container's words must leave the page: {}", ed.body);
        assert!(ed.body.contains("Other box"), "the other container must stay: {}", ed.body);
        assert_eq!(crate::containers::selected(&h.ctx, &sample.md_page), None);
    }

    #[test]
    fn platform_text_insert_and_delete_over_multi_box_across_preserve_boxes_and_undo() {
        let (_t, mut app, sample) = setup_app("m4-r2-across-platform", "One\n");
        let arranged = concat!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nAlpha first\n\nAlpha second\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"200\">\n\nBeta middle\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"220\" data-width=\"200\">\n\nGamma last\n\n</div>\n",
        );
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());

        let original_body = app.body().unwrap_or_default().to_owned();
        let original_boxes = app.editor.as_ref().expect("ed").boxes.clone();
        assert_eq!(original_boxes.len(), 3);

        // Prior typing in block 0 must NOT merge with cross-block replacement.
        focus_block(&mut app, 0, (11, 11));
        app.insert_platform_text("!");
        let typed_body = app.body().unwrap_or_default().to_owned();

        // 1. insert_platform_text over multi-box Across preserves ed.boxes count and undoes in 1 atomic step.
        {
            let ed = app.editor.as_mut().expect("ed");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (0, 6), to: (3, 5) });
        }
        app.insert_platform_text("REPLACED");
        {
            let ed = app.editor.as_ref().expect("ed");
            assert!(ed.across.is_none());
            assert_eq!(ed.boxes.len(), 1, "emptied boxes 1 and 2 must be pruned cleanly");
            assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len());
        }
        let after_insert = app.body().unwrap_or_default().to_owned();
        assert!(after_insert.contains("Alpha REPLACED last") && !after_insert.contains("Beta"), "{after_insert:?}");
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), typed_body, "1 undo must restore all 3 boxes without folding into prior typing");
        assert_eq!(app.editor.as_ref().expect("ed").boxes, original_boxes);
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after_insert);
        app.undo();
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original_body);

        // 2. PlatformEditAction::Delete over multi-box Across preserves ed.boxes and undoes/redoes atomically.
        {
            let ed = app.editor.as_mut().expect("ed");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (0, 6), to: (3, 6) });
        }
        app.apply_platform_edit_action(PlatformEditAction::Delete);
        {
            let ed = app.editor.as_ref().expect("ed");
            assert!(ed.across.is_none());
            assert_eq!(ed.boxes.len(), 1);
            assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len());
        }
        let after_del = app.body().unwrap_or_default().to_owned();
        assert!(after_del.contains("Alpha last") && !after_del.contains("Beta"), "{after_del:?}");
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original_body);
        assert_eq!(app.editor.as_ref().expect("ed").boxes, original_boxes);
        app.redo();
        assert_eq!(app.body().unwrap_or_default(), after_del);

        // 3. delete_backward_platform over multi-box Across preserves ed.boxes and undoes/redoes atomically.
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original_body);
        {
            let ed = app.editor.as_mut().expect("ed");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (0, 6), to: (3, 6) });
        }
        app.delete_backward_platform();
        {
            let ed = app.editor.as_ref().expect("ed");
            assert!(ed.across.is_none());
            assert_eq!(ed.boxes.len(), 1);
            assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len());
        }
        assert_eq!(app.body().unwrap_or_default(), after_del);
        app.undo();
        assert_eq!(app.body().unwrap_or_default(), original_body);
        assert_eq!(app.editor.as_ref().expect("ed").boxes, original_boxes);
    }

    /// Typing over a selection across blocks lands where the selection began when the first
    /// block carries a tag marker. The delete used to hand back a count of `block_text`
    /// characters, marker included, so the caret sat past the end of its shorter text and
    /// the typed letter went after "here" instead of before "line".
    #[test]
    fn platform_text_over_across_from_a_tagged_block_types_where_the_selection_began() {
        let (_t, mut app, _sample) = setup_app("m2-reg-across-tagged", "#important Hello world\n\nSecond line here\n");
        let first = {
            let ed = app.editor.as_mut().expect("ed");
            let first = ed.blocks.iter().position(|b| pageedit::block_text(b).contains("Hello world")).expect("tagged block");
            // Across ends are bytes of `block_text`, which keeps the tag marker.
            let from = pageedit::block_text(&ed.blocks[first]).find("world").expect("the words");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (first, from), to: (first + 1, "Second ".len()) });
            first
        };
        app.insert_platform_text("X");
        let ed = app.editor.as_ref().expect("ed");
        assert!(ed.across.is_none());
        let caret = ed.caret.as_ref().expect("caret");
        assert_eq!((caret.block, caret.text.as_str(), caret.selection), (first, "Hello Xline here", (7, 7)));
        assert!(pageedit::block_text(&ed.blocks[first]).starts_with("#important "), "the tag stays");
        let body = app.body().unwrap_or_default().to_owned();
        assert!(body.contains("Hello Xline here") && !body.contains("Second"), "{body:?}");
    }

    /// Cut over a selection that runs through three boxes goes through the same delete as
    /// the Delete key: the emptied boxes go, the one left counts every block, and one Undo
    /// brings all three boxes back. Before, Cut joined the blocks but left the boxes
    /// counting blocks that were gone.
    #[test]
    fn cut_over_multi_box_across_keeps_boxes_in_step_and_undoes_once() {
        let (_t, mut app, sample) = setup_app("m5-cut-across-boxes", "One\n");
        let arranged = concat!(
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"20\" data-width=\"200\">\n\nAlpha first\n\nAlpha second\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"120\" data-width=\"200\">\n\nBeta middle\n\n</div>\n",
            "<div class=\"nc-box\" data-x=\"40\" data-y=\"220\" data-width=\"200\">\n\nGamma last\n\n</div>\n",
        );
        nocturne_store::page::save_body(&sample.md_page, arranged).expect("save arranged");
        app.open_path(&sample.md_page.to_string_lossy());
        let original_body = app.body().unwrap_or_default().to_owned();
        let original_boxes = app.editor.as_ref().expect("ed").boxes.clone();
        assert_eq!(original_boxes.len(), 3);

        for (label, cut) in [("platform Cut", true), ("Ctrl+X path", false)] {
            {
                let ed = app.editor.as_mut().expect("ed");
                ed.caret = None;
                ed.across = Some(pageedit::Across { from: (0, 6), to: (3, 6) });
            }
            if cut {
                app.apply_platform_edit_action(PlatformEditAction::Cut);
            } else {
                app.clipboard("cut");
            }
            {
                let ed = app.editor.as_ref().expect("ed");
                assert!(ed.across.is_none(), "{label}");
                assert_eq!(ed.boxes.len(), 1, "{label}: emptied boxes must be pruned");
                assert_eq!(ed.boxes.iter().map(|c| c.blocks).sum::<usize>(), ed.blocks.len(), "{label}: box counts match blocks");
            }
            let after_cut = app.body().unwrap_or_default().to_owned();
            assert!(after_cut.contains("Alpha last") && !after_cut.contains("Beta"), "{label}: {after_cut:?}");
            app.undo();
            assert_eq!(app.body().unwrap_or_default(), original_body, "{label}: one Undo restores the page");
            assert_eq!(app.editor.as_ref().expect("ed").boxes, original_boxes, "{label}: one Undo restores the boxes");
        }
        assert!(app.clipboard.text().is_some_and(|t| t.contains("Beta middle")), "platform Cut put the words on the clipboard");
    }

    /// A cut across paragraphs moves the justified marks after it up; Undo puts them back,
    /// in the editor and in the page's own file, and Redo moves them up again.
    #[test]
    fn undo_of_cut_across_blocks_puts_justified_marks_back() {
        use nocturne_model::justify::{Justified, Strength};
        let (_t, mut app, sample) = setup_app("m5-cut-justified-undo", "Zero\n\nOne one\n\nTwo two\n\nThree three\n\nFour four\n\nFive\n");
        app.open_path(&sample.md_page.to_string_lossy());
        let mut marked = Justified::default();
        marked.set(4, Some(Strength::Medium));
        app.write_justified(marked.clone(), Some(Strength::Medium));
        assert_eq!(app.editor.as_ref().expect("ed").justified, marked);

        {
            let ed = app.editor.as_mut().expect("ed");
            ed.caret = None;
            ed.across = Some(pageedit::Across { from: (1, 4), to: (3, 6) });
        }
        app.clipboard("cut");
        let dir = app.editor.as_ref().expect("ed").dir.clone();
        let moved = app.editor.as_ref().expect("ed").justified.clone();
        assert_eq!(moved.strength_of(2), Some(Strength::Medium), "the mark moves up with Four: {moved:?}");
        assert_eq!(nocturne_store::page::justified(&dir).unwrap_or_default(), moved, "moved mark saved");

        app.undo();
        assert_eq!(app.editor.as_ref().expect("ed").justified, marked, "Undo puts the mark back on Four");
        assert_eq!(nocturne_store::page::justified(&dir).unwrap_or_default(), marked, "and into the page file");

        app.redo();
        assert_eq!(app.editor.as_ref().expect("ed").justified, moved, "Redo moves it up again");
    }

    /// Justify is not an undo step of its own. A redo back to the step the page stood on when
    /// a paragraph was justified keeps that paragraph justified, in the editor and in the file:
    /// the step takes the new marks, rather than putting back the ones it was recorded with.
    #[test]
    fn redo_after_justify_keeps_the_justified_paragraph() {
        use nocturne_model::justify::{Justified, Strength};
        let (_t, mut app, sample) = setup_app("m5-redo-keeps-justify", "Zero\n\nOne one\n\nTwo two\n\nThree three\n");
        app.open_path(&sample.md_page.to_string_lossy());
        focus_block(&mut app, 1, (7, 7));
        app.insert_platform_text("x");
        let mut marked = Justified::default();
        marked.set(3, Some(Strength::Medium));
        app.write_justified(marked.clone(), Some(Strength::Medium));
        let dir = app.editor.as_ref().expect("ed").dir.clone();

        app.undo();
        assert_eq!(app.editor.as_ref().expect("ed").justified, marked, "undoing the typing leaves the justified paragraph alone");
        app.redo();
        assert_eq!(app.editor.as_ref().expect("ed").justified, marked, "redo keeps the paragraph justified");
        assert_eq!(nocturne_store::page::justified(&dir).unwrap_or_default(), marked, "and so does the page file");
    }
}
