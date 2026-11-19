//! iPadOS finger-only pan & pinch-zoom gesture bridge + `RawInput` touch/pointer filter.
//!
//! Pure-Rust gating, delta/scale math, and `RawInput` event filtering live at module top level so
//! they compile and run under `cargo test -p nocturne-ios` on Windows and macOS hosts. The UIKit
//! `UIPanGestureRecognizer` / `UIPinchGestureRecognizer` bridge lives inside `#[cfg(target_os = "ios")]`.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Minimum single-step pinch scale ratio accepted from UIKit before clamping.
const MIN_PINCH_STEP_SCALE: f32 = 0.2;
/// Maximum single-step pinch scale ratio accepted from UIKit before clamping.
const MAX_PINCH_STEP_SCALE: f32 = 5.0;
/// Maximum accumulated pan delta magnitude per axis to match `NocturneApp::apply_external_pan`.
const MAX_PAN_ACCUM_PTS: f32 = 1.0e6;
/// Offscreen coordinate used when synthesizing a release for a cancelled/suppressed touch pointer
/// so `egui::PointerState` clears `down[Primary]` without firing a spurious click.
pub const OFFSCREEN_RELEASE_POS: [f32; 2] = [-10_000.0, -10_000.0];

/// Axis-aligned canvas hit-test rectangle in logical UIKit / egui points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl GestureRect {
    /// Creates a `GestureRect` from `[min_x, min_y, max_x, max_y]`.
    pub fn from_bounds(bounds: [f32; 4]) -> Self {
        Self { min_x: bounds[0], min_y: bounds[1], max_x: bounds[2], max_y: bounds[3] }
    }

    /// Returns `true` when `(x, y)` and all bounds are finite and `(x, y)` lies inside the rect.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x.is_finite()
            && y.is_finite()
            && self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
            && x >= self.min_x
            && x <= self.max_x
            && y >= self.min_y
            && y <= self.max_y
    }
}

/// Shared state between UIKit pan/pinch recognizers and the per-frame `eframe` shell hook.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GestureBridgeState {
    pub finger_drawing: bool,
    pub canvas_rect: Option<GestureRect>,
    pub egui_using_pointer: bool,
    pub pan_active: bool,
    pub pinch_active: bool,
    pub pending_pan_delta: [f32; 2],
    pub pending_pinch: Option<(f32, Option<[f32; 2]>)>,
    pub needs_pointer_release: bool,
    pub known_stylus_ids: BTreeSet<u64>,
}

impl GestureBridgeState {
    /// Creates an idle `GestureBridgeState`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` while either the native finger pan or native pinch recognizer is active.
    pub fn is_native_gesture_active(&self) -> bool {
        self.pan_active || self.pinch_active
    }

    /// Returns `true` when `(x, y)` lies inside the current canvas rect.
    pub fn point_in_canvas(&self, x: f32, y: f32) -> bool {
        self.canvas_rect.is_some_and(|rect| rect.contains(x, y))
    }

    /// Accumulates an incremental pan delta in logical points.
    pub fn accumulate_pan(&mut self, dx: f32, dy: f32) {
        let [inc_x, inc_y] = compute_incremental_pan(dx, dy);
        if inc_x == 0.0 && inc_y == 0.0 {
            return;
        }
        let next_x = self.pending_pan_delta[0] + inc_x;
        let next_y = self.pending_pan_delta[1] + inc_y;
        if next_x.is_finite() && next_y.is_finite() {
            self.pending_pan_delta = [next_x.clamp(-MAX_PAN_ACCUM_PTS, MAX_PAN_ACCUM_PTS), next_y.clamp(-MAX_PAN_ACCUM_PTS, MAX_PAN_ACCUM_PTS)];
        }
    }

    /// Accumulates a multiplicative incremental pinch scale and latest focal point.
    pub fn accumulate_pinch(&mut self, scale: f32, focal: Option<[f32; 2]>) {
        let Some((step_scale, clean_focal)) = compute_incremental_pinch(scale, focal) else {
            return;
        };
        self.pending_pinch = Some(match self.pending_pinch {
            Some((prev_scale, prev_focal)) => {
                let combined = (prev_scale * step_scale).clamp(MIN_PINCH_STEP_SCALE, MAX_PINCH_STEP_SCALE);
                let safe_scale = if combined.is_finite() && combined > 0.0 { combined } else { step_scale };
                (safe_scale, clean_focal.or(prev_focal))
            }
            None => (step_scale, clean_focal),
        });
    }

    /// Drains any accumulated finite non-zero pan delta.
    pub fn drain_pan_delta(&mut self) -> Option<[f32; 2]> {
        let delta = self.pending_pan_delta;
        self.pending_pan_delta = [0.0, 0.0];
        (delta[0].is_finite() && delta[1].is_finite() && (delta[0] != 0.0 || delta[1] != 0.0)).then_some(delta)
    }

    /// Drains any accumulated pinch scale factor and latest focal point.
    pub fn drain_pinch(&mut self) -> Option<(f32, Option<[f32; 2]>)> {
        self.pending_pinch.take()
    }
}

/// Shared handle type used by UIKit delegates and `GestureBridge`.
pub type SharedGestureState = Rc<RefCell<GestureBridgeState>>;

/// Returns `true` when the native finger-only `UIPanGestureRecognizer` should begin.
pub fn should_begin_native_pan(touch_count: usize, finger_drawing: bool, in_canvas: bool, egui_using_pointer: bool) -> bool {
    !egui_using_pointer && in_canvas && if finger_drawing { touch_count >= 2 } else { (1..=2).contains(&touch_count) }
}

/// Returns `true` when the native `UIPinchGestureRecognizer` should begin.
pub fn should_begin_native_pinch(touch_count: usize, in_canvas: bool, egui_using_pointer: bool) -> bool {
    !egui_using_pointer && in_canvas && touch_count >= 2
}

