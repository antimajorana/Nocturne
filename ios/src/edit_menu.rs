//! Native iPadOS / iOS edit menu bridge (`UIEditMenuInteraction` on iOS 16+,
//! `UIMenuController` fallback on iOS 15.x).
//!
//! The pure-Rust state, capability matrix, and target-rect helpers at the top
//! of this module compile and run unit tests on all hosts, while the UIKit
//! bridge under [`ios`] is gated behind `#[cfg(target_os = "ios")]`.

/// Standard editing action surfaced in the native iPadOS / iOS edit menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Delete,
}

impl EditMenuAction {
    /// Objective-C standard responder selector corresponding to this action.
    #[must_use]
    pub const fn selector_name(self) -> &'static str {
        match self {
            Self::Cut => "cut:",
            Self::Copy => "copy:",
            Self::Paste => "paste:",
            Self::SelectAll => "selectAll:",
            Self::Delete => "delete:",
        }
    }
}

/// Pure-Rust specification for a single item in the native edit menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditMenuItemSpec {
    pub action: EditMenuAction,
    pub title: &'static str,
    pub identifier: &'static str,
}

/// Current editor/pasteboard capability flags controlling which menu items appear.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditMenuCapabilities {
    pub has_selection: bool,
    pub can_paste: bool,
    pub is_writable: bool,
}

/// Builds the ordered list of menu items appropriate for the current editor state.
#[must_use]
pub fn build_menu_items(has_selection: bool, can_paste: bool, is_writable: bool) -> Vec<EditMenuItemSpec> {
    let mut items = Vec::with_capacity(5);
    if has_selection && is_writable {
        items.push(EditMenuItemSpec { action: EditMenuAction::Cut, title: "Cut", identifier: "nocturne.edit.cut" });
    }
    if has_selection {
        items.push(EditMenuItemSpec { action: EditMenuAction::Copy, title: "Copy", identifier: "nocturne.edit.copy" });
    }
    if is_writable && can_paste {
        items.push(EditMenuItemSpec { action: EditMenuAction::Paste, title: "Paste", identifier: "nocturne.edit.paste" });
    }
    items.push(EditMenuItemSpec { action: EditMenuAction::SelectAll, title: "Select All", identifier: "nocturne.edit.select_all" });
    if has_selection && is_writable {
        items.push(EditMenuItemSpec { action: EditMenuAction::Delete, title: "Delete", identifier: "nocturne.edit.delete" });
    }
    items
}

/// Horizontal tolerance around `caret_rect` (in logical points) for treating a long-press as targeting text editing.
pub const EDIT_MENU_CARET_HIT_PAD_X: f32 = 120.0;

/// Vertical tolerance around `caret_rect` (in logical points) for treating a long-press as targeting text editing.
pub const EDIT_MENU_CARET_HIT_PAD_Y: f32 = 48.0;

/// Returns whether a long-press should open the native edit menu rather than a canvas context menu.
#[must_use]
pub const fn should_present_native_edit_menu(has_caret: bool, long_press_triggered: bool) -> bool {
    has_caret && long_press_triggered
}

/// Returns `true` when `long_press_pos` lies within a bounded hit region around `caret_rect`
/// (clamped to `page_rect` when provided), preventing long-presses on distant empty canvas,
/// images, containers, or the sidebar from hijacking `PointerButton::Secondary` context menus.
#[must_use]
pub fn is_long_press_near_caret(long_press_pos: [f32; 2], caret_rect: Option<[f32; 4]>, page_rect: Option<[f32; 4]>) -> bool {
    let [lx, ly] = long_press_pos;
    let Some([cx, cy, cw, ch]) = caret_rect else {
        return false;
    };
    if !(lx.is_finite() && ly.is_finite() && cx.is_finite() && cy.is_finite() && cw.is_finite() && ch.is_finite()) || cw < 0.0 || ch < 0.0 {
        return false;
    }

    let mut min_x = cx - EDIT_MENU_CARET_HIT_PAD_X;
    let mut max_x = cx + cw + EDIT_MENU_CARET_HIT_PAD_X;
    let mut min_y = cy - EDIT_MENU_CARET_HIT_PAD_Y;
    let mut max_y = cy + ch + EDIT_MENU_CARET_HIT_PAD_Y;

    if let Some([px, py, pw, ph]) = page_rect {
        if !(px.is_finite() && py.is_finite() && pw.is_finite() && ph.is_finite()) || pw < 0.0 || ph < 0.0 {
            return false;
        }
        // When `keyboard_caret_rect()` falls back to the entire `page_rect`, there is no localized
        // on-screen caret rect; do not hijack canvas long-presses across the whole page.
        if (cx - px).abs() < 0.5 && (cy - py).abs() < 0.5 && (cw - pw).abs() < 0.5 && (ch - ph).abs() < 0.5 {
            return false;
        }
        let p_max_x = px + pw;
        let p_max_y = py + ph;
        if lx < px || lx > p_max_x || ly < py || ly > p_max_y {
            return false;
        }
        min_x = min_x.max(px);
        max_x = max_x.min(p_max_x);
        min_y = min_y.max(py);
        max_y = max_y.min(p_max_y);
    }

    lx >= min_x && lx <= max_x && ly >= min_y && ly <= max_y
}

