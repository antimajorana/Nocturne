//! iPadOS / iOS software keyboard safe-area avoidance.
//!
//! Tracks `UIKeyboardWillChangeFrameNotification` and `UIKeyboardWillHideNotification`
//! so `Shell::raw_input_hook` can expand `raw_input.safe_area_insets.bottom` when a
//! docked keyboard covers the screen bottom while ignoring floating iPad keyboards.

const FLOATING_BOTTOM_GAP_PTS: f32 = 8.0;
const FLOATING_MAX_HEIGHT_RATIO: f32 = 0.55;
const MAX_INSET_HEIGHT_RATIO: f32 = 0.75;

/// Computes the effective bottom safe-area inset in UIKit points for a keyboard frame.
///
/// Floating mid-screen iPad keyboards (`kb_origin_y + kb_height < screen_height - 8` and
/// `kb_height < screen_height * 0.55`) leave `safe_bottom_pts` unchanged.
pub fn compute_keyboard_bottom_inset(screen_height_pts: f32, kb_origin_y_pts: f32, kb_height_pts: f32, safe_bottom_pts: f32) -> f32 {
    let safe = if safe_bottom_pts.is_finite() { safe_bottom_pts.max(0.0) } else { 0.0 };
    if !(screen_height_pts.is_finite() && kb_origin_y_pts.is_finite() && kb_height_pts.is_finite())
        || screen_height_pts <= 0.0
        || kb_height_pts <= 0.0
        || kb_origin_y_pts >= screen_height_pts
    {
        return safe;
    }
    let is_floating = kb_origin_y_pts + kb_height_pts < screen_height_pts - FLOATING_BOTTOM_GAP_PTS
        && kb_height_pts < screen_height_pts * FLOATING_MAX_HEIGHT_RATIO;
    if is_floating {
        return safe;
    }
    (screen_height_pts - kb_origin_y_pts).clamp(0.0, screen_height_pts * MAX_INSET_HEIGHT_RATIO).max(safe)
}

/// Computes `(soft_keyboard_visible, bottom_inset_points)` for an optional UIKit keyboard frame
/// `(x, y, width, height)` in logical points.
///
/// Docked keyboards return `(true, bottom_inset_points)` expanding `bottom_inset_points` to cover the
/// keyboard overlap above `safe_bottom_points`. Floating iPad keyboards return `(true, safe_bottom_points.max(0.0))`
/// so the mobile formatting bar appears without shrinking the viewport under the floating keyboard.
pub fn compute_soft_keyboard_state(screen_height_points: f32, kb_frame: Option<(f32, f32, f32, f32)>, safe_bottom_points: f32) -> (bool, f32) {
    let safe = if safe_bottom_points.is_finite() { safe_bottom_points.max(0.0) } else { 0.0 };
    let Some((kb_x, kb_y, kb_w, kb_h)) = kb_frame else {
        return (false, safe);
    };
    if !(screen_height_points.is_finite() && kb_x.is_finite() && kb_y.is_finite() && kb_w.is_finite() && kb_h.is_finite())
        || screen_height_points <= 0.0
        || kb_h <= 1.0
        || kb_w < 0.0
        || kb_y >= screen_height_points
        || kb_y + kb_h <= 0.0
    {
        return (false, safe);
    }
    let inset = compute_keyboard_bottom_inset(screen_height_points, kb_y, kb_h, safe);
    (true, inset.max(safe))
}

