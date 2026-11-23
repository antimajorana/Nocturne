//! Reusable tablet touch ergonomics, two-finger canvas navigation math, and long-press tracking.

use std::collections::BTreeSet;
use std::time::Duration;

/// Minimum interactive control height (and width) in points for comfortable tablet touch targets.
pub const TABLET_MIN_INTERACT_HEIGHT: f32 = 40.0;

/// Minimum hit-target width/height in points for touch selection and resize handles.
pub const TOUCH_HANDLE_HIT_SIZE: f32 = 32.0;

/// Height in points for floating touch selection action pills.
pub const TOUCH_PILL_HEIGHT: f32 = TABLET_MIN_INTERACT_HEIGHT;

/// Minimum canvas zoom percentage shared across pinch-zoom, toolbar zoom, and wheel zoom.
pub const MIN_ZOOM_PERCENT: u32 = 15;

/// Maximum canvas zoom percentage shared across pinch-zoom, toolbar zoom, and wheel zoom.
pub const MAX_ZOOM_PERCENT: u32 = 250;

/// Duration in seconds a stationary finger must stay down to trigger a touch long press.
pub const LONG_PRESS_SECS: f64 = 0.50;

/// Maximum finger movement in points allowed before cancelling a pending touch long press.
pub const LONG_PRESS_MOVE_TOLERANCE_PTS: f32 = 10.0;

/// How far in points a finger may drift between touch-down and lift and still count as a tap.
/// egui's default of 6 suits a mouse; a fingertip rolls further than that on glass, and the
/// long press above already allows 10.
pub const TOUCH_CLICK_SLOP_PTS: f32 = 10.0;

/// Widens egui's click tolerance for fingers without narrowing a larger custom value, so a tap
/// that drifts a few points still presses the button under it. Note that egui also uses
/// `max_click_dist` as its click-and-drag start threshold and double-tap distance.
pub fn apply_mobile_touch_input_options(options: &mut egui::Options) {
    options.input_options.max_click_dist = options.input_options.max_click_dist.max(TOUCH_CLICK_SLOP_PTS);
}

/// Raises spacing minimums for finger-friendly touch targets without shrinking larger custom values.
pub fn apply_mobile_touch_style(style: &mut egui::Style) {
    style.spacing.interact_size.x = style.spacing.interact_size.x.max(TABLET_MIN_INTERACT_HEIGHT);
    style.spacing.interact_size.y = style.spacing.interact_size.y.max(TABLET_MIN_INTERACT_HEIGHT);
    style.spacing.button_padding.x = style.spacing.button_padding.x.max(8.0);
    style.spacing.button_padding.y = style.spacing.button_padding.y.max(6.0);
    style.spacing.item_spacing.y = style.spacing.item_spacing.y.max(6.0);
    style.spacing.scroll.bar_width = style.spacing.scroll.bar_width.max(10.0);
    // A finger does not hover. A tooltip on a touch screen appears under the finger that is
    // pressing the button and stays there. The delay is long, not infinite: egui makes a
    // Duration of it.
    style.interaction.tooltip_delay = style.interaction.tooltip_delay.max(NO_HOVER_TOOLTIP_DELAY_SECS);
}

/// Input state for [`mobile_formatting_bar`], describing which docked formatting and quick-switch
/// buttons are enabled or active for the current caret/selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MobileFormattingBarState {
    pub can_outdent: bool,
    pub can_indent: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_format: bool,
    pub bold_active: bool,
    pub italic_active: bool,
    pub underline_active: bool,
    pub can_block: bool,
    pub bullets_active: bool,
    pub todo_active: bool,
    pub can_quick_pen: bool,
    pub pen_active: bool,
}

/// Action returned when the user taps a control on [`mobile_formatting_bar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobileFormattingAction {
    Outdent,
    Indent,
    Undo,
    Redo,
    Bold,
    Italic,
    Underline,
    Bullets,
    Todo,
    QuickPen,
}