/// Extracts the most recent finite touch or pointer position from an `egui::RawInput` event batch.
#[must_use]
pub fn latest_pointer_pos_in_events(events: &[egui::Event]) -> Option<egui::Pos2> {
    events.iter().rev().find_map(|ev| match ev {
        egui::Event::Touch { pos, .. } | egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } if pos.is_finite() => Some(*pos),
        _ => None,
    })
}

/// Returns `true` when an egui popup, modal, dialog window (`Order::Middle`+), foreground overlay,
/// source view, or active touch resize/ink handle owns the pointer inside the canvas, so native
/// pan/pinch recognizers should yield.
///
/// Unlike `ctx.egui_is_using_pointer()`, this does NOT fire for `potential_click_id` or
/// `potential_drag_id` from normal `Sense::click()` / `Sense::click_and_drag()` canvas widgets
/// (`open-paper`, `ink-layer`, `ink-overlay`, text blocks), allowing 1-finger pan and 2-finger
/// pan/pinch to begin.
#[must_use]
pub fn egui_overlay_blocks_native_gesture(ctx: &egui::Context, show_source: bool, touch_pos: Option<egui::Pos2>) -> bool {
    if show_source || ctx.any_popup_open() || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return true;
    }
    let pos = touch_pos.or_else(|| ctx.pointer_interact_pos()).or_else(|| ctx.pointer_latest_pos());
    if let Some(pos) = pos.filter(|p| p.is_finite()) {
        if ctx.layer_id_at(pos).is_some_and(|layer| layer.order >= egui::Order::Middle) {
            return true;
        }
        if nocturne_ui_egui::containers::active_touch_handle_at(ctx, pos) || nocturne_ui_egui::inkpad::held_handle_at(ctx, pos) {
            return true;
        }
    }
    false
}

/// Returns `true` when a long-press at `pos` lands inside the open page canvas and no modal,
/// dialog, backstage screen, source view, popup, or foreground overlay (`Order::Middle`+) owns
/// `pos`.
///
/// Outside the open page canvas (modal confirmation buttons such as Delete notebook, top bar,
/// ribbon, backstage, drawer), synthetic `PointerButton::Secondary` injection must be suppressed:
/// in `egui 0.37`, any `PointerButton { pressed: false }` clears `PointerState::press_origin`
/// while the finger is still down, canceling deliberate taps (>= 0.50 s), whereas egui's built-in
/// `LONG_TOUCHED` flag already opens `.context_menu(...)` on `Sense::click()` widgets.
#[must_use]
pub fn should_inject_canvas_long_press(ctx: &egui::Context, page_rect: Option<egui::Rect>, canvas_overlay_open: bool, pos: egui::Pos2) -> bool {
    if canvas_overlay_open || !pos.is_finite() {
        return false;
    }
    let Some(rect) = page_rect else {
        return false;
    };
    if !rect.contains(pos) {
        return false;
    }
    !egui_overlay_blocks_native_gesture(ctx, false, Some(pos))
}

/// Validates an incremental pan delta in logical points matching `NocturneApp::apply_external_pan`.
///
/// Positive `(dx, dy)` moves the canvas/scroll in the direction of the finger drag (natural scrolling),
/// and any non-finite input collapses to `[0.0, 0.0]`.
pub fn compute_incremental_pan(dx: f32, dy: f32) -> [f32; 2] {
    if dx.is_finite() && dy.is_finite() { [dx, dy] } else { [0.0, 0.0] }
}

/// Validates and clamps an incremental pinch scale factor and optional focal point.
///
/// Non-finite or non-positive scales return `None`; non-finite focal coordinates sanitize to `None`.
pub fn compute_incremental_pinch(scale: f32, focal: Option<[f32; 2]>) -> Option<(f32, Option<[f32; 2]>)> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let clamped = scale.clamp(MIN_PINCH_STEP_SCALE, MAX_PINCH_STEP_SCALE);
    let clean_focal = focal.filter(|[x, y]| x.is_finite() && y.is_finite());
    Some((clamped, clean_focal))
}

/// Pure-Rust touch phase mirroring `egui::TouchPhase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PureTouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

/// Pure-Rust input event mirroring the `egui::Event` variants inspected by the gesture filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PureInputEvent {
    Touch { device_id: u64, id: u64, phase: PureTouchPhase, pos: [f32; 2], force: Option<f32> },
    PointerMoved([f32; 2]),
    PointerButton { pos: [f32; 2], primary: bool, pressed: bool },
    PointerGone,
    Zoom(f32),
    Other,
}

enum EventKind {
    Touch { id: u64, phase: PureTouchPhase, is_stylus_force: bool, pos: [f32; 2] },
    PointerMoved([f32; 2]),
    PointerButton { primary: bool, pressed: bool, pos: [f32; 2] },
    PointerGone,
    Zoom,
    Other,
}