/// Computes the effective bottom safe-area inset in UIKit view points for a keyboard frame
/// already converted into the host `UIView`'s coordinate space.
///
/// Works uniformly for full-screen windows as well as Stage Manager, Split View, and Slide Over:
/// - Horizontally or vertically disjoint keyboards (e.g. a Stage Manager window floating above
///   the docked keyboard, or a floating keyboard dragged onto the other half of Split View) leave
///   `safe_bottom_pts` unchanged.
/// - Floating iPad keyboards hovering above the bottom of the view (`k_bottom < v_bottom - 8`
///   and `kb_height < view_height * 0.55`) leave `safe_bottom_pts` unchanged.
/// - Docked or partially overlapping keyboards return only the actual bottom overlap with
///   `view_bounds`, clamped to `75%` of view height and never below `safe_bottom_pts` (so the
///   hardware home-indicator inset is neither lost nor double-counted).
#[must_use]
pub fn compute_view_keyboard_bottom_inset(view_bounds: [f32; 4], kb_frame_in_view: [f32; 4], safe_bottom_pts: f32) -> f32 {
    let safe = if safe_bottom_pts.is_finite() { safe_bottom_pts.max(0.0) } else { 0.0 };
    let [vx, vy, vw, vh] = view_bounds;
    let [kx, ky, kw, kh] = kb_frame_in_view;
    if !(vx.is_finite() && vy.is_finite() && vw.is_finite() && vh.is_finite() && kx.is_finite() && ky.is_finite() && kw.is_finite() && kh.is_finite())
        || vw <= 0.0
        || vh <= 0.0
        || kw <= 0.0
        || kh <= 0.0
    {
        return safe;
    }

    let v_right = vx + vw;
    let v_bottom = vy + vh;
    let k_right = kx + kw;
    let k_bottom = ky + kh;

    let x_overlap = k_right.min(v_right) - kx.max(vx);
    let y_overlap = k_bottom.min(v_bottom) - ky.max(vy);
    if !(x_overlap.is_finite() && y_overlap.is_finite()) || x_overlap <= 0.0 || y_overlap <= 0.0 {
        return safe;
    }

    let is_floating = k_bottom < v_bottom - FLOATING_BOTTOM_GAP_PTS && kh < vh * FLOATING_MAX_HEIGHT_RATIO;
    if is_floating {
        return safe;
    }

    (v_bottom - ky).clamp(0.0, vh * MAX_INSET_HEIGHT_RATIO).max(safe)
}

/// Computes `(soft_keyboard_visible, effective_bottom_inset_pts)` from a keyboard frame in
/// `view_bounds` space and an optional screen-level visibility flag.
///
/// When `screen_kb_visible` is `Some(true)` and `kb_frame_in_view` does not overlap `view_bounds`
/// (for instance, a Stage Manager window positioned completely above the docked keyboard), this
/// returns `(true, safe_bottom_pts.max(0.0))` so the formatting bar remains available without
/// shrinking the window.
#[must_use]
pub fn compute_view_keyboard_state(
    view_bounds: [f32; 4],
    kb_frame_in_view: Option<[f32; 4]>,
    screen_kb_visible: Option<bool>,
    safe_bottom_pts: f32,
) -> (bool, f32) {
    let safe = if safe_bottom_pts.is_finite() { safe_bottom_pts.max(0.0) } else { 0.0 };
    let Some(kb) = kb_frame_in_view else {
        return (false, safe);
    };
    let [vx, vy, vw, vh] = view_bounds;
    let [kx, ky, kw, kh] = kb;
    if !(vx.is_finite() && vy.is_finite() && vw.is_finite() && vh.is_finite() && kx.is_finite() && ky.is_finite() && kw.is_finite() && kh.is_finite())
        || vw <= 0.0
        || vh <= 0.0
        || kw < 0.0
        || kh <= 1.0
    {
        return (false, safe);
    }

    let intersects_view = kw > 0.0 && (kx + kw).min(vx + vw) > kx.max(vx) && (ky + kh).min(vy + vh) > ky.max(vy);
    let visible = screen_kb_visible.unwrap_or(intersects_view);
    if !visible {
        return (false, safe);
    }

    let inset = compute_view_keyboard_bottom_inset(view_bounds, kb, safe);
    (true, inset.max(safe))
}

/// Converts a view-space keyboard bottom inset (in UIKit points) into egui logical points
/// using `zoom_factor`, while preserving at least `hardware_safe_bottom_egui`.
#[must_use]
pub fn scale_keyboard_inset_to_egui(inset_view_pts: f32, hardware_safe_bottom_egui: f32, zoom_factor: f32) -> f32 {
    let safe = if hardware_safe_bottom_egui.is_finite() { hardware_safe_bottom_egui.max(0.0) } else { 0.0 };
    let z = if zoom_factor.is_finite() && zoom_factor > 0.0 { zoom_factor } else { 1.0 };
    let scaled = if inset_view_pts.is_finite() && inset_view_pts > 0.0 { inset_view_pts / z } else { 0.0 };
    if scaled.is_finite() { scaled.max(safe) } else { safe }
}

fn sanitize_safe_area_insets(insets: egui::SafeAreaInsets) -> egui::SafeAreaInsets {
    let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let m = insets.0;
    egui::SafeAreaInsets(egui::epaint::MarginF32 { left: clean(m.left), right: clean(m.right), top: clean(m.top), bottom: clean(m.bottom) })
}

/// Caches hardware safe-area insets across frames so temporary keyboard overrides neither lose
/// the hardware home-indicator inset on frames where `egui-winit` passes `safe_area_insets = None`
/// nor double-count `safe_bottom` while the software keyboard is open.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyboardInsetCache {
    hardware_base: egui::SafeAreaInsets,
    has_hardware_base: bool,
    override_active: bool,
}