impl MobileFormattingAction {
    /// Converts this action into the matching [`crate::ribbon::Act`].
    /// `active_nib` is the current pen rack index used by [`Self::QuickPen`].
    #[must_use]
    pub fn into_ribbon_act(self, active_nib: usize) -> crate::ribbon::Act {
        match self {
            Self::Outdent => crate::ribbon::Act::Indent(false),
            Self::Indent => crate::ribbon::Act::Indent(true),
            Self::Undo => crate::ribbon::Act::Undo,
            Self::Redo => crate::ribbon::Act::Redo,
            Self::Bold => crate::ribbon::Act::Mark("bold"),
            Self::Italic => crate::ribbon::Act::Mark("italic"),
            Self::Underline => crate::ribbon::Act::Mark("underline"),
            Self::Bullets => crate::ribbon::Act::Block("bullet"),
            Self::Todo => crate::ribbon::Act::Block("todo"),
            Self::QuickPen => crate::ribbon::Act::InkPen(active_nib),
        }
    }

    /// True when this action formats text and the editor should keep caret focus/selection
    /// active after applying it. [`Self::QuickPen`] switches into drawing instead.
    #[must_use]
    pub fn restores_text_caret(self) -> bool {
        !matches!(self, Self::QuickPen)
    }
}

/// Renders the docked mobile/tablet formatting bar above the soft keyboard with 40pt touch
/// targets and returns whichever action was tapped this frame.
pub fn mobile_formatting_bar(ui: &mut egui::Ui, state: MobileFormattingBarState, tokens: &crate::theme::Tokens) -> Option<MobileFormattingAction> {
    use crate::floating::Glyph;
    use crate::icons::Icon;
    use crate::ribbon_icons::Mark;
    use crate::theme::{RADIUS_POPUP, Tokens};
    use egui::{CornerRadius, Sense, Stroke, StrokeKind, vec2};

    let mut chosen: Option<MobileFormattingAction> = None;
    egui::Panel::bottom("mobile-formatting-bar")
        .frame(egui::Frame::new().fill(tokens.surface_elevated).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, tokens.border)))
        .show_separator_line(false)
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("mobile-formatting-scroll")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let items: [(&str, Glyph, bool, bool, MobileFormattingAction); 10] = [
                            (
                                crate::tl_id!("ui.decrease_indentation"),
                                Glyph::Mark(Mark::IndentLess),
                                state.can_outdent,
                                false,
                                MobileFormattingAction::Outdent,
                            ),
                            (
                                crate::tl_id!("ui.increase_indentation"),
                                Glyph::Mark(Mark::IndentMore),
                                state.can_indent,
                                false,
                                MobileFormattingAction::Indent,
                            ),
                            (crate::tl_id!("ui.undo"), Glyph::Icon(Icon::Undo), state.can_undo, false, MobileFormattingAction::Undo),
                            (crate::tl_id!("ui.redo"), Glyph::Icon(Icon::Redo), state.can_redo, false, MobileFormattingAction::Redo),
                            (crate::tl_id!("ui.bold"), Glyph::Mark(Mark::Bold), state.can_format, state.bold_active, MobileFormattingAction::Bold),
                            (
                                crate::tl_id!("ui.italic"),
                                Glyph::Mark(Mark::Italic),
                                state.can_format,
                                state.italic_active,
                                MobileFormattingAction::Italic,
                            ),
                            (
                                crate::tl_id!("ui.underline"),
                                Glyph::Mark(Mark::Underline),
                                state.can_format,
                                state.underline_active,
                                MobileFormattingAction::Underline,
                            ),
                            (
                                crate::tl_id!("ui.bullets"),
                                Glyph::Icon(Icon::ListBullet),
                                state.can_block,
                                state.bullets_active,
                                MobileFormattingAction::Bullets,
                            ),
                            (crate::tl_id!("ui.to_do_2"), Glyph::Mark(Mark::ToDo), state.can_block, state.todo_active, MobileFormattingAction::Todo),
                            (
                                crate::tl_id!("ui.pen"),
                                Glyph::Icon(Icon::Pen),
                                state.can_quick_pen,
                                state.pen_active,
                                MobileFormattingAction::QuickPen,
                            ),
                        ];
                        for (label, glyph, enabled, active, action) in items {
                            let resp = ui
                                .add_enabled_ui(enabled, |ui| {
                                    let size = vec2(46.0, TABLET_MIN_INTERACT_HEIGHT);
                                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                                    let painter = ui.painter_at(rect);
                                    let pressed = enabled && resp.is_pointer_button_down_on();
                                    let r = CornerRadius::same(RADIUS_POPUP);
                                    if pressed {
                                        painter.rect_filled(rect, r, tokens.row_selected);
                                        painter.rect_stroke(rect, r, Stroke::new(1.5, tokens.accent), StrokeKind::Inside);
                                    } else if active {
                                        painter.rect_filled(rect, r, tokens.row_selected);
                                        painter.rect_stroke(rect, r, Stroke::new(1.0, tokens.accent), StrokeKind::Inside);
                                    } else if resp.hovered() && enabled {
                                        painter.rect_filled(rect, r, tokens.row_hover);
                                        painter.rect_stroke(rect, r, Stroke::new(1.0, tokens.border_subtle), StrokeKind::Inside);
                                    }
                                    if enabled && resp.has_focus() {
                                        ui.painter().rect_stroke(
                                            rect.expand(1.5),
                                            CornerRadius::same(RADIUS_POPUP + 2),
                                            Stroke::new(2.0, tokens.accent),
                                            StrokeKind::Outside,
                                        );
                                    }
                                    let icon_tokens = if !enabled {
                                        Tokens { text: tokens.disabled, accent: tokens.disabled, ..*tokens }
                                    } else if active || pressed {
                                        Tokens { text: tokens.accent, ..*tokens }
                                    } else {
                                        *tokens
                                    };
                                    let icon_rect = egui::Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
                                    glyph.paint(&painter, icon_rect, &icon_tokens);
                                    let name = label.to_string();
                                    resp.widget_info(move || egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, name.clone()));
                                    resp.on_hover_text(label)
                                })
                                .inner;
                            if enabled && resp.clicked() {
                                chosen = Some(action);
                            }
                        }
                    });
                });
        });
    chosen
}