/// Resolves the anchor rectangle `[x, y, width, height]` in view points for edit-menu presentation.
///
/// Prefers a finite non-negative `caret_rect`, otherwise falls back to a `2x2` rect centered on
/// a finite `touch_pos`, or `[0.0, 0.0, 1.0, 1.0]` when neither is valid.
#[must_use]
pub fn compute_menu_target_rect(caret_rect: Option<[f32; 4]>, touch_pos: Option<[f32; 2]>) -> [f32; 4] {
    if let Some([x, y, w, h]) = caret_rect
        && x.is_finite()
        && y.is_finite()
        && w.is_finite()
        && h.is_finite()
        && w >= 0.0
        && h >= 0.0
    {
        return [x, y, w, h];
    }
    if let Some([tx, ty]) = touch_pos
        && tx.is_finite()
        && ty.is_finite()
    {
        return [tx - 1.0, ty - 1.0, 2.0, 2.0];
    }
    [0.0, 0.0, 1.0, 1.0]
}

/// Shared mutable state between the UIKit edit-menu delegate and the `eframe` loop.
#[derive(Debug, Clone, PartialEq)]
pub struct EditMenuBridgeState {
    pub capabilities: EditMenuCapabilities,
    pub last_target_rect: [f32; 4],
    pending_actions: Vec<EditMenuAction>,
}

impl Default for EditMenuBridgeState {
    fn default() -> Self {
        Self::new()
    }
}

impl EditMenuBridgeState {
    /// Creates an empty edit-menu bridge state with a safe `[0, 0, 1, 1]` target rect.
    #[must_use]
    pub fn new() -> Self {
        Self { capabilities: EditMenuCapabilities::default(), last_target_rect: [0.0, 0.0, 1.0, 1.0], pending_actions: Vec::new() }
    }

    /// Records the target rectangle and capabilities for the next menu presentation.
    pub fn set_presentation(&mut self, target_rect: [f32; 4], capabilities: EditMenuCapabilities) {
        self.last_target_rect = compute_menu_target_rect(Some(target_rect), None);
        self.capabilities = capabilities;
    }

    /// Returns the menu items matching the currently stored capabilities.
    #[must_use]
    pub fn menu_items(&self) -> Vec<EditMenuItemSpec> {
        build_menu_items(self.capabilities.has_selection, self.capabilities.can_paste, self.capabilities.is_writable)
    }

    /// Enqueues an action triggered by the user from the native edit menu.
    pub fn queue_action(&mut self, action: EditMenuAction) {
        self.pending_actions.push(action);
    }

    /// Drains all queued edit-menu actions in FIFO order.
    pub fn drain_actions(&mut self) -> Vec<EditMenuAction> {
        std::mem::take(&mut self.pending_actions)
    }
}