impl KeyboardInsetCache {
    /// Creates an empty cache with zeroed hardware insets.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the cached hardware safe-area insets whenever `egui-winit` delivers a fresh
    /// `Some(insets)` (on resize, scale-factor change, focus, or un-occlusion).
    pub fn record_hardware_insets(&mut self, incoming: Option<egui::SafeAreaInsets>) {
        if let Some(insets) = incoming {
            self.hardware_base = sanitize_safe_area_insets(insets);
            self.has_hardware_base = true;
        }
    }

    /// Returns the most recently cached hardware safe-area insets.
    #[must_use]
    pub fn hardware_insets(&self) -> egui::SafeAreaInsets {
        self.hardware_base
    }

    /// Returns the cached hardware bottom safe-area inset (e.g. home indicator), or `0.0` before
    /// the first hardware inset update arrives.
    #[must_use]
    pub fn hardware_safe_bottom(&self) -> f32 {
        if self.has_hardware_base && self.hardware_base.0.bottom.is_finite() { self.hardware_base.0.bottom.max(0.0) } else { 0.0 }
    }

    /// Returns `true` when the cache is currently overriding `raw_input.safe_area_insets` with a
    /// keyboard bottom inset.
    #[must_use]
    pub fn is_override_active(&self) -> bool {
        self.override_active
    }

