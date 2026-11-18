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

#[cfg(target_os = "ios")]
mod ios {
    use super::{compute_keyboard_bottom_inset, compute_soft_keyboard_state};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{MainThreadMarker, msg_send};
    use objc2_core_foundation::CGRect;
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSValue};
    use objc2_ui_kit::{UIKeyboardFrameEndUserInfoKey, UIKeyboardWillChangeFrameNotification, UIKeyboardWillHideNotification, UIScreen};
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, PoisonError};

    #[derive(Clone, Copy, Default)]
    struct SafeAreaCache {
        base: egui::SafeAreaInsets,
        override_active: bool,
    }

    /// Returns the main screen height in UIKit points when called on the UIKit main thread.
    pub fn current_screen_height_pts() -> Option<f32> {
        let mtm = MainThreadMarker::new()?;
        // `UIScreen::mainScreen` is deprecated on iOS 16+; used as a scene-agnostic fallback.
        #[allow(deprecated)]
        let height = UIScreen::mainScreen(mtm).bounds().size.height as f32;
        (height.is_finite() && height > 0.0).then_some(height)
    }

    /// Observes UIKit keyboard frame notifications to update egui's bottom safe-area inset.
    pub struct KeyboardObserver {
        frame: Arc<Mutex<Option<(f32, f32, f32, f32)>>>,
        safe_area: Mutex<SafeAreaCache>,
        _will_change_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
        _will_hide_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl KeyboardObserver {
        /// Registers UIKit keyboard frame-change and hide observers on the main thread.
        pub fn new() -> Option<Self> {
            let _mtm = MainThreadMarker::new()?;
            let frame = Arc::new(Mutex::new(None));
            let center = NSNotificationCenter::defaultCenter();

            let change_frame = Arc::clone(&frame);
            let will_change_block = RcBlock::new(move |notification: NonNull<NSNotification>| {
                #[allow(unsafe_code)]
                // SAFETY: `NSNotificationCenter` passes a valid `NSNotification` pointer for this call.
                let notification = unsafe { notification.as_ref() };
                if let Some(end_frame) = extract_keyboard_end_frame(notification) {
                    *change_frame.lock().unwrap_or_else(PoisonError::into_inner) = Some(end_frame);
                }
            });

            let hide_frame = Arc::clone(&frame);
            let will_hide_block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
                *hide_frame.lock().unwrap_or_else(PoisonError::into_inner) = None;
            });

            #[allow(unsafe_code)]
            // SAFETY: Notification names are static UIKit constants, `obj`/`queue` are `None`, and both
            // `RcBlock` closures capture only `Arc<Mutex<_>>` (`Send + Sync + 'static`).
            let (_will_change_observer, _will_hide_observer) = unsafe {
                (
                    center.addObserverForName_object_queue_usingBlock(Some(UIKeyboardWillChangeFrameNotification), None, None, &will_change_block),
                    center.addObserverForName_object_queue_usingBlock(Some(UIKeyboardWillHideNotification), None, None, &will_hide_block),
                )
            };

            Some(Self { frame, safe_area: Mutex::new(SafeAreaCache::default()), _will_change_observer, _will_hide_observer })
        }

        /// Returns `(soft_keyboard_visible, bottom_inset_points)` for the current keyboard frame.
        pub fn soft_keyboard_state(&self, screen_height_points: f32, safe_bottom_points: f32) -> (bool, f32) {
            let kb_frame = *self.frame.lock().unwrap_or_else(PoisonError::into_inner);
            compute_soft_keyboard_state(screen_height_points, kb_frame, safe_bottom_points)
        }

        /// Applies the active docked keyboard inset to `raw_input.safe_area_insets`,
        /// preserving existing insets untouched while the keyboard stays hidden.
        pub fn apply_to_raw_input(&self, screen_height_pts: f32, raw_input: &mut egui::RawInput) {
            let mut cache = self.safe_area.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(insets) = raw_input.safe_area_insets {
                cache.base = insets;
            }
            let Some((_kb_x, kb_y, _kb_w, kb_h)) = *self.frame.lock().unwrap_or_else(PoisonError::into_inner) else {
                if cache.override_active {
                    if raw_input.safe_area_insets.is_none() {
                        raw_input.safe_area_insets = Some(cache.base);
                    }
                    cache.override_active = false;
                }
                return;
            };
            let mut margin = raw_input.safe_area_insets.unwrap_or(cache.base).0;
            margin.bottom = compute_keyboard_bottom_inset(screen_height_pts, kb_y, kb_h, margin.bottom);
            raw_input.safe_area_insets = Some(egui::SafeAreaInsets(margin));
            cache.override_active = true;
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
    use super::{compute_keyboard_bottom_inset, compute_soft_keyboard_state};

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 1e-5, "expected {expected}, got {actual}");
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
}