#[cfg(target_os = "ios")]
pub mod ios {
    use super::{EditMenuAction, EditMenuBridgeState, EditMenuCapabilities, compute_menu_target_rect};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, available, define_class, msg_send};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSArray, NSString};
    #[allow(deprecated)]
    use objc2_ui_kit::UIMenuController;
    use objc2_ui_kit::{
        UIAction, UIEditMenuConfiguration, UIEditMenuInteraction, UIEditMenuInteractionDelegate, UIInteraction, UIMenu, UIMenuElement, UIView,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::rc::Rc;

    fn to_cg_rect(rect: [f32; 4]) -> CGRect {
        CGRect {
            origin: CGPoint { x: f64::from(rect[0]), y: f64::from(rect[1]) },
            size: CGSize { width: f64::from(rect[2].max(1.0)), height: f64::from(rect[3].max(1.0)) },
        }
    }

    pub(super) struct EditMenuDelegateIvars {
        state: Rc<RefCell<EditMenuBridgeState>>,
        egui_ctx: egui::Context,
    }

    #[allow(unsafe_code)]
    // SAFETY:
    // - `NSObject` has no subclassing invariants beyond valid Objective-C inheritance.
    // - `NocturneEditMenuDelegate` is `MainThreadOnly`, does not implement `Drop`, and only borrows
    //   `Rc<RefCell<EditMenuBridgeState>>` transiently inside non-reentrant UIKit delegate callbacks.
    mod delegate_class {
        use super::*;
        use objc2::rc::Allocated;

        define_class!(
            #[unsafe(super(NSObject))]
            #[thread_kind = MainThreadOnly]
            #[ivars = EditMenuDelegateIvars]
            pub(super) struct NocturneEditMenuDelegate;

            unsafe impl NSObjectProtocol for NocturneEditMenuDelegate {}

            unsafe impl UIEditMenuInteractionDelegate for NocturneEditMenuDelegate {
                #[unsafe(method_id(editMenuInteraction:menuForConfiguration:suggestedActions:))]
                fn __menu_for_configuration(
                    &self,
                    _interaction: &UIEditMenuInteraction,
                    _configuration: &UIEditMenuConfiguration,
                    _suggested_actions: &NSArray<UIMenuElement>,
                ) -> Option<Retained<UIMenu>> {
                    let mtm = self.mtm();
                    let specs = self.ivars().state.try_borrow().map(|s| s.menu_items()).unwrap_or_default();
                    let mut elements: Vec<Retained<UIMenuElement>> = Vec::with_capacity(specs.len());
                    for spec in specs {
                        let action_kind = spec.action;
                        let state_ref = Rc::clone(&self.ivars().state);
                        let egui_ctx = self.ivars().egui_ctx.clone();
                        let handler = RcBlock::new(move |_ui_action: NonNull<UIAction>| {
                            if let Ok(mut state) = state_ref.try_borrow_mut() {
                                state.queue_action(action_kind);
                            }
                            // The menu tap lands in UIKit's menu view, not `WinitView`, so without this
                            // the action (and the pasteboard write for Copy) waits for the next touch.
                            egui_ctx.request_repaint();
                        });
                        let title = NSString::from_str(spec.title);
                        let identifier = NSString::from_str(spec.identifier);
                        // SAFETY: `RcBlock::as_ptr(&handler)` is a valid non-null block pointer matching
                        // `UIActionHandler`, and `UIAction` copies the block on the main thread.
                        let ui_action = unsafe {
                            UIAction::actionWithTitle_image_identifier_handler(&title, None, Some(&identifier), RcBlock::as_ptr(&handler), mtm)
                        };
                        elements.push(Retained::into_super(ui_action));
                    }
                    let refs: Vec<&UIMenuElement> = elements.iter().map(|elem| &**elem).collect();
                    let children = NSArray::from_slice(&refs);
                    Some(UIMenu::menuWithChildren(&children, mtm))
                }

                #[unsafe(method(editMenuInteraction:targetRectForConfiguration:))]
                fn __target_rect_for_configuration(&self, _interaction: &UIEditMenuInteraction, _configuration: &UIEditMenuConfiguration) -> CGRect {
                    let rect = self.ivars().state.try_borrow().map(|s| s.last_target_rect).unwrap_or([0.0, 0.0, 1.0, 1.0]);
                    to_cg_rect(rect)
                }
            }
        );

        impl NocturneEditMenuDelegate {
            pub(super) fn new(mtm: MainThreadMarker, state: Rc<RefCell<EditMenuBridgeState>>, egui_ctx: egui::Context) -> Retained<Self> {
                let this: Allocated<Self> = mtm.alloc();
                let this = this.set_ivars(EditMenuDelegateIvars { state, egui_ctx });
                // SAFETY: `NSObject::init` is the designated initializer of superclass `NSObject`.
                unsafe { msg_send![super(this), init] }
            }
        }
    }

    use delegate_class::NocturneEditMenuDelegate;

    /// Bridges native iPadOS / iOS edit menus (`UIEditMenuInteraction` on iOS 16+,
    /// `UIMenuController` fallback on iOS 15.x) to [`EditMenuAction`] events.
    pub struct EditMenuBridge {
        mtm: MainThreadMarker,
        state: Rc<RefCell<EditMenuBridgeState>>,
        interaction: Option<Retained<UIEditMenuInteraction>>,
        _delegate: Option<Retained<NocturneEditMenuDelegate>>,
    }

    impl EditMenuBridge {
        /// Attaches the native edit-menu interaction (iOS 16+) or prepares the iOS 15 fallback on `view`.
        pub fn attach(view: &UIView, mtm: MainThreadMarker, egui_ctx: &egui::Context) -> Self {
            let state = Rc::new(RefCell::new(EditMenuBridgeState::new()));
            if available!(ios = 16.0) {
                let delegate = NocturneEditMenuDelegate::new(mtm, Rc::clone(&state), egui_ctx.clone());
                let delegate_proto = ProtocolObject::from_ref(&*delegate);
                let interaction = UIEditMenuInteraction::initWithDelegate(mtm.alloc(), Some(delegate_proto));
                view.addInteraction(ProtocolObject::from_ref(&*interaction));
                Self { mtm, state, interaction: Some(interaction), _delegate: Some(delegate) }
            } else {
                Self { mtm, state, interaction: None, _delegate: None }
            }
        }

        /// Presents the native edit menu anchored to `target_rect` with the given capabilities.
        pub fn present(&self, view: &UIView, target_rect: egui::Rect, has_selection: bool, can_paste: bool, is_writable: bool) {
            let rect_opt = (target_rect.min.x.is_finite()
                && target_rect.min.y.is_finite()
                && target_rect.width().is_finite()
                && target_rect.height().is_finite())
            .then(|| [target_rect.min.x, target_rect.min.y, target_rect.width().max(0.0), target_rect.height().max(0.0)]);
            let resolved_rect = compute_menu_target_rect(rect_opt, None);
            self.state.borrow_mut().set_presentation(resolved_rect, EditMenuCapabilities { has_selection, can_paste, is_writable });

            if available!(ios = 16.0)
                && let Some(ref interaction) = self.interaction
            {
                let source_point =
                    CGPoint { x: f64::from(resolved_rect[0] + resolved_rect[2] * 0.5), y: f64::from(resolved_rect[1] + resolved_rect[3] * 0.5) };
                #[allow(unsafe_code)]
                // SAFETY: `self.mtm` guarantees UIKit main-thread execution and passing `None` for
                // `identifier` is documented to let UIKit generate a default configuration identifier.
                let config = unsafe { UIEditMenuConfiguration::configurationWithIdentifier_sourcePoint(None, source_point, self.mtm) };
                interaction.presentEditMenuWithConfiguration(&config);
            } else {
                #[allow(deprecated)]
                let controller = UIMenuController::sharedMenuController(self.mtm);
                #[allow(deprecated)]
                controller.setMenuItems(None);
                #[allow(deprecated)]
                controller.showMenuFromView_rect(view, to_cg_rect(resolved_rect));
            }
        }

        /// Dismisses any visible native edit menu.
        pub fn dismiss(&self, view: &UIView) {
            if available!(ios = 16.0)
                && let Some(ref interaction) = self.interaction
            {
                interaction.dismissMenu();
            } else {
                #[allow(deprecated)]
                let controller = UIMenuController::sharedMenuController(self.mtm);
                #[allow(deprecated)]
                controller.hideMenuFromView(view);
            }
        }

        /// Drains all edit-menu actions selected since the last poll.
        pub fn drain_actions(&self) -> Vec<EditMenuAction> {
            self.state.borrow_mut().drain_actions()
        }
    }

    impl Drop for EditMenuBridge {
        fn drop(&mut self) {
            if let Some(ref interaction) = self.interaction {
                let interaction_proto: &ProtocolObject<dyn UIInteraction> = ProtocolObject::from_ref(&**interaction);
                if let Some(view) = interaction_proto.view() {
                    view.removeInteraction(interaction_proto);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EditMenuAction, EditMenuBridgeState, EditMenuCapabilities, build_menu_items, compute_menu_target_rect, is_long_press_near_caret,
        should_present_native_edit_menu,
    };

    fn actions_of(has_selection: bool, can_paste: bool, is_writable: bool) -> Vec<EditMenuAction> {
        build_menu_items(has_selection, can_paste, is_writable).into_iter().map(|spec| spec.action).collect()
    }

    #[test]
    fn menu_items_matrix_writable_and_read_only() {
        assert_eq!(
            actions_of(true, true, true),
            vec![EditMenuAction::Cut, EditMenuAction::Copy, EditMenuAction::Paste, EditMenuAction::SelectAll, EditMenuAction::Delete]
        );
        assert_eq!(actions_of(true, false, true), vec![EditMenuAction::Cut, EditMenuAction::Copy, EditMenuAction::SelectAll, EditMenuAction::Delete]);
        assert_eq!(actions_of(false, true, true), vec![EditMenuAction::Paste, EditMenuAction::SelectAll]);
        assert_eq!(actions_of(false, false, true), vec![EditMenuAction::SelectAll]);

        // Read-only surfaces must never expose Cut, Paste, or Delete.
        assert_eq!(actions_of(true, true, false), vec![EditMenuAction::Copy, EditMenuAction::SelectAll]);
        assert_eq!(actions_of(false, true, false), vec![EditMenuAction::SelectAll]);
    }

    #[test]
    fn menu_item_specs_have_non_empty_titles_identifiers_and_selectors() {
        let items = build_menu_items(true, true, true);
        assert_eq!(items.len(), 5);
        for item in items {
            assert!(!item.title.is_empty());
            assert!(item.identifier.starts_with("nocturne.edit."));
            assert!(item.action.selector_name().ends_with(':'));
        }
    }

    #[test]
    fn native_edit_menu_requires_both_caret_and_long_press() {
        assert!(should_present_native_edit_menu(true, true));
        assert!(!should_present_native_edit_menu(false, true));
        assert!(!should_present_native_edit_menu(true, false));
        assert!(!should_present_native_edit_menu(false, false));
    }

    #[test]
    fn target_rect_prefers_finite_caret_then_touch_then_safe_default() {
        assert_eq!(compute_menu_target_rect(Some([120.0, 240.0, 2.0, 18.0]), Some([50.0, 60.0])), [120.0, 240.0, 2.0, 18.0]);
        assert_eq!(compute_menu_target_rect(Some([120.0, 240.0, 0.0, 18.0]), Some([50.0, 60.0])), [120.0, 240.0, 0.0, 18.0]);
        assert_eq!(compute_menu_target_rect(None, Some([50.0, 60.0])), [49.0, 59.0, 2.0, 2.0]);
        assert_eq!(compute_menu_target_rect(Some([f32::NAN, 10.0, 2.0, 10.0]), Some([50.0, 60.0])), [49.0, 59.0, 2.0, 2.0]);
        assert_eq!(compute_menu_target_rect(Some([10.0, 10.0, -2.0, 10.0]), Some([50.0, 60.0])), [49.0, 59.0, 2.0, 2.0]);
        assert_eq!(compute_menu_target_rect(Some([f32::INFINITY, 10.0, 2.0, 10.0]), Some([f32::NAN, 60.0])), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(compute_menu_target_rect(None, None), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn bridge_state_queues_and_drains_in_fifo_order() {
        let mut state = EditMenuBridgeState::new();
        assert_eq!(state.last_target_rect, [0.0, 0.0, 1.0, 1.0]);
        assert!(state.drain_actions().is_empty());

        state.set_presentation([40.0, 80.0, 3.0, 20.0], EditMenuCapabilities { has_selection: true, can_paste: false, is_writable: true });
        assert_eq!(state.last_target_rect, [40.0, 80.0, 3.0, 20.0]);
        assert_eq!(
            state.menu_items().into_iter().map(|spec| spec.action).collect::<Vec<_>>(),
            vec![EditMenuAction::Cut, EditMenuAction::Copy, EditMenuAction::SelectAll, EditMenuAction::Delete]
        );

        state.queue_action(EditMenuAction::Copy);
        state.queue_action(EditMenuAction::SelectAll);
        assert_eq!(state.drain_actions(), vec![EditMenuAction::Copy, EditMenuAction::SelectAll]);
        assert!(state.drain_actions().is_empty());

        // Non-finite presentation rect sanitizes back to the safe default.
        state.set_presentation([f32::NAN, 80.0, 3.0, 20.0], EditMenuCapabilities::default());
        assert_eq!(state.last_target_rect, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn long_press_proximity_gates_native_edit_menu_to_caret_vicinity() {
        let caret = Some([500.0, 268.0, 2.0, 17.0]);
        let page = Some([486.0, 209.0, 522.0, 517.0]);

        // Near caret inside page_rect -> true.
        assert!(is_long_press_near_caret([500.0, 275.0], caret, page));
        assert!(is_long_press_near_caret([590.0, 300.0], caret, page));

        // Empty canvas 350pt below caret -> false (lets pagemenu open).
        assert!(!is_long_press_near_caret([747.0, 635.0], caret, page));

        // Sidebar outside page_rect even if horizontally within 120pt of caret -> false.
        assert!(!is_long_press_near_caret([420.0, 275.0], caret, page));

        // Whole-page fallback rect must not hijack long-presses across the entire canvas.
        assert!(!is_long_press_near_caret([747.0, 635.0], page, page));

        // Missing or non-finite caret/pos -> false.
        assert!(!is_long_press_near_caret([500.0, 275.0], None, page));
        assert!(!is_long_press_near_caret([f32::NAN, 275.0], caret, page));
    }
}