    /// Applies an optional keyboard bottom inset to `raw_input.safe_area_insets`, returning the
    /// effective bottom inset in points.
    ///
    /// - Fresh hardware `raw_input.safe_area_insets` are captured before any keyboard override is
    ///   written so rotations/resizes while the keyboard is visible keep the true hardware insets.
    /// - When `keyboard_bottom_inset` is `Some(kb)`, `margin.bottom` becomes `kb.max(safe_bottom)`
    ///   (never `kb + safe_bottom`).
    /// - When `keyboard_bottom_inset` is `None` after an active override, `hardware_base` is
    ///   restored once onto `raw_input.safe_area_insets`.
    pub fn apply_to_raw_input(&mut self, raw_input: &mut egui::RawInput, keyboard_bottom_inset: Option<f32>) -> f32 {
        self.record_hardware_insets(raw_input.safe_area_insets);
        let safe_bottom = self.hardware_safe_bottom();
        let Some(kb_bottom) = keyboard_bottom_inset else {
            if self.override_active {
                if raw_input.safe_area_insets.is_none() {
                    raw_input.safe_area_insets = Some(self.hardware_base);
                }
                self.override_active = false;
            }
            return safe_bottom;
        };

        let effective = if kb_bottom.is_finite() { kb_bottom.max(safe_bottom) } else { safe_bottom };
        let mut margin = self.hardware_base.0;
        margin.bottom = effective;
        raw_input.safe_area_insets = Some(egui::SafeAreaInsets(margin));
        self.override_active = true;
        effective
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use super::{KeyboardInsetCache, compute_soft_keyboard_state, compute_view_keyboard_state, scale_keyboard_inset_to_egui};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{MainThreadMarker, msg_send};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSValue};
    use objc2_ui_kit::{UIKeyboardFrameEndUserInfoKey, UIKeyboardWillChangeFrameNotification, UIKeyboardWillHideNotification, UIScreen, UIView};
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, PoisonError};

    /// Returns the main screen height in UIKit points when called on the UIKit main thread.
    pub fn current_screen_height_pts() -> Option<f32> {
        let mtm = MainThreadMarker::new()?;
        // `UIScreen::mainScreen` is deprecated on iOS 16+; used as a scene-agnostic fallback.
        #[allow(deprecated)]
        let height = UIScreen::mainScreen(mtm).bounds().size.height as f32;
        (height.is_finite() && height > 0.0).then_some(height)
    }

    fn convert_screen_rect_to_view(view: &UIView, screen_frame: (f32, f32, f32, f32)) -> Option<([f32; 4], [f32; 4])> {
        let window = view.window()?;
        let cg_screen = CGRect::new(
            CGPoint::new(f64::from(screen_frame.0), f64::from(screen_frame.1)),
            CGSize::new(f64::from(screen_frame.2), f64::from(screen_frame.3)),
        );
        // Passing `None` converts from screen coordinates to `window` coordinates, then from
        // `window` coordinates into `view`'s own bounds coordinate space (handling Stage Manager,
        // Split View, and Slide Over offsets).
        let in_window = window.convertRect_fromWindow(cg_screen, None);
        let in_view = view.convertRect_fromView(in_window, None);
        let bounds = view.bounds();
        let kb = [in_view.origin.x as f32, in_view.origin.y as f32, in_view.size.width as f32, in_view.size.height as f32];
        let vb = [bounds.origin.x as f32, bounds.origin.y as f32, bounds.size.width as f32, bounds.size.height as f32];
        if kb.iter().all(|v| v.is_finite()) && vb.iter().all(|v| v.is_finite()) && vb[2] > 0.0 && vb[3] > 0.0 { Some((kb, vb)) } else { None }
    }

    fn is_screen_frame_visible(screen_frame: (f32, f32, f32, f32), screen_height_pts: f32) -> bool {
        let (sx, sy, sw, sh) = screen_frame;
        sx.is_finite()
            && sy.is_finite()
            && sw.is_finite()
            && sh.is_finite()
            && screen_height_pts.is_finite()
            && screen_height_pts > 0.0
            && sw >= 0.0
            && sh > 1.0
            && sy < screen_height_pts
            && sy + sh > 0.0
    }

    /// Observes UIKit keyboard frame notifications to update egui's bottom safe-area inset
    /// and wake `eframe` immediately when the keyboard opens, closes, or resizes.
    pub struct KeyboardObserver {
        frame: Arc<Mutex<Option<(f32, f32, f32, f32)>>>,
        safe_area: Mutex<KeyboardInsetCache>,
        _will_change_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
        _will_hide_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl KeyboardObserver {
        /// Registers UIKit keyboard frame-change and hide observers on the main thread.
        ///
        /// Both notifications wake `egui_ctx` via `request_repaint()` because UIKit keyboard
        /// frame changes arrive without a corresponding `winit` event.
        pub fn new(egui_ctx: &egui::Context) -> Option<Self> {
            let _mtm = MainThreadMarker::new()?;
            let frame = Arc::new(Mutex::new(None));
            let center = NSNotificationCenter::defaultCenter();

            let change_frame = Arc::clone(&frame);
            let change_ctx = egui_ctx.clone();
            let will_change_block = RcBlock::new(move |notification: NonNull<NSNotification>| {
                #[allow(unsafe_code)]
                // SAFETY: `NSNotificationCenter` passes a valid `NSNotification` pointer for this call.
                let notification = unsafe { notification.as_ref() };
                if let Some(end_frame) = extract_keyboard_end_frame(notification) {
                    {
                        *change_frame.lock().unwrap_or_else(PoisonError::into_inner) = Some(end_frame);
                    }
                    change_ctx.request_repaint();
                }
            });

            let hide_frame = Arc::clone(&frame);
            let hide_ctx = egui_ctx.clone();
            let will_hide_block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
                {
                    *hide_frame.lock().unwrap_or_else(PoisonError::into_inner) = None;
                }
                hide_ctx.request_repaint();
            });

            #[allow(unsafe_code)]
            // SAFETY: Notification names are static UIKit constants, `obj`/`queue` are `None`, and both
            // `RcBlock` closures capture only `Arc<Mutex<_>>` and `egui::Context` (`Send + Sync + 'static`).
            let (_will_change_observer, _will_hide_observer) = unsafe {
                (
                    center.addObserverForName_object_queue_usingBlock(Some(UIKeyboardWillChangeFrameNotification), None, None, &will_change_block),
                    center.addObserverForName_object_queue_usingBlock(Some(UIKeyboardWillHideNotification), None, None, &will_hide_block),
                )
            };

            Some(Self { frame, safe_area: Mutex::new(KeyboardInsetCache::new()), _will_change_observer, _will_hide_observer })
        }

        /// Returns the cached hardware bottom safe-area inset in points.
        #[must_use]
        pub fn cached_safe_bottom(&self) -> f32 {
            self.safe_area.lock().unwrap_or_else(PoisonError::into_inner).hardware_safe_bottom()
        }

        /// Returns `(soft_keyboard_visible, bottom_inset_points)` using screen-space fallback math
        /// while respecting the cached hardware `safe_bottom`.
        pub fn soft_keyboard_state(&self, screen_height_points: f32, safe_bottom_points: f32) -> (bool, f32) {
            let kb_frame = *self.frame.lock().unwrap_or_else(PoisonError::into_inner);
            let cached = self.cached_safe_bottom();
            let safe = if safe_bottom_points.is_finite() { safe_bottom_points.max(0.0).max(cached) } else { cached };
            compute_soft_keyboard_state(screen_height_points, kb_frame, safe)
        }

        /// Converts the latest screen-space keyboard frame into `view` coordinates (handling Stage
        /// Manager, Split View, and Slide Over), caches hardware safe-area insets across frames,
        /// updates `raw_input.safe_area_insets`, and returns `(soft_keyboard_visible, effective_bottom_inset_egui)`.
        pub fn apply_to_raw_input(
            &self,
            view: Option<&UIView>,
            zoom_factor: f32,
            fallback_screen_height_pts: f32,
            raw_input: &mut egui::RawInput,
        ) -> (bool, f32) {
            let mut cache = self.safe_area.lock().unwrap_or_else(PoisonError::into_inner);
            cache.record_hardware_insets(raw_input.safe_area_insets);
            let safe_bottom_egui = cache.hardware_safe_bottom();

            let kb_screen = *self.frame.lock().unwrap_or_else(PoisonError::into_inner);
            let (visible, desired_bottom_egui) = match kb_screen {
                None => (false, safe_bottom_egui),
                Some(screen_frame) => {
                    if let Some(host_view) = view
                        && let Some((kb_in_view, view_bounds)) = convert_screen_rect_to_view(host_view, screen_frame)
                    {
                        let screen_vis = current_screen_height_pts().map(|sh| is_screen_frame_visible(screen_frame, sh));
                        let safe_view_pts =
                            if zoom_factor.is_finite() && zoom_factor > 0.0 { safe_bottom_egui * zoom_factor } else { safe_bottom_egui };
                        let (vis, inset_view_pts) = compute_view_keyboard_state(view_bounds, Some(kb_in_view), screen_vis, safe_view_pts);
                        let inset_egui = scale_keyboard_inset_to_egui(inset_view_pts, safe_bottom_egui, zoom_factor);
                        (vis, inset_egui)
                    } else {
                        let screen_h = current_screen_height_pts().unwrap_or(fallback_screen_height_pts);
                        let (vis, inset_view_pts) = compute_soft_keyboard_state(screen_h, Some(screen_frame), safe_bottom_egui);
                        let inset_egui = scale_keyboard_inset_to_egui(inset_view_pts, safe_bottom_egui, zoom_factor);
                        (vis, inset_egui)
                    }
                }
            };

            let effective = cache.apply_to_raw_input(raw_input, visible.then_some(desired_bottom_egui));
            (visible, effective)
        }
    }

    impl Drop for KeyboardObserver {
        fn drop(&mut self) {
            let center = NSNotificationCenter::defaultCenter();
            #[allow(unsafe_code)]
            // SAFETY: Both tokens came from `addObserverForName:object:queue:usingBlock:` on `center`.
            unsafe {
                center.removeObserver(self._will_change_observer.as_ref());
                center.removeObserver(self._will_hide_observer.as_ref());
            }
        }
    }

    fn extract_keyboard_end_frame(notification: &NSNotification) -> Option<(f32, f32, f32, f32)> {
        let info = notification.userInfo()?;
        #[allow(unsafe_code)]
        // SAFETY: `UIKeyboardFrameEndUserInfoKey` is a static UIKit `NSString` constant.
        let key: &AnyObject = unsafe { UIKeyboardFrameEndUserInfoKey }.as_ref();
        let value = info.objectForKey(key)?;
        let ns_value = value.downcast_ref::<NSValue>()?;
        if !ns_value.contains_encoding::<CGRect>() {
            return None;
        }
        #[allow(unsafe_code)]
        // SAFETY: `contains_encoding::<CGRect>()` verified `ns_value` holds a `CGRect`.
        let frame: CGRect = unsafe { msg_send![ns_value, CGRectValue] };
        let (x, y, w, h) = (frame.origin.x as f32, frame.origin.y as f32, frame.size.width as f32, frame.size.height as f32);
        (x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()).then_some((x, y, w, h))
    }
}