/// A tooltip delay long enough that no tooltip is ever shown.
pub const NO_HOVER_TOOLTIP_DELAY_SECS: f32 = 1.0e6;

/// Where a pointer is sent to let go of a press without clicking what is under the finger.
pub const OFFSCREEN_RELEASE_POS: egui::Pos2 = egui::pos2(-10_000.0, -10_000.0);

/// The pressure given to a pen that reports none.
pub const PENCIL_FALLBACK_PRESSURE: f32 = 0.5;

/// Gives a pen that reports no usable pressure a middle one.
///
/// On an iPad a touch carries a force only when it is a Pencil (a finger carries none), and the
/// window layer reports it as force over the most the pen can report. A Pencil with no pressure
/// sensor reports 0 over 0, which is not a number. Left alone, that reading is thrown away and
/// the pen is taken for a finger. Call this on the iPad's events before anything reads them.
pub fn normalize_pencil_force(events: &mut [egui::Event]) {
    for event in events {
        if let egui::Event::Touch { force: Some(force), .. } = event
            && !force.is_finite()
        {
            *force = PENCIL_FALLBACK_PRESSURE;
        }
    }
}

/// Per-frame two-finger pan, vertical scroll, and optional clamped zoom update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultiTouchStep {
    pub scroll_y: f32,
    pub pan_x: f32,
    pub new_zoom: Option<u32>,
}

/// Converts multi-touch translation and pinch deltas into finite canvas pan/scroll and clamped zoom.
pub fn compute_multitouch_canvas_step(translation_delta: egui::Vec2, zoom_delta: f32, current_zoom_pct: u32) -> MultiTouchStep {
    let pan_x = if translation_delta.x.is_finite() { translation_delta.x } else { 0.0 };
    let scroll_y = if translation_delta.y.is_finite() { translation_delta.y } else { 0.0 };
    let new_zoom = if zoom_delta.is_finite() && zoom_delta > 0.0 && (zoom_delta - 1.0).abs() > 0.015 {
        let next = ((f64::from(current_zoom_pct) * f64::from(zoom_delta)).round() as i64)
            .clamp(i64::from(MIN_ZOOM_PERCENT), i64::from(MAX_ZOOM_PERCENT)) as u32;
        (next != current_zoom_pct).then_some(next)
    } else {
        None
    };
    MultiTouchStep { scroll_y, pan_x, new_zoom }
}

#[derive(Clone, Copy, Debug)]
struct PendingLongPress {
    id: egui::TouchId,
    start_pos: egui::Pos2,
    start_time: f64,
    fired: bool,
}