fn pos_matches(a: [f32; 2], b: [f32; 2]) -> bool {
    a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite() && (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

struct ReleaseEventFns<M, R, G> {
    pointer_moved: M,
    primary_release: R,
    pointer_gone: G,
}

fn filter_events_core<E, M, R, G>(
    events: &mut Vec<E>,
    state: &mut GestureBridgeState,
    pointer_was_down: bool,
    stylus_active_hint: bool,
    active_stylus_ids: &BTreeSet<u64>,
    classify: impl Fn(&E) -> EventKind,
    release_fns: ReleaseEventFns<M, R, G>,
) where
    M: Fn([f32; 2]) -> E,
    R: Fn([f32; 2]) -> E,
    G: Fn() -> E,
{
    let native_active = state.is_native_gesture_active();

    for ev in events.iter() {
        if let EventKind::Touch { id, phase: PureTouchPhase::Start | PureTouchPhase::Move, is_stylus_force: true, .. } = classify(ev) {
            state.known_stylus_ids.insert(id);
        }
    }

    let mut frame_stylus_ids = state.known_stylus_ids.clone();
    frame_stylus_ids.extend(active_stylus_ids.iter().copied());
    let stylus_active = stylus_active_hint || !frame_stylus_ids.is_empty();

    let mut frame_has_stylus_touch = false;
    let mut stylus_positions = Vec::new();
    let mut ended_stylus_ids = Vec::new();
    for ev in events.iter() {
        if let EventKind::Touch { id, phase, is_stylus_force, pos } = classify(ev) {
            let is_stylus = is_stylus_force || frame_stylus_ids.contains(&id);
            if is_stylus {
                frame_has_stylus_touch = true;
                stylus_positions.push(pos);
                if matches!(phase, PureTouchPhase::End | PureTouchPhase::Cancel) {
                    ended_stylus_ids.push(id);
                }
            }
        }
    }
    let should_suppress_pointer = |pos: [f32; 2]| -> bool {
        if !native_active {
            return false;
        }
        // While native finger pan/pinch is active, suppress all pointer events unless they match
        // a stylus touch position in this frame.
        !stylus_positions.iter().any(|&p| pos_matches(p, pos))
    };

    let mut filtered = Vec::with_capacity(events.len() + 3);
    let mut saw_non_stylus_cancel = false;
    let mut suppressed_non_stylus_pointer = false;
    let mut kept_non_stylus_primary_press = false;
    let mut already_released_in_frame = false;

    for ev in events.drain(..) {
        match classify(&ev) {
            EventKind::Touch { id, is_stylus_force, .. } => {
                let is_stylus = is_stylus_force || frame_stylus_ids.contains(&id);
                if is_stylus {
                    // Apple Pencil (`force: Some(_)` or known stylus `TouchId`) must never be suppressed or delayed.
                    filtered.push(ev);
                } else if let EventKind::Touch { phase, .. } = classify(&ev) {
                    let is_cancel = matches!(phase, PureTouchPhase::Cancel);
                    let is_release = matches!(phase, PureTouchPhase::End | PureTouchPhase::Cancel);
                    if is_release {
                        if is_cancel {
                            saw_non_stylus_cancel = true;
                        }
                        // Keep both `TouchPhase::End` and `TouchPhase::Cancel` so `TouchLongPress`
                        // and `egui::TouchState` never leak active touch contacts.
                        filtered.push(ev);
                    } else if !native_active {
                        filtered.push(ev);
                    }
                }
            }
            EventKind::PointerMoved(pos) => {
                if should_suppress_pointer(pos) {
                    suppressed_non_stylus_pointer = true;
                } else {
                    filtered.push(ev);
                }
            }
            EventKind::PointerButton { primary: true, pressed, pos } => {
                if should_suppress_pointer(pos) {
                    suppressed_non_stylus_pointer = true;
                } else {
                    if pressed {
                        if !stylus_active && !frame_has_stylus_touch {
                            kept_non_stylus_primary_press = true;
                            already_released_in_frame = false;
                        }
                    } else {
                        already_released_in_frame = true;
                    }
                    filtered.push(ev);
                }
            }
            EventKind::PointerButton { primary: false, .. } | EventKind::PointerGone | EventKind::Other => {
                filtered.push(ev);
            }
            EventKind::Zoom => {
                if !state.pinch_active {
                    filtered.push(ev);
                }
            }
        }
    }

    let release_triggered = saw_non_stylus_cancel || suppressed_non_stylus_pointer || state.needs_pointer_release;
    let had_down_pointer = pointer_was_down || kept_non_stylus_primary_press;
    let stylus_blocking_release = stylus_active || frame_has_stylus_touch;
    let mut synthesized_release = false;
    if release_triggered && had_down_pointer && !stylus_blocking_release && !already_released_in_frame {
        filtered.push((release_fns.pointer_moved)(OFFSCREEN_RELEASE_POS));
        filtered.push((release_fns.primary_release)(OFFSCREEN_RELEASE_POS));
        filtered.push((release_fns.pointer_gone)());
        synthesized_release = true;
    }
    if synthesized_release || already_released_in_frame || !had_down_pointer {
        state.needs_pointer_release = false;
    } else if release_triggered && had_down_pointer && stylus_blocking_release {
        state.needs_pointer_release = true;
    }

    for id in ended_stylus_ids {
        state.known_stylus_ids.remove(&id);
    }

    *events = filtered;
}

fn classify_pure_event(ev: &PureInputEvent) -> EventKind {
    match ev {
        PureInputEvent::Touch { id, phase, pos, force, .. } => {
            EventKind::Touch { id: *id, phase: *phase, is_stylus_force: force.is_some_and(f32::is_finite), pos: *pos }
        }
        PureInputEvent::PointerMoved(pos) => EventKind::PointerMoved(*pos),
        PureInputEvent::PointerButton { pos, primary, pressed } => EventKind::PointerButton { primary: *primary, pressed: *pressed, pos: *pos },
        PureInputEvent::PointerGone => EventKind::PointerGone,
        PureInputEvent::Zoom(_) => EventKind::Zoom,
        PureInputEvent::Other => EventKind::Other,
    }
}

/// Filters pure-Rust input events for active native pan/pinch gestures and stuck-pointer recovery.
pub fn filter_pure_events(events: &mut Vec<PureInputEvent>, state: &mut GestureBridgeState, pointer_was_down: bool, stylus_active: bool) {
    let empty_ids = BTreeSet::new();
    filter_events_core(
        events,
        state,
        pointer_was_down,
        stylus_active,
        &empty_ids,
        classify_pure_event,
        ReleaseEventFns {
            pointer_moved: PureInputEvent::PointerMoved,
            primary_release: |pos| PureInputEvent::PointerButton { pos, primary: true, pressed: false },
            pointer_gone: || PureInputEvent::PointerGone,
        },
    );
}

/// Filters pure-Rust input events using an explicit set of active stylus `TouchId`s (captured
/// before removing `End`/`Cancel` for the current frame).
pub fn filter_pure_events_with_stylus_ids(
    events: &mut Vec<PureInputEvent>,
    state: &mut GestureBridgeState,
    pointer_was_down: bool,
    active_stylus_ids: &BTreeSet<u64>,
) {
    filter_events_core(
        events,
        state,
        pointer_was_down,
        !active_stylus_ids.is_empty(),
        active_stylus_ids,
        classify_pure_event,
        ReleaseEventFns {
            pointer_moved: PureInputEvent::PointerMoved,
            primary_release: |pos| PureInputEvent::PointerButton { pos, primary: true, pressed: false },
            pointer_gone: || PureInputEvent::PointerGone,
        },
    );
}

#[cfg(target_os = "ios")]
pub mod ios {
    use super::{
        EventKind, GestureBridgeState, GestureRect, PureTouchPhase, ReleaseEventFns, SharedGestureState, filter_events_core, should_begin_native_pan,
        should_begin_native_pinch,
    };
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_core_foundation::CGPoint;
    use objc2_foundation::{NSArray, NSNumber};
    use objc2_ui_kit::{
        UIGestureRecognizer, UIGestureRecognizerDelegate, UIGestureRecognizerState, UIPanGestureRecognizer, UIPinchGestureRecognizer, UITouch,
        UITouchType, UIView,
    };
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::rc::Rc;

    #[allow(unsafe_code)]
    // SAFETY: `NocturneGestureDelegate` subclasses `NSObject` with `MainThreadOnly` thread kind,
    // has no custom `Drop`, and only exposes UIKit target-action and `UIGestureRecognizerDelegate` methods.
    mod delegate_class {
        use super::*;

        pub(super) struct GestureDelegateIvars {
            state: SharedGestureState,
            egui_ctx: egui::Context,
            pan_addr: Cell<usize>,
            pinch_addr: Cell<usize>,
        }

        define_class!(
            #[unsafe(super(NSObject))]
            #[thread_kind = MainThreadOnly]
            #[ivars = GestureDelegateIvars]
            pub(super) struct NocturneGestureDelegate;

            impl NocturneGestureDelegate {
                #[unsafe(method(handlePan:))]
                fn __handle_pan(&self, recognizer: &UIPanGestureRecognizer) {
                    let Ok(mut state) = self.ivars().state.try_borrow_mut() else {
                        return;
                    };
                    match recognizer.state() {
                        UIGestureRecognizerState::Began => {
                            state.pan_active = true;
                            state.needs_pointer_release = true;
                            let view = recognizer.view();
                            let delta = recognizer.translationInView(view.as_deref());
                            state.accumulate_pan(delta.x as f32, delta.y as f32);
                            recognizer.setTranslation_inView(CGPoint::ZERO, view.as_deref());
                        }
                        UIGestureRecognizerState::Changed => {
                            state.pan_active = true;
                            let view = recognizer.view();
                            let delta = recognizer.translationInView(view.as_deref());
                            state.accumulate_pan(delta.x as f32, delta.y as f32);
                            recognizer.setTranslation_inView(CGPoint::ZERO, view.as_deref());
                        }
                        UIGestureRecognizerState::Ended | UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => {
                            state.pan_active = false;
                        }
                        _ => {}
                    }
                    drop(state);
                    // The recognizer cancels its touches out of `WinitView`, so no winit event follows to
                    // run the frame that drains this pan; request it here once the borrow is released.
                    self.ivars().egui_ctx.request_repaint();
                }

                #[unsafe(method(handlePinch:))]
                fn __handle_pinch(&self, recognizer: &UIPinchGestureRecognizer) {
                    let Ok(mut state) = self.ivars().state.try_borrow_mut() else {
                        return;
                    };
                    match recognizer.state() {
                        UIGestureRecognizerState::Began => {
                            state.pinch_active = true;
                            state.needs_pointer_release = true;
                            let view = recognizer.view();
                            let loc = recognizer.locationInView(view.as_deref());
                            state.accumulate_pinch(recognizer.scale() as f32, Some([loc.x as f32, loc.y as f32]));
                            recognizer.setScale(1.0);
                        }
                        UIGestureRecognizerState::Changed => {
                            state.pinch_active = true;
                            let view = recognizer.view();
                            let loc = recognizer.locationInView(view.as_deref());
                            state.accumulate_pinch(recognizer.scale() as f32, Some([loc.x as f32, loc.y as f32]));
                            recognizer.setScale(1.0);
                        }
                        UIGestureRecognizerState::Ended | UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => {
                            state.pinch_active = false;
                        }
                        _ => {}
                    }
                    drop(state);
                    // The recognizer cancels its touches out of `WinitView`, so no winit event follows to
                    // run the frame that drains this pinch; request it here once the borrow is released.
                    self.ivars().egui_ctx.request_repaint();
                }
            }

            unsafe impl NSObjectProtocol for NocturneGestureDelegate {}

            unsafe impl UIGestureRecognizerDelegate for NocturneGestureDelegate {
                #[unsafe(method(gestureRecognizer:shouldReceiveTouch:))]
                fn __should_receive_touch(&self, _gesture_recognizer: &UIGestureRecognizer, touch: &UITouch) -> bool {
                    touch.r#type() == UITouchType::Direct
                }

                #[unsafe(method(gestureRecognizerShouldBegin:))]
                fn __gesture_recognizer_should_begin(&self, gesture_recognizer: &UIGestureRecognizer) -> bool {
                    self.should_begin(gesture_recognizer)
                }

                #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
                fn __should_recognize_simultaneously(
                    &self,
                    gesture_recognizer: &UIGestureRecognizer,
                    other_gesture_recognizer: &UIGestureRecognizer,
                ) -> bool {
                    (self.is_pan(gesture_recognizer) && self.is_pinch(other_gesture_recognizer))
                        || (self.is_pinch(gesture_recognizer) && self.is_pan(other_gesture_recognizer))
                }
            }
        );

        impl NocturneGestureDelegate {
            pub(super) fn new(mtm: MainThreadMarker, state: SharedGestureState, egui_ctx: egui::Context) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(GestureDelegateIvars { state, egui_ctx, pan_addr: Cell::new(0), pinch_addr: Cell::new(0) });
                // SAFETY: `NSObject::init` is the designated initializer of superclass `NSObject`,
                // and `MainThreadMarker` guarantees execution on the UIKit main thread.
                unsafe { msg_send![super(this), init] }
            }

            pub(super) fn set_recognizer_addrs(&self, pan_addr: usize, pinch_addr: usize) {
                self.ivars().pan_addr.set(pan_addr);
                self.ivars().pinch_addr.set(pinch_addr);
            }

            fn should_begin(&self, gesture_recognizer: &UIGestureRecognizer) -> bool {
                let Ok(state) = self.ivars().state.try_borrow() else {
                    return false;
                };
                let view = gesture_recognizer.view();
                let loc = gesture_recognizer.locationInView(view.as_deref());
                let in_canvas = state.point_in_canvas(loc.x as f32, loc.y as f32);
                let touch_count = gesture_recognizer.numberOfTouches();
                if self.is_pan(gesture_recognizer) {
                    should_begin_native_pan(touch_count, state.finger_drawing, in_canvas, state.egui_using_pointer)
                } else if self.is_pinch(gesture_recognizer) {
                    should_begin_native_pinch(touch_count, in_canvas, state.egui_using_pointer)
                } else {
                    false
                }
            }

            fn is_pan(&self, recognizer: &UIGestureRecognizer) -> bool {
                let addr = std::ptr::from_ref(recognizer) as usize;
                addr != 0 && addr == self.ivars().pan_addr.get()
            }

            fn is_pinch(&self, recognizer: &UIGestureRecognizer) -> bool {
                let addr = std::ptr::from_ref(recognizer) as usize;
                addr != 0 && addr == self.ivars().pinch_addr.get()
            }
        }
    }

    use delegate_class::NocturneGestureDelegate;

    /// Owns native UIKit finger-only pan & pinch gesture recognizers attached to `WinitView`.
    pub struct GestureBridge {
        pan: Retained<UIPanGestureRecognizer>,
        pinch: Retained<UIPinchGestureRecognizer>,
        _delegate: Retained<NocturneGestureDelegate>,
        state: SharedGestureState,
    }

    impl GestureBridge {
        /// Attaches finger-only `UIPanGestureRecognizer` and `UIPinchGestureRecognizer` onto `view`.
        pub fn attach(view: &UIView, mtm: MainThreadMarker, egui_ctx: &egui::Context) -> Self {
            let state = Rc::new(RefCell::new(GestureBridgeState::new()));
            let delegate = NocturneGestureDelegate::new(mtm, Rc::clone(&state), egui_ctx.clone());
            let delegate_ns: &NSObject = delegate.as_ref();
            let target_obj: &AnyObject = delegate_ns.as_ref();

            let pan = {
                let alloc = mtm.alloc::<UIPanGestureRecognizer>();
                #[allow(unsafe_code)]
                // SAFETY: `target_obj` is a retained `NocturneGestureDelegate` implementing `handlePan:`
                // with signature `fn(&self, &UIPanGestureRecognizer)` on the main thread.
                unsafe {
                    UIPanGestureRecognizer::initWithTarget_action(alloc, Some(target_obj), Some(sel!(handlePan:)))
                }
            };
            let pinch = {
                let alloc = mtm.alloc::<UIPinchGestureRecognizer>();
                #[allow(unsafe_code)]
                // SAFETY: `target_obj` is a retained `NocturneGestureDelegate` implementing `handlePinch:`
                // with signature `fn(&self, &UIPinchGestureRecognizer)` on the main thread.
                unsafe {
                    UIPinchGestureRecognizer::initWithTarget_action(alloc, Some(target_obj), Some(sel!(handlePinch:)))
                }
            };

            delegate.set_recognizer_addrs(Retained::as_ptr(&pan) as usize, Retained::as_ptr(&pinch) as usize);

            let direct_touch = NSNumber::new_isize(UITouchType::Direct.0);
            let allowed_types = NSArray::from_slice(&[&*direct_touch]);
            let delegate_proto = ProtocolObject::from_ref(&*delegate);

            pan.setAllowedTouchTypes(&allowed_types);
            pan.setDelaysTouchesBegan(false);
            pan.setCancelsTouchesInView(true);
            pan.setMinimumNumberOfTouches(1);
            pan.setMaximumNumberOfTouches(2);
            pan.setDelegate(Some(delegate_proto));

            pinch.setAllowedTouchTypes(&allowed_types);
            pinch.setDelaysTouchesBegan(false);
            pinch.setCancelsTouchesInView(true);
            pinch.setDelegate(Some(delegate_proto));

            view.addGestureRecognizer(&pan);
            view.addGestureRecognizer(&pinch);

            Self { pan, pinch, _delegate: delegate, state }
        }

        /// Synchronizes `finger_drawing`, canvas rect bounds, and egui pointer ownership each frame.
        pub fn sync_from_app(&self, finger_drawing: bool, canvas_rect: Option<egui::Rect>, egui_using_pointer: bool) {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.finger_drawing = finger_drawing;
                state.canvas_rect =
                    canvas_rect.filter(|r| r.is_finite()).map(|r| GestureRect { min_x: r.min.x, min_y: r.min.y, max_x: r.max.x, max_y: r.max.y });
                state.egui_using_pointer = egui_using_pointer;
            }
            let min_touches = if finger_drawing { 2 } else { 1 };
            if self.pan.minimumNumberOfTouches() != min_touches {
                self.pan.setMinimumNumberOfTouches(min_touches);
            }
        }

        /// Drains any accumulated pan delta in logical points.
        pub fn drain_pan_delta(&self) -> Option<egui::Vec2> {
            let mut state = self.state.try_borrow_mut().ok()?;
            state.drain_pan_delta().map(|[dx, dy]| egui::vec2(dx, dy))
        }

        /// Drains any accumulated pinch zoom factor and optional focal position in logical points.
        pub fn drain_pinch(&self) -> Option<(f32, Option<egui::Pos2>)> {
            let mut state = self.state.try_borrow_mut().ok()?;
            state.drain_pinch().map(|(scale, focal)| (scale, focal.map(|[x, y]| egui::pos2(x, y))))
        }

        /// Filters `raw_input.events` while native pan/pinch is active and synthesizes clean pointer release
        /// on `TouchPhase::Cancel` or suppressed pointer events without touching Apple Pencil events.
        pub fn filter_raw_input(&self, raw_input: &mut egui::RawInput, pointer_was_down: bool, active_stylus_ids: &BTreeSet<egui::TouchId>) {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                return;
            };
            let pure_ids: BTreeSet<u64> = active_stylus_ids.iter().map(|id| id.0).collect();
            filter_events_core(
                &mut raw_input.events,
                &mut state,
                pointer_was_down,
                !pure_ids.is_empty(),
                &pure_ids,
                |ev| match ev {
                    egui::Event::Touch { id, phase, pos, force, .. } => EventKind::Touch {
                        id: id.0,
                        phase: match phase {
                            egui::TouchPhase::Start => PureTouchPhase::Start,
                            egui::TouchPhase::Move => PureTouchPhase::Move,
                            egui::TouchPhase::End => PureTouchPhase::End,
                            egui::TouchPhase::Cancel => PureTouchPhase::Cancel,
                        },
                        is_stylus_force: force.is_some_and(f32::is_finite),
                        pos: [pos.x, pos.y],
                    },
                    egui::Event::PointerMoved(pos) => EventKind::PointerMoved([pos.x, pos.y]),
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        EventKind::PointerButton { primary: *button == egui::PointerButton::Primary, pressed: *pressed, pos: [pos.x, pos.y] }
                    }
                    egui::Event::PointerGone => EventKind::PointerGone,
                    egui::Event::Zoom(_) => EventKind::Zoom,
                    _ => EventKind::Other,
                },
                ReleaseEventFns {
                    pointer_moved: |[x, y]: [f32; 2]| egui::Event::PointerMoved(egui::pos2(x, y)),
                    primary_release: |[x, y]: [f32; 2]| egui::Event::PointerButton {
                        pos: egui::pos2(x, y),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    pointer_gone: || egui::Event::PointerGone,
                },
            );
        }

        /// Returns `true` when either native pan or native pinch is currently active.
        pub fn is_native_gesture_active(&self) -> bool {
            self.state.try_borrow().map(|s| s.is_native_gesture_active()).unwrap_or(false)
        }
    }

    impl Drop for GestureBridge {
        fn drop(&mut self) {
            self.pan.setDelegate(None);
            self.pinch.setDelegate(None);
            #[allow(unsafe_code)]
            // SAFETY: Clearing all targets/actions (`None, None`) on live recognizers on the main thread
            // prevents UIKit from sending actions after `self._delegate` is dropped.
            unsafe {
                self.pan.removeTarget_action(None, None);
                self.pinch.removeTarget_action(None, None);
            }
            if let Some(view) = self.pan.view() {
                view.removeGestureRecognizer(&self.pan);
            }
            if let Some(view) = self.pinch.view() {
                view.removeGestureRecognizer(&self.pinch);
            }
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::GestureBridge;

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_event(id: u64, phase: PureTouchPhase, pos: [f32; 2], force: Option<f32>) -> PureInputEvent {
        PureInputEvent::Touch { device_id: 0, id, phase, pos, force }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 1e-5, "expected {expected}, got {actual}");
    }

    #[test]
    fn gesture_rect_contains_checks_bounds_and_finiteness() {
        let rect = GestureRect { min_x: 10.0, min_y: 20.0, max_x: 110.0, max_y: 220.0 };
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(110.0, 220.0));
        assert!(rect.contains(50.0, 100.0));
        assert!(!rect.contains(9.9, 100.0));
        assert!(!rect.contains(50.0, 220.1));
        assert!(!rect.contains(f32::NAN, 100.0));
        assert!(!rect.contains(50.0, f32::INFINITY));

        let non_finite = GestureRect::from_bounds([f32::NAN, 0.0, 100.0, 100.0]);
        assert!(!non_finite.contains(50.0, 50.0));
    }

    #[test]
    fn pan_gating_respects_finger_drawing_touch_count_canvas_and_egui_pointer() {
        // Finger drawing OFF: 1-2 touches inside canvas begin when egui doesn't own pointer.
        assert!(!should_begin_native_pan(0, false, true, false));
        assert!(should_begin_native_pan(1, false, true, false));
        assert!(should_begin_native_pan(2, false, true, false));
        assert!(!should_begin_native_pan(3, false, true, false));

        // Finger drawing ON: 1-finger yields to drawing; 2-finger pan still begins.
        assert!(!should_begin_native_pan(1, true, true, false));
        assert!(should_begin_native_pan(2, true, true, false));

        // Outside canvas or egui already using pointer: never begins.
        assert!(!should_begin_native_pan(1, false, false, false));
        assert!(!should_begin_native_pan(2, true, false, false));
        assert!(!should_begin_native_pan(1, false, true, true));
        assert!(!should_begin_native_pan(2, true, true, true));
    }

    #[test]
    fn pinch_gating_requires_two_touches_in_canvas_and_idle_egui_pointer() {
        assert!(!should_begin_native_pinch(0, true, false));
        assert!(!should_begin_native_pinch(1, true, false));
        assert!(should_begin_native_pinch(2, true, false));
        assert!(!should_begin_native_pinch(2, false, false));
        assert!(!should_begin_native_pinch(2, true, true));
    }

    #[test]
    fn incremental_pan_preserves_sign_and_filters_non_finite() {
        assert_eq!(compute_incremental_pan(14.5, -32.25), [14.5, -32.25]);
        assert_eq!(compute_incremental_pan(f32::NAN, 10.0), [0.0, 0.0]);
        assert_eq!(compute_incremental_pan(10.0, f32::INFINITY), [0.0, 0.0]);
        assert_eq!(compute_incremental_pan(f32::NEG_INFINITY, f32::NAN), [0.0, 0.0]);

        let mut state = GestureBridgeState::new();
        state.accumulate_pan(5.0, -3.0);
        state.accumulate_pan(2.5, -7.0);
        state.accumulate_pan(f32::NAN, 100.0);
        assert_eq!(state.drain_pan_delta(), Some([7.5, -10.0]));
        assert_eq!(state.drain_pan_delta(), None);
    }

    #[test]
    fn incremental_pinch_validates_clamps_and_accumulates_multiplicatively() {
        assert_eq!(compute_incremental_pinch(0.0, Some([50.0, 60.0])), None);
        assert_eq!(compute_incremental_pinch(-1.5, Some([50.0, 60.0])), None);
        assert_eq!(compute_incremental_pinch(f32::NAN, Some([50.0, 60.0])), None);
        assert_eq!(compute_incremental_pinch(f32::INFINITY, Some([50.0, 60.0])), None);

        assert_eq!(compute_incremental_pinch(100.0, Some([50.0, 60.0])), Some((MAX_PINCH_STEP_SCALE, Some([50.0, 60.0]))));
        assert_eq!(compute_incremental_pinch(0.01, Some([f32::NAN, 60.0])), Some((MIN_PINCH_STEP_SCALE, None)));

        let mut state = GestureBridgeState::new();
        state.accumulate_pinch(1.2, Some([100.0, 200.0]));
        state.accumulate_pinch(1.25, Some([110.0, 210.0]));
        state.accumulate_pinch(f32::NAN, Some([999.0, 999.0]));
        let drained = state.drain_pinch();
        assert!(drained.is_some());
        if let Some((scale, focal)) = drained {
            assert_close(scale, 1.5);
            assert_eq!(focal, Some([110.0, 210.0]));
        }
        assert_eq!(state.drain_pinch(), None);
    }

    #[test]
    fn filter_passes_pencil_through_during_active_pan_while_suppressing_finger_events() {
        let mut state = GestureBridgeState { pan_active: true, ..Default::default() };
        let pencil_start = touch_event(99, PureTouchPhase::Start, [150.0, 260.0], Some(0.45));
        let pencil_move = touch_event(99, PureTouchPhase::Move, [154.0, 265.0], Some(0.50));
        let pencil_pointer = PureInputEvent::PointerMoved([154.0, 265.0]);
        let mut events = vec![
            touch_event(1, PureTouchPhase::Move, [20.0, 30.0], None),
            PureInputEvent::PointerMoved([20.0, 30.0]),
            PureInputEvent::PointerButton { pos: [20.0, 30.0], primary: true, pressed: true },
            pencil_start,
            pencil_move,
            pencil_pointer,
            PureInputEvent::Zoom(1.1),
        ];

        // Even with pan active, Pencil touches/pointer and frame zoom (pinch_active == false) pass through,
        // while finger touch and finger-emulated pointer events are suppressed.
        filter_pure_events(&mut events, &mut state, false, false);
        assert_eq!(events, vec![pencil_start, pencil_move, pencil_pointer, PureInputEvent::Zoom(1.1)]);
    }

    #[test]
    fn filter_suppresses_zoom_only_when_native_pinch_active() {
        let mut state = GestureBridgeState { pinch_active: true, ..Default::default() };
        let mut events = vec![
            PureInputEvent::Zoom(1.25),
            PureInputEvent::PointerMoved([40.0, 50.0]),
            PureInputEvent::PointerButton { pos: [40.0, 50.0], primary: false, pressed: true },
        ];
        filter_pure_events(&mut events, &mut state, false, false);
        assert_eq!(events, vec![PureInputEvent::PointerButton { pos: [40.0, 50.0], primary: false, pressed: true }]);
    }

    #[test]
    fn filter_keeps_touch_cancel_and_synthesizes_single_stuck_pointer_release() {
        let mut state = GestureBridgeState { pan_active: true, needs_pointer_release: true, ..Default::default() };
        let cancel = touch_event(7, PureTouchPhase::Cancel, [80.0, 90.0], None);
        let mut events = vec![cancel, PureInputEvent::PointerMoved([82.0, 92.0]), PureInputEvent::PointerGone];

        filter_pure_events(&mut events, &mut state, true, false);
        assert_eq!(
            events,
            vec![
                cancel,
                PureInputEvent::PointerGone,
                PureInputEvent::PointerMoved(OFFSCREEN_RELEASE_POS),
                PureInputEvent::PointerButton { pos: OFFSCREEN_RELEASE_POS, primary: true, pressed: false },
                PureInputEvent::PointerGone,
            ]
        );
        assert!(!state.needs_pointer_release);

        // Next frame after egui cleared `primary_down`: suppressed pointer move emits zero extra releases.
        let mut frame2 = vec![PureInputEvent::PointerMoved([90.0, 100.0])];
        filter_pure_events(&mut frame2, &mut state, false, false);
        assert!(frame2.is_empty());
    }

    #[test]
    fn filter_avoids_double_release_when_frame_already_contains_primary_release() {
        let mut state = GestureBridgeState { pan_active: false, needs_pointer_release: true, ..Default::default() };
        let cancel = touch_event(3, PureTouchPhase::Cancel, [60.0, 70.0], None);
        let real_release = PureInputEvent::PointerButton { pos: [60.0, 70.0], primary: true, pressed: false };
        let mut events = vec![cancel, real_release, PureInputEvent::PointerGone];

        filter_pure_events(&mut events, &mut state, true, false);
        assert_eq!(events, vec![cancel, real_release, PureInputEvent::PointerGone]);
        assert!(!state.needs_pointer_release);
    }

    #[test]
    fn filter_does_not_release_pointer_while_stylus_is_active() {
        let mut state = GestureBridgeState { pan_active: true, needs_pointer_release: true, ..Default::default() };
        let palm_cancel = touch_event(5, PureTouchPhase::Cancel, [200.0, 200.0], None);
        let pencil_move = touch_event(6, PureTouchPhase::Move, [300.0, 300.0], Some(0.6));
        let mut events = vec![palm_cancel, pencil_move, PureInputEvent::PointerMoved([300.0, 300.0])];

        filter_pure_events(&mut events, &mut state, true, true);
        assert_eq!(events, vec![palm_cancel, pencil_move, PureInputEvent::PointerMoved([300.0, 300.0])]);
    }

    #[test]
    fn filter_releases_when_needs_pointer_release_armed_even_without_events() {
        let mut state = GestureBridgeState { pan_active: true, needs_pointer_release: true, ..Default::default() };
        let mut events = Vec::new();
        filter_pure_events(&mut events, &mut state, true, false);
        assert_eq!(
            events,
            vec![
                PureInputEvent::PointerMoved(OFFSCREEN_RELEASE_POS),
                PureInputEvent::PointerButton { pos: OFFSCREEN_RELEASE_POS, primary: true, pressed: false },
                PureInputEvent::PointerGone,
            ]
        );
        assert!(!state.needs_pointer_release);

        // Calling again with empty events does not double-release spam even if pointer_was_down is still true.
        let mut frame2 = Vec::new();
        filter_pure_events(&mut frame2, &mut state, true, false);
        assert!(frame2.is_empty());
    }

    #[test]
    fn filter_preserves_touch_end_during_active_pan_for_long_press_cleanup() {
        let mut state = GestureBridgeState { pan_active: true, needs_pointer_release: false, ..Default::default() };
        let finger_end = touch_event(11, PureTouchPhase::End, [120.0, 220.0], None);
        let mut events = vec![finger_end, PureInputEvent::PointerMoved([120.0, 220.0])];
        filter_pure_events(&mut events, &mut state, false, false);
        assert_eq!(events, vec![finger_end]);
    }

    #[test]
    fn filter_passes_known_stylus_move_and_end_with_none_force_and_suppresses_stray_pointer() {
        let mut state = GestureBridgeState { pan_active: true, needs_pointer_release: true, ..Default::default() };
        let pencil_start = touch_event(99, PureTouchPhase::Start, [300.0, 300.0], Some(0.4));
        let palm_cancel = touch_event(1, PureTouchPhase::Cancel, [20.0, 20.0], None);
        let mut frame1 = vec![palm_cancel, pencil_start, PureInputEvent::PointerMoved([300.0, 300.0])];
        let mut stylus_ids = BTreeSet::from([99]);
        filter_pure_events_with_stylus_ids(&mut frame1, &mut state, true, &stylus_ids);
        assert_eq!(frame1, vec![palm_cancel, pencil_start, PureInputEvent::PointerMoved([300.0, 300.0])]);
        // Release was skipped because stylus was active, so needs_pointer_release must remain armed!
        assert!(state.needs_pointer_release);
        assert!(state.known_stylus_ids.contains(&99));

        // Frame 2: Pencil Move arrives with force: None alongside a stray finger PointerMoved([20, 20]).
        let pencil_move_light = touch_event(99, PureTouchPhase::Move, [305.0, 310.0], None);
        let stray_pointer = PureInputEvent::PointerMoved([20.0, 20.0]);
        let pencil_pointer = PureInputEvent::PointerMoved([305.0, 310.0]);
        let mut frame2 = vec![pencil_move_light, stray_pointer, pencil_pointer];
        filter_pure_events_with_stylus_ids(&mut frame2, &mut state, true, &stylus_ids);
        assert_eq!(frame2, vec![pencil_move_light, pencil_pointer]);

        // Frame 3: Pencil End arrives with force: None; passes through and removes ID 99 from known_stylus_ids.
        let pencil_end_light = touch_event(99, PureTouchPhase::End, [310.0, 315.0], None);
        let mut frame3 = vec![pencil_end_light];
        filter_pure_events_with_stylus_ids(&mut frame3, &mut state, true, &stylus_ids);
        assert_eq!(frame3, vec![pencil_end_light]);
        assert!(!state.known_stylus_ids.contains(&99));

        // Frame 4: Stylus is now gone, palm pointer still stuck -> deferred OFFSCREEN_RELEASE_POS fires once.
        stylus_ids.clear();
        let mut frame4 = Vec::new();
        filter_pure_events_with_stylus_ids(&mut frame4, &mut state, true, &stylus_ids);
        assert_eq!(
            frame4,
            vec![
                PureInputEvent::PointerMoved(OFFSCREEN_RELEASE_POS),
                PureInputEvent::PointerButton { pos: OFFSCREEN_RELEASE_POS, primary: true, pressed: false },
                PureInputEvent::PointerGone,
            ]
        );
        assert!(!state.needs_pointer_release);
    }
}