#[cfg(target_os = "ios")]
pub use ios::{KeyboardObserver, current_screen_height_pts};

#[cfg(test)]
mod tests {
    use super::{
        KeyboardInsetCache, compute_keyboard_bottom_inset, compute_soft_keyboard_state, compute_view_keyboard_bottom_inset,
        compute_view_keyboard_state, scale_keyboard_inset_to_egui,
    };

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 1e-5, "expected {expected}, got {actual}");
    }

    fn safe_insets(top: f32, bottom: f32, left: f32, right: f32) -> egui::SafeAreaInsets {
        egui::SafeAreaInsets(egui::epaint::MarginF32 { left, right, top, bottom })
    }

    #[test]
    fn hidden_keyboard_preserves_safe_bottom() {
        assert_close(compute_keyboard_bottom_inset(1024.0, 1024.0, 314.0, 20.0), 20.0);
        assert_close(compute_keyboard_bottom_inset(1024.0, 1200.0, 314.0, 20.0), 20.0);
    }

    #[test]
    fn docked_portrait_ipad_keyboard_expands_bottom_inset() {
        assert_close(compute_keyboard_bottom_inset(1024.0, 710.0, 314.0, 20.0), 314.0);
    }

    #[test]
    fn docked_landscape_ipad_keyboard_expands_bottom_inset() {
        assert_close(compute_keyboard_bottom_inset(768.0, 415.0, 353.0, 20.0), 353.0);
    }

    #[test]
    fn floating_ipad_keyboard_mid_screen_does_not_shrink_app() {
        assert_close(compute_keyboard_bottom_inset(1024.0, 380.0, 280.0, 20.0), 20.0);
    }

    #[test]
    fn hardware_safe_bottom_larger_than_overlap_is_preserved() {
        assert_close(compute_keyboard_bottom_inset(1024.0, 1012.0, 12.0, 24.0), 24.0);
    }

    #[test]
    fn negative_nan_and_zero_inputs_fall_back_safely() {
        for (h, y, kh, sb, expected) in [
            (0.0, 500.0, 300.0, 18.0, 18.0),
            (-1024.0, 500.0, 300.0, 18.0, 18.0),
            (1024.0, 500.0, 0.0, 18.0, 18.0),
            (1024.0, 500.0, -50.0, 18.0, 18.0),
            (f32::NAN, 500.0, 300.0, 18.0, 18.0),
            (1024.0, f32::NAN, 300.0, 18.0, 18.0),
            (1024.0, 500.0, f32::INFINITY, 18.0, 18.0),
            (1024.0, 710.0, 314.0, -10.0, 314.0),
            (1024.0, 1024.0, 314.0, f32::NAN, 0.0),
            (1024.0, 1024.0, 314.0, -12.0, 0.0),
        ] {
            assert_close(compute_keyboard_bottom_inset(h, y, kh, sb), expected);
        }
    }

    #[test]
    fn soft_keyboard_state_hidden_when_none_offscreen_or_zero_height() {
        for (screen_h, frame, safe, exp_vis, exp_inset) in [
            (1024.0, None, 20.0, false, 20.0),
            (1024.0, Some((0.0, 1024.0, 768.0, 314.0)), 20.0, false, 20.0),
            (1024.0, Some((0.0, 1200.0, 768.0, 314.0)), 20.0, false, 20.0),
            (1024.0, Some((0.0, -314.0, 768.0, 314.0)), 20.0, false, 20.0),
            (1024.0, Some((0.0, 710.0, 768.0, 1.0)), 20.0, false, 20.0),
            (1024.0, Some((0.0, 710.0, 768.0, 0.0)), 20.0, false, 20.0),
            (1024.0, None, -5.0, false, 0.0),
            (1024.0, None, f32::NAN, false, 0.0),
        ] {
            let (vis, inset) = compute_soft_keyboard_state(screen_h, frame, safe);
            assert_eq!(vis, exp_vis);
            assert_close(inset, exp_inset);
        }
    }

    #[test]
    fn soft_keyboard_state_docked_portrait_and_landscape() {
        let (vis_p, inset_p) = compute_soft_keyboard_state(1024.0, Some((0.0, 710.0, 768.0, 314.0)), 20.0);
        assert!(vis_p);
        assert_close(inset_p, 314.0);

        let (vis_l, inset_l) = compute_soft_keyboard_state(768.0, Some((0.0, 415.0, 1024.0, 353.0)), 20.0);
        assert!(vis_l);
        assert_close(inset_l, 353.0);

        let (vis_hw, inset_hw) = compute_soft_keyboard_state(1024.0, Some((0.0, 1012.0, 768.0, 12.0)), 24.0);
        assert!(vis_hw);
        assert_close(inset_hw, 24.0);
    }

    #[test]
    fn soft_keyboard_state_floating_ipad_keyboard_shows_bar_without_shrinking_viewport() {
        let (vis, inset) = compute_soft_keyboard_state(1024.0, Some((220.0, 380.0, 320.0, 280.0)), 20.0);
        assert!(vis);
        assert_close(inset, 20.0);
    }

    #[test]
    fn soft_keyboard_state_nan_inf_and_invalid_dimensions_fail_closed() {
        for (screen_h, frame, safe, exp_vis, exp_inset) in [
            (0.0, Some((0.0, 710.0, 768.0, 314.0)), 18.0, false, 18.0),
            (f32::NAN, Some((0.0, 710.0, 768.0, 314.0)), 18.0, false, 18.0),
            (1024.0, Some((f32::NAN, 710.0, 768.0, 314.0)), 18.0, false, 18.0),
            (1024.0, Some((0.0, f32::INFINITY, 768.0, 314.0)), 18.0, false, 18.0),
            (1024.0, Some((0.0, 710.0, f32::NAN, 314.0)), 18.0, false, 18.0),
            (1024.0, Some((0.0, 710.0, 768.0, f32::INFINITY)), 18.0, false, 18.0),
            (1024.0, Some((0.0, 710.0, -10.0, 314.0)), 18.0, false, 18.0),
            (1024.0, Some((0.0, 710.0, 768.0, 314.0)), f32::INFINITY, true, 314.0),
        ] {
            let (vis, inset) = compute_soft_keyboard_state(screen_h, frame, safe);
            assert_eq!(vis, exp_vis);
            assert_close(inset, exp_inset);
        }
    }

    #[test]
    fn view_space_stage_manager_split_view_and_slide_over_overlaps() {
        // 1. Stage Manager window floating completely above docked keyboard:
        //    Screen 1366x1024, keyboard [0, 710, 1366, 314], window at [100, 60, 700, 600]
        //    -> kb in view space is [-100, 650, 1366, 314]. No overlap with view [0, 0, 700, 600].
        let sm_above = [0.0, 0.0, 700.0, 600.0];
        let kb_below_sm = [-100.0, 650.0, 1366.0, 314.0];
        assert_close(compute_view_keyboard_bottom_inset(sm_above, kb_below_sm, 20.0), 20.0);
        let (vis_sm_above, inset_sm_above) = compute_view_keyboard_state(sm_above, Some(kb_below_sm), Some(true), 20.0);
        assert!(vis_sm_above, "keyboard is still visible on screen even when below Stage Manager window");
        assert_close(inset_sm_above, 20.0);

        // 2. Stage Manager window partially overlapped by docked keyboard:
        //    Window at [100, 200, 700, 620] (bottom = 820), keyboard at [0, 710, 1366, 314]
        //    -> kb in view space is [-100, 510, 1366, 314], overlapping bottom 110pt (not 314pt!).
        let sm_partial = [0.0, 0.0, 700.0, 620.0];
        let kb_partial = [-100.0, 510.0, 1366.0, 314.0];
        assert_close(compute_view_keyboard_bottom_inset(sm_partial, kb_partial, 20.0), 110.0);
        let (vis_partial, inset_partial) = compute_view_keyboard_state(sm_partial, Some(kb_partial), Some(true), 20.0);
        assert!(vis_partial);
        assert_close(inset_partial, 110.0);

        // 3. Split View right pane [686, 0, 680, 1024]:
        //    Docked keyboard [0, 710, 1366, 314] -> in view space [-686, 710, 1366, 314].
        let split_right = [0.0, 0.0, 680.0, 1024.0];
        let kb_split_right = [-686.0, 710.0, 1366.0, 314.0];
        assert_close(compute_view_keyboard_bottom_inset(split_right, kb_split_right, 20.0), 314.0);

        // 4. Slide Over right pane [1000, 24, 320, 980]:
        //    Docked keyboard [0, 710, 1366, 314] -> in view space [-1000, 686, 1366, 314], overlap = 294pt.
        let slide_over = [0.0, 0.0, 320.0, 980.0];
        let kb_slide_over = [-1000.0, 686.0, 1366.0, 314.0];
        assert_close(compute_view_keyboard_bottom_inset(slide_over, kb_slide_over, 20.0), 294.0);

        // 5. Floating keyboard dragged horizontally onto the OTHER side of Split View / Slide Over:
        //    No horizontal intersection -> leaves safe_bottom untouched!
        let kb_other_pane = [-700.0, 400.0, 320.0, 280.0];
        assert_close(compute_view_keyboard_bottom_inset(slide_over, kb_other_pane, 20.0), 20.0);
        let (vis_other, inset_other) = compute_view_keyboard_state(slide_over, Some(kb_other_pane), Some(true), 20.0);
        assert!(vis_other);
        assert_close(inset_other, 20.0);

        // 6. Floating keyboard hovering mid-window inside Stage Manager does not shrink window.
        let kb_float_sm = [180.0, 150.0, 320.0, 260.0];
        assert_close(compute_view_keyboard_bottom_inset(sm_partial, kb_float_sm, 20.0), 20.0);

        // 7. Offscreen keyboard (`screen_kb_visible == Some(false)`) reports hidden and preserves safe_bottom.
        let (vis_off, inset_off) = compute_view_keyboard_state(sm_partial, Some(kb_partial), Some(false), 20.0);
        assert!(!vis_off);
        assert_close(inset_off, 20.0);

        // 8. Non-finite and invalid dimensions fail closed safely.
        assert_close(compute_view_keyboard_bottom_inset([0.0, 0.0, 0.0, 600.0], kb_partial, 18.0), 18.0);
        assert_close(compute_view_keyboard_bottom_inset(sm_partial, [f32::NAN, 510.0, 1366.0, 314.0], 18.0), 18.0);
        assert_eq!(compute_view_keyboard_state(sm_partial, None, Some(true), 18.0), (false, 18.0));
    }

    #[test]
    fn scale_keyboard_inset_respects_zoom_factor_and_hardware_safe_bottom() {
        assert_close(scale_keyboard_inset_to_egui(314.0, 20.0, 1.0), 314.0);
        assert_close(scale_keyboard_inset_to_egui(314.0, 20.0, 1.25), 251.2);
        assert_close(scale_keyboard_inset_to_egui(12.0, 20.0, 1.25), 20.0);
        assert_close(scale_keyboard_inset_to_egui(314.0, 20.0, f32::NAN), 314.0);
        assert_close(scale_keyboard_inset_to_egui(314.0, 20.0, 0.0), 314.0);
        assert_close(scale_keyboard_inset_to_egui(314.0, 20.0, -2.0), 314.0);
        assert_close(scale_keyboard_inset_to_egui(f32::INFINITY, 20.0, 1.25), 20.0);
    }

    #[test]
    fn keyboard_inset_cache_preserves_hardware_safe_bottom_without_double_counting_or_loss() {
        let mut cache = KeyboardInsetCache::new();
        assert_close(cache.hardware_safe_bottom(), 0.0);
        assert!(!cache.is_override_active());

        // Frame 0: egui-winit delivers hardware safe area (status bar 24pt, home indicator 20pt).
        let mut raw0 = egui::RawInput { safe_area_insets: Some(safe_insets(24.0, 20.0, 0.0, 0.0)), ..Default::default() };
        assert_close(cache.apply_to_raw_input(&mut raw0, None), 20.0);
        assert_close(cache.hardware_safe_bottom(), 20.0);
        assert_eq!(raw0.safe_area_insets, Some(safe_insets(24.0, 20.0, 0.0, 0.0)));
        assert!(!cache.is_override_active());

        // Frame 1: normal frame with `safe_area_insets = None` still remembers 20pt hardware safe_bottom.
        let mut raw1 = egui::RawInput::default();
        assert_close(cache.apply_to_raw_input(&mut raw1, None), 20.0);
        assert_eq!(raw1.safe_area_insets, None);

        // Frame 2: docked keyboard opens (314pt). Must NOT double-count (not 334pt!), preserves top=24pt.
        let mut raw2 = egui::RawInput::default();
        assert_close(cache.apply_to_raw_input(&mut raw2, Some(314.0)), 314.0);
        assert_eq!(raw2.safe_area_insets, Some(safe_insets(24.0, 314.0, 0.0, 0.0)));
        assert!(cache.is_override_active());
        assert_close(cache.hardware_safe_bottom(), 20.0);

        // Frame 3: keyboard shrinks to small 12pt overlap -> clamped up to cached 20pt hardware safe_bottom.
        let mut raw3 = egui::RawInput::default();
        assert_close(cache.apply_to_raw_input(&mut raw3, Some(12.0)), 20.0);
        assert_eq!(raw3.safe_area_insets, Some(safe_insets(24.0, 20.0, 0.0, 0.0)));

        // Frame 4: orientation change delivers fresh hardware insets (top=0, bottom=24) while keyboard is open.
        let mut raw4 = egui::RawInput { safe_area_insets: Some(safe_insets(0.0, 24.0, 0.0, 0.0)), ..Default::default() };
        assert_close(cache.apply_to_raw_input(&mut raw4, Some(353.0)), 353.0);
        assert_eq!(raw4.safe_area_insets, Some(safe_insets(0.0, 353.0, 0.0, 0.0)));
        assert_close(cache.hardware_safe_bottom(), 24.0);

        // Frame 5: keyboard hides on a `None` frame -> restores the updated hardware base (top=0, bottom=24) once!
        let mut raw5 = egui::RawInput::default();
        assert_close(cache.apply_to_raw_input(&mut raw5, None), 24.0);
        assert_eq!(raw5.safe_area_insets, Some(safe_insets(0.0, 24.0, 0.0, 0.0)));
        assert!(!cache.is_override_active());

        // Frame 6: steady-state hidden frame leaves `raw_input.safe_area_insets = None` untouched.
        let mut raw6 = egui::RawInput::default();
        assert_close(cache.apply_to_raw_input(&mut raw6, None), 24.0);
        assert_eq!(raw6.safe_area_insets, None);
    }
}