/// Tracks active touch contacts to recognize a stationary single-finger long press.
#[derive(Clone, Debug, Default)]
pub struct TouchLongPress {
    active_touches: BTreeSet<egui::TouchId>,
    pending: Option<PendingLongPress>,
    /// The finger whose long press became a context click, until it lifts.
    consumed: Option<egui::TouchId>,
}

impl TouchLongPress {
    /// Updates touch state for this frame and returns the press position once when `LONG_PRESS_SECS` elapses.
    pub fn update(&mut self, now_secs: f64, events: &[egui::Event]) -> Option<egui::Pos2> {
        for event in events {
            let egui::Event::Touch { id, phase, pos, force, .. } = event else { continue };
            match phase {
                egui::TouchPhase::Start => {
                    self.active_touches.insert(*id);
                    if force.is_some() || self.active_touches.len() > 1 || !now_secs.is_finite() || !pos.is_finite() {
                        self.pending = None;
                    } else {
                        self.pending = Some(PendingLongPress { id: *id, start_pos: *pos, start_time: now_secs, fired: false });
                    }
                }
                egui::TouchPhase::Move => {
                    if let Some(pending) = self.pending {
                        let dist = pos.distance(pending.start_pos);
                        let within_tolerance = dist.is_finite() && dist <= LONG_PRESS_MOVE_TOLERANCE_PTS;
                        if force.is_some() || *id != pending.id || self.active_touches.len() != 1 || !within_tolerance {
                            self.pending = None;
                        }
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    self.active_touches.remove(id);
                    if self.pending.is_some_and(|pending| pending.id == *id) {
                        self.pending = None;
                    }
                }
            }
        }
        let pending = self.pending.as_mut()?;
        if !pending.fired && self.active_touches.len() == 1 && now_secs.is_finite() && now_secs - pending.start_time >= LONG_PRESS_SECS {
            pending.fired = true;
            return Some(pending.start_pos);
        }
        None
    }

    /// Turns the long press that [`Self::update`] just reported at `pos` into a context click,
    /// and uses the finger up: see [`Self::filter_consumed`].
    ///
    /// The finger is still on the glass, so the window still holds its primary press. That press
    /// is let go first, off screen so it clicks nothing. A secondary click made while the primary
    /// button is down leaves egui with a press that has no start time, and the finger's lift
    /// then counts as a click: it closed the menu this opens.
    pub fn consume_as_context_click(&mut self, events: &mut Vec<egui::Event>, pos: egui::Pos2) {
        let button = |button, pressed, pos| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
        events.extend([
            egui::Event::PointerMoved(OFFSCREEN_RELEASE_POS),
            button(egui::PointerButton::Primary, false, OFFSCREEN_RELEASE_POS),
            egui::Event::PointerMoved(pos),
            button(egui::PointerButton::Secondary, true, pos),
            button(egui::PointerButton::Secondary, false, pos),
        ]);
        self.consumed = self.pending.map(|pending| pending.id);
    }

    /// Drops what is left of a finger that [`Self::consume_as_context_click`] used up. Call it on
    /// each frame's events before [`Self::update`].
    ///
    /// Until that finger lifts, its moves and the pointer events the window makes from them are
    /// dropped, and its lift arrives as a cancelled touch: the contact is over, and it was not a
    /// tap on the page. Other contacts are left alone.
    pub fn filter_consumed(&mut self, events: &mut Vec<egui::Event>) {
        let Some(consumed) = self.consumed else { return };
        let mut lifted = false;
        events.retain_mut(|event| match event {
            egui::Event::Touch { id, phase, .. } if *id == consumed => match phase {
                egui::TouchPhase::Start | egui::TouchPhase::Move => false,
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    *phase = egui::TouchPhase::Cancel;
                    lifted = true;
                    true
                }
            },
            egui::Event::PointerMoved(_) | egui::Event::PointerButton { button: egui::PointerButton::Primary, .. } => false,
            _ => true,
        });
        if lifted {
            self.consumed = None;
        }
    }

