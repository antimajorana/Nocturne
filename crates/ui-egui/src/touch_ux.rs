//! Reusable tablet touch ergonomics, two-finger canvas navigation math, and long-press tracking.

use std::collections::BTreeSet;
use std::time::Duration;

/// Minimum interactive control height (and width) in points for comfortable tablet touch targets.
pub const TABLET_MIN_INTERACT_HEIGHT: f32 = 36.0;

/// Minimum hit-target width/height in points for touch selection and resize handles.
pub const TOUCH_HANDLE_HIT_SIZE: f32 = 32.0;

/// Height in points for floating touch selection action pills.
pub const TOUCH_PILL_HEIGHT: f32 = TABLET_MIN_INTERACT_HEIGHT;

/// Minimum canvas zoom percentage shared across pinch-zoom, toolbar zoom, and wheel zoom.
pub const MIN_ZOOM_PERCENT: u32 = 50;

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
        assert_eq!(step(Vec2::ZERO, 0.5, 60).new_zoom, Some(MIN_ZOOM_PERCENT));
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
}