    /// Returns the delay until the armed long press should be re-evaluated, or `None` when idle or fired.
    pub fn next_wakeup_delay(&self, now_secs: f64) -> Option<Duration> {
        let pending = self.pending?;
        if pending.fired || self.active_touches.len() != 1 || !now_secs.is_finite() {
            return None;
        }
        let remaining = (LONG_PRESS_SECS - (now_secs - pending.start_time)).max(0.016);
        // Clamp to a finite positive upper bound so malformed timestamps can never panic Duration::from_secs_f64.
        remaining.is_finite().then(|| Duration::from_secs_f64(remaining.min(86_400.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::TouchPhase::{Cancel, End, Move, Start};
    use egui::{Event, Pos2, TouchDeviceId, TouchId, TouchPhase, Vec2, pos2, vec2};

    fn touch(id: u64, phase: TouchPhase, pos: Pos2, force: Option<f32>) -> Event {
        Event::Touch { device_id: TouchDeviceId(0), id: TouchId(id), phase, pos, force }
    }

    fn custom_style(interact: Vec2, padding: Vec2, spacing: Vec2, bar_width: f32) -> egui::Style {
        let mut style = egui::Style::default();
        style.spacing.interact_size = interact;
        style.spacing.button_padding = padding;
        style.spacing.item_spacing = spacing;
        style.spacing.scroll.bar_width = bar_width;
        style
    }

    /// Replays what egui-winit sends for one finger tap: the touch plus the emulated pointer.
    fn drifting_tap_clicks(configure: impl FnOnce(&mut egui::Options), drift: f32) -> bool {
        let ctx = egui::Context::default();
        ctx.options_mut(configure);
        let at = pos2(60.0, 40.0);
        let end = at + vec2(drift, 0.0);
        let button =
            |pressed: bool, pos: Pos2| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let frame = |time: f64, events: Vec<Event>| {
            let mut clicked = false;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                clicked = ui.put(egui::Rect::from_center_size(at, vec2(120.0, 60.0)), egui::Button::new("Tap")).clicked();
            });
            out.textures_delta.clear();
            clicked
        };
        assert!(!frame(0.0, Vec::new()), "layout frame");
        assert!(!frame(0.02, vec![touch(1, Start, at, None), Event::PointerMoved(at), button(true, at)]));
        assert!(!frame(0.06, vec![touch(1, Move, end, None), Event::PointerMoved(end)]));
        frame(0.10, vec![touch(1, End, end, None), button(false, end), Event::PointerGone])
    }

    #[test]
    fn finger_tap_that_drifts_a_few_points_still_clicks_with_mobile_input_options() {
        // egui's mouse default: a tap that rolls 8 points is thrown away.
        assert!(drifting_tap_clicks(|_| {}, 3.0));
        assert!(!drifting_tap_clicks(|_| {}, 8.0), "egui default max_click_dist is 6");

        assert!(drifting_tap_clicks(apply_mobile_touch_input_options, 8.0), "an 8 point drift is still a tap on a tablet");
        assert!(!drifting_tap_clicks(apply_mobile_touch_input_options, 14.0), "a real drag is still not a click");

        // A larger custom value is not narrowed.
        let mut options = egui::Options::default();
        options.input_options.max_click_dist = 16.0;
        apply_mobile_touch_input_options(&mut options);
        assert_eq!(options.input_options.max_click_dist, 16.0);
    }

    fn button(primary: bool, pressed: bool, pos: Pos2) -> Event {
        let button = if primary { egui::PointerButton::Primary } else { egui::PointerButton::Secondary };
        Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn long_press_context_click_lets_go_of_the_finger_first_and_eats_its_lift() {
        let mut lp = TouchLongPress::default();
        let p = pos2(120.0, 240.0);
        let mut down = vec![touch(1, Start, p, None), Event::PointerMoved(p), button(true, true, p)];
        lp.filter_consumed(&mut down);
        assert_eq!(down.len(), 3, "nothing is consumed before the long press fires");
        assert_eq!(lp.update(0.0, &down), None);

        let mut held: Vec<Event> = Vec::new();
        lp.filter_consumed(&mut held);
        let at = lp.update(0.55, &held).expect("long press fires");
        lp.consume_as_context_click(&mut held, at);
        assert_eq!(
            held,
            vec![
                Event::PointerMoved(OFFSCREEN_RELEASE_POS),
                button(true, false, OFFSCREEN_RELEASE_POS),
                Event::PointerMoved(p),
                button(false, true, p),
                button(false, false, p),
            ],
            "the primary button is let go off screen before the secondary click"
        );

        // The finger wobbles, then lifts: none of it reaches egui as a pointer, and the touch ends
        // as a cancel, so the page does not read it as a tap.
        let mut wobble = vec![touch(1, Move, p + vec2(2.0, 1.0), None), Event::PointerMoved(p + vec2(2.0, 1.0))];
        lp.filter_consumed(&mut wobble);
        assert_eq!(wobble, Vec::new());
        let mut lift = vec![touch(1, End, p, None), button(true, false, p), Event::PointerGone];
        lp.filter_consumed(&mut lift);
        assert_eq!(lift, vec![touch(1, Cancel, p, None), Event::PointerGone]);
        assert_eq!(lp.update(0.9, &lift), None);

        // The next tap is an ordinary tap again.
        let mut tap = vec![touch(2, Start, p, None), Event::PointerMoved(p), button(true, true, p)];
        lp.filter_consumed(&mut tap);
        assert_eq!(tap.len(), 3);
    }

    #[test]
    fn pencil_without_pressure_gets_the_fallback_and_fingers_stay_fingers() {
        let p = pos2(10.0, 20.0);
        let mut events =
            vec![touch(1, Start, p, Some(f32::NAN)), touch(1, Move, p, Some(f32::INFINITY)), touch(2, Start, p, None), touch(3, Move, p, Some(0.3))];
        normalize_pencil_force(&mut events);
        assert_eq!(
            events,
            vec![
                touch(1, Start, p, Some(PENCIL_FALLBACK_PRESSURE)),
                touch(1, Move, p, Some(PENCIL_FALLBACK_PRESSURE)),
                touch(2, Start, p, None),
                touch(3, Move, p, Some(0.3)),
            ]
        );
    }

    #[test]
    fn mobile_touch_style_never_shows_hover_tooltips() {
        let mut style = egui::Style::default();
        apply_mobile_touch_style(&mut style);
        assert!(style.interaction.tooltip_delay >= 1.0e5, "a finger does not hover: {}", style.interaction.tooltip_delay);
        assert!(style.interaction.tooltip_delay.is_finite(), "egui turns the delay into a Duration");
    }

    #[test]
    fn mobile_touch_style_raises_minimums_without_shrinking_larger_values() {
        let mut small = custom_style(vec2(20.0, 18.0), vec2(4.0, 2.0), vec2(5.0, 3.0), 6.0);
        apply_mobile_touch_style(&mut small);
        assert_eq!(small.spacing.interact_size, vec2(TABLET_MIN_INTERACT_HEIGHT, TABLET_MIN_INTERACT_HEIGHT));
        assert_eq!(small.spacing.button_padding, vec2(8.0, 6.0));
        assert_eq!(small.spacing.item_spacing, vec2(5.0, 6.0));
        assert_eq!(small.spacing.scroll.bar_width, 10.0);

        let mut large = custom_style(vec2(48.0, 44.0), vec2(14.0, 10.0), vec2(12.0, 9.0), 16.0);
        apply_mobile_touch_style(&mut large);
        assert_eq!(large.spacing.interact_size, vec2(48.0, 44.0));
        assert_eq!(large.spacing.button_padding, vec2(14.0, 10.0));
        assert_eq!(large.spacing.item_spacing, vec2(12.0, 9.0));
        assert_eq!(large.spacing.scroll.bar_width, 16.0);
    }

    #[test]
    fn multitouch_canvas_step_zoom_deadzone_and_clamps() {
        let step = compute_multitouch_canvas_step;
        assert_eq!(step(Vec2::ZERO, 1.01, 100).new_zoom, None);
        assert_eq!(step(Vec2::ZERO, 0.99, 100).new_zoom, None);
        assert_eq!(step(Vec2::ZERO, 1.5, 220).new_zoom, Some(MAX_ZOOM_PERCENT));
        assert_eq!(step(Vec2::ZERO, 1.5, MAX_ZOOM_PERCENT).new_zoom, None);
        assert_eq!(step(Vec2::ZERO, 0.5, 20).new_zoom, Some(MIN_ZOOM_PERCENT));
        assert_eq!(step(Vec2::ZERO, 0.5, MIN_ZOOM_PERCENT).new_zoom, None);
    }

    #[test]
    fn multitouch_canvas_step_preserves_pan_scroll_and_guards_non_finite() {
        let step = compute_multitouch_canvas_step;
        assert_eq!(step(vec2(12.5, -24.0), 1.25, 100), MultiTouchStep { scroll_y: -24.0, pan_x: 12.5, new_zoom: Some(125) });
        assert_eq!(step(vec2(f32::NAN, f32::INFINITY), f32::NAN, 100), MultiTouchStep { scroll_y: 0.0, pan_x: 0.0, new_zoom: None });
        assert_eq!(step(vec2(f32::NAN, 8.0), -2.0, 100), MultiTouchStep { scroll_y: 8.0, pan_x: 0.0, new_zoom: None });
    }

    #[test]
    fn long_press_stationary_finger_fires_once() {
        let mut lp = TouchLongPress::default();
        let p = pos2(120.0, 240.0);
        assert_eq!(lp.update(0.0, &[touch(1, Start, p, None)]), None);
        assert_eq!(lp.update(0.40, &[]), None);
        assert_eq!(lp.update(0.50, &[]), Some(p));
        assert_eq!(lp.update(0.80, &[]), None);
        assert_eq!(lp.update(0.90, &[touch(1, End, p, None)]), None);
        assert_eq!(lp.update(1.50, &[]), None);
    }

    #[test]
    fn long_press_allows_small_jitter_and_cancels_past_tolerance() {
        let mut lp = TouchLongPress::default();
        let start = pos2(50.0, 50.0);
        assert_eq!(lp.update(0.0, &[touch(1, Start, start, None)]), None);
        assert_eq!(lp.update(0.20, &[touch(1, Move, pos2(56.0, 57.0), None)]), None);
        assert_eq!(lp.update(0.50, &[]), Some(start));
        assert_eq!(lp.update(0.60, &[touch(1, End, start, None)]), None);
        assert_eq!(lp.update(1.0, &[touch(2, Start, start, None)]), None);
        assert_eq!(lp.update(1.2, &[touch(2, Move, pos2(60.5, 50.0), None)]), None);
        assert_eq!(lp.update(1.7, &[]), None);
    }

    #[test]
    fn long_press_cancels_on_end_or_cancel_before_threshold() {
        let mut lp = TouchLongPress::default();
        let p = pos2(80.0, 80.0);
        assert_eq!(lp.update(0.0, &[touch(1, Start, p, None)]), None);
        assert_eq!(lp.update(0.30, &[touch(1, End, p, None)]), None);
        assert_eq!(lp.update(0.60, &[]), None);
        assert_eq!(lp.update(1.0, &[touch(2, Start, p, None)]), None);
        assert_eq!(lp.update(1.2, &[touch(2, Cancel, p, None)]), None);
        assert_eq!(lp.update(1.7, &[]), None);
    }

    #[test]
    fn long_press_ignores_apple_pencil_with_force() {
        let mut lp = TouchLongPress::default();
        let p = pos2(10.0, 20.0);
        assert_eq!(lp.update(0.0, &[touch(1, Start, p, Some(0.5))]), None);
        assert_eq!(lp.next_wakeup_delay(0.20), None);
        assert_eq!(lp.update(0.50, &[]), None);
        assert_eq!(lp.update(0.60, &[touch(1, End, p, Some(0.0))]), None);
        assert_eq!(lp.update(1.0, &[touch(2, Start, p, None)]), None);
        assert_eq!(lp.update(1.1, &[touch(2, Move, p, Some(0.3))]), None);
        assert_eq!(lp.update(1.6, &[]), None);
    }

    #[test]
    fn long_press_cancels_when_second_finger_arrives() {
        let mut lp = TouchLongPress::default();
        assert_eq!(lp.update(0.0, &[touch(1, Start, pos2(10.0, 10.0), None)]), None);
        assert_eq!(lp.update(0.20, &[touch(2, Start, pos2(40.0, 10.0), None)]), None);
        assert_eq!(lp.next_wakeup_delay(0.25), None);
        assert_eq!(lp.update(0.30, &[touch(2, End, pos2(40.0, 10.0), None)]), None);
        assert_eq!(lp.update(0.80, &[]), None);
    }

    #[test]
    fn long_press_next_wakeup_delay_tracks_remaining_wait() {
        let mut lp = TouchLongPress::default();
        assert_eq!(lp.next_wakeup_delay(1.0), None);
        let p = pos2(10.0, 10.0);
        assert_eq!(lp.update(1.0, &[touch(1, Start, p, None)]), None);
        assert!(lp.next_wakeup_delay(1.10).is_some_and(|wait| (wait.as_secs_f64() - 0.40).abs() < 1e-6));
        assert_eq!(lp.next_wakeup_delay(1.495), Some(Duration::from_secs_f64(0.016)));
        assert_eq!(lp.next_wakeup_delay(f64::NAN), None);
        assert_eq!(lp.update(1.50, &[]), Some(p));
        assert_eq!(lp.next_wakeup_delay(1.50), None);
    }

    #[test]
    fn tablet_min_interact_height_is_40pt() {
        assert!((TABLET_MIN_INTERACT_HEIGHT - 40.0).abs() < f32::EPSILON);
        assert!((TOUCH_PILL_HEIGHT - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mobile_formatting_action_maps_to_ribbon_acts_and_caret_policy() {
        use crate::ribbon::Act;
        assert_eq!(MobileFormattingAction::Bold.into_ribbon_act(2), Act::Mark("bold"));
        assert_eq!(MobileFormattingAction::Italic.into_ribbon_act(2), Act::Mark("italic"));
        assert_eq!(MobileFormattingAction::Underline.into_ribbon_act(2), Act::Mark("underline"));
        assert_eq!(MobileFormattingAction::Bullets.into_ribbon_act(2), Act::Block("bullet"));
        assert_eq!(MobileFormattingAction::Todo.into_ribbon_act(2), Act::Block("todo"));
        assert_eq!(MobileFormattingAction::Outdent.into_ribbon_act(2), Act::Indent(false));
        assert_eq!(MobileFormattingAction::Indent.into_ribbon_act(2), Act::Indent(true));
        assert_eq!(MobileFormattingAction::Undo.into_ribbon_act(2), Act::Undo);
        assert_eq!(MobileFormattingAction::Redo.into_ribbon_act(2), Act::Redo);
        assert_eq!(MobileFormattingAction::QuickPen.into_ribbon_act(2), Act::InkPen(2));

        assert!(MobileFormattingAction::Italic.restores_text_caret());
        assert!(MobileFormattingAction::Underline.restores_text_caret());
        assert!(MobileFormattingAction::Bullets.restores_text_caret());
        assert!(!MobileFormattingAction::QuickPen.restores_text_caret());
    }

    #[test]
    fn mobile_formatting_bar_exposes_italic_underline_bullets_and_quick_pen_at_40pt() {
        let ctx = egui::Context::default();
        let tokens = crate::theme::Theme::DEFAULT.tokens();
        let state = MobileFormattingBarState {
            can_outdent: true,
            can_indent: true,
            can_undo: true,
            can_redo: true,
            can_format: true,
            can_block: true,
            can_quick_pen: true,
            ..Default::default()
        };
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 200.0));
        let run = |events: Vec<Event>| {
            let mut got = None;
            let raw = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                got = mobile_formatting_bar(ui, state, &tokens);
            });
            out.textures_delta.clear();
            got
        };
        assert_eq!(run(Vec::new()), None);
        // 10 buttons of width 46 + 6px spacing inside 8px left margin, bottom-aligned at height 40pt.
        let bar_y = screen.max.y - 4.0 - TABLET_MIN_INTERACT_HEIGHT * 0.5;
        let center_of = |idx: usize| pos2(8.0 + (idx as f32) * (46.0 + 6.0) + 23.0, bar_y);
        for (idx, expected) in [
            (5usize, MobileFormattingAction::Italic),
            (6, MobileFormattingAction::Underline),
            (7, MobileFormattingAction::Bullets),
            (9, MobileFormattingAction::QuickPen),
        ] {
            let at = center_of(idx);
            let _ = run(vec![Event::PointerMoved(at), button(true, true, at)]);
            let fired = run(vec![button(true, false, at)]);
            assert_eq!(fired, Some(expected), "tapping slot {idx} should fire {expected:?}");
        }
    }
}
