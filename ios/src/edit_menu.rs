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

/// Offscreen position used when releasing egui's held primary press before presenting the native
/// edit menu so lifting the finger never taps through and clears the active text selection.
pub const EDIT_MENU_OFFSCREEN_RELEASE_POS: egui::Pos2 = egui::pos2(-10_000.0, -10_000.0);

/// Resolves the anchor rectangle `[x, y, width, height]` in logical points for edit-menu presentation.
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

/// Resolves the edit-menu anchor rectangle from egui logical points into UIKit view points using
/// `egui::Context::zoom_factor()` (`uikit_pts = egui_pts * zoom_factor`).
///
/// Non-finite or non-positive `zoom_factor` values fall back to `1.0`.
#[must_use]
pub fn compute_menu_target_rect_in_view_points(caret_rect: Option<[f32; 4]>, touch_pos: Option<[f32; 2]>, zoom_factor: f32) -> [f32; 4] {
    let has_valid_caret =
        caret_rect.is_some_and(|[x, y, w, h]| x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0);
    let has_valid_touch = touch_pos.is_some_and(|[tx, ty]| tx.is_finite() && ty.is_finite());
    if !has_valid_caret && !has_valid_touch {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let [x, y, w, h] = compute_menu_target_rect(caret_rect, touch_pos);
    let z = if zoom_factor.is_finite() && zoom_factor > 0.0 { zoom_factor } else { 1.0 };
    let scaled = [x * z, y * z, w * z, h * z];
    if scaled.iter().all(|v| v.is_finite()) { scaled } else { [0.0, 0.0, 1.0, 1.0] }
}

/// Tracks the held finger that triggered a long-press near the text caret so presenting the native
/// `UIEditMenuInteraction` releases egui's primary press offscreen and swallows the finger's
/// remaining moves and lift without emitting a `PointerButton::Secondary` click or tapping through
/// and clearing the active text selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditMenuTouchConsumer {
    active_finger_touches: std::collections::BTreeSet<egui::TouchId>,
    consumed_touch: Option<egui::TouchId>,
    suppress_pointer_until_release: bool,
}

impl EditMenuTouchConsumer {
    /// Creates an idle [`EditMenuTouchConsumer`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` while a held long-press finger is being suppressed until it lifts.
    #[must_use]
    pub fn is_consuming_finger(&self) -> bool {
        self.consumed_touch.is_some() || self.suppress_pointer_until_release
    }

    /// Consumes the currently held finger right when the native edit menu is presented.
    ///
    /// Releases egui's primary press offscreen (`EDIT_MENU_OFFSCREEN_RELEASE_POS`) without firing
    /// a `Secondary` click, and arms suppression for that finger's remaining move/end events until
    /// it lifts.
    pub fn consume_held_finger(&mut self, events: &mut Vec<egui::Event>) {
        events.extend([
            egui::Event::PointerMoved(EDIT_MENU_OFFSCREEN_RELEASE_POS),
            egui::Event::PointerButton {
                pos: EDIT_MENU_OFFSCREEN_RELEASE_POS,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerGone,
        ]);
        self.consumed_touch = self.active_finger_touches.iter().next().copied();
        self.suppress_pointer_until_release = true;
    }

    /// Filters a frame's `egui::RawInput::events` before `TouchLongPress::update`.
    ///
    /// - Keeps `active_finger_touches` synchronized for non-stylus touches.
    /// - While a long-press finger is consumed: drops its moves and primary pointer events, turns
    ///   its `TouchPhase::End` into `TouchPhase::Cancel` (so `TouchLongPress` and egui never leak
    ///   the contact and never interpret the lift as a tap), and preserves Apple Pencil touches.
    pub fn filter_events(&mut self, events: &mut Vec<egui::Event>) {
        if let Some(consumed_id) = self.consumed_touch {
            let mut lifted = false;
            events.retain_mut(|event| match event {
                egui::Event::Touch { id, phase, .. } if *id == consumed_id => match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::Move => false,
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                        *phase = egui::TouchPhase::Cancel;
                        lifted = true;
                        true
                    }
                },
                egui::Event::Touch { id, phase, force: None, .. } => {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.active_finger_touches.insert(*id);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.active_finger_touches.remove(id);
                        }
                    }
                    true
                }
                egui::Event::PointerMoved(_) | egui::Event::PointerButton { button: egui::PointerButton::Primary, .. } => false,
                _ => true,
            });
            if lifted {
                self.consumed_touch = None;
                self.suppress_pointer_until_release = false;
                self.active_finger_touches.remove(&consumed_id);
            }
            return;
        }

        if self.suppress_pointer_until_release {
            let mut released = false;
            events.retain(|event| match event {
                egui::Event::Touch { id, phase, force: None, .. } => {
                    match phase {
                        egui::TouchPhase::Start => {
                            self.active_finger_touches.insert(*id);
                            released = true;
                        }
                        egui::TouchPhase::Move => {
                            self.active_finger_touches.insert(*id);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.active_finger_touches.remove(id);
                        }
                    }
                    true
                }
                egui::Event::PointerMoved(_) => released,
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed, .. } => {
                    if !*pressed {
                        released = true;
                        return false;
                    }
                    released
                }
                egui::Event::PointerGone => {
                    released = true;
                    true
                }
                _ => true,
            });
            if released {
                self.suppress_pointer_until_release = false;
            }
            return;
        }

        for event in events.iter() {
            if let egui::Event::Touch { id, phase, force: None, .. } = event {
                match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::Move => {
                        self.active_finger_touches.insert(*id);
                    }
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                        self.active_finger_touches.remove(id);
                    }
                }
            }
        }
    }
}

/// Shared mutable state between the UIKit edit-menu delegate and the `eframe` loop.
#[derive(Debug, Clone, PartialEq)]
pub struct EditMenuBridgeState {
    pub capabilities: EditMenuCapabilities,
    pub last_target_rect: [f32; 4],
    pending_actions: Vec<EditMenuAction>,
    touch_consumer: EditMenuTouchConsumer,
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
        Self {
            capabilities: EditMenuCapabilities::default(),
            last_target_rect: [0.0, 0.0, 1.0, 1.0],
            pending_actions: Vec::new(),
            touch_consumer: EditMenuTouchConsumer::new(),
        }
    }

    /// Records the target rectangle (already in view points) and capabilities for the next menu presentation.
    pub fn set_presentation(&mut self, target_rect: [f32; 4], capabilities: EditMenuCapabilities) {
        self.last_target_rect = compute_menu_target_rect(Some(target_rect), None);
        self.capabilities = capabilities;
    }

    /// Records an egui-logical target rectangle scaled by `zoom_factor` into UIKit view points.
    pub fn set_presentation_scaled(&mut self, target_rect_egui: [f32; 4], zoom_factor: f32, capabilities: EditMenuCapabilities) {
        self.last_target_rect = compute_menu_target_rect_in_view_points(Some(target_rect_egui), None, zoom_factor);
        self.capabilities = capabilities;
    }

    /// Filters a frame's events to track active fingers and drop post-presentation move/lift
    /// events from a consumed long-press finger.
    pub fn filter_events(&mut self, events: &mut Vec<egui::Event>) {
        self.touch_consumer.filter_events(events);
    }

    /// Consumes the currently held long-press finger so lifting it does not tap through to egui.
    pub fn consume_held_finger(&mut self, events: &mut Vec<egui::Event>) {
        self.touch_consumer.consume_held_finger(events);
    }

    /// Returns `true` while a consumed long-press finger is still down.
    #[must_use]
    pub fn is_consuming_finger(&self) -> bool {
        self.touch_consumer.is_consuming_finger()
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
    use super::{EditMenuAction, EditMenuBridgeState, EditMenuCapabilities, compute_menu_target_rect_in_view_points};
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

        /// Filters a frame's `raw_input.events` before `TouchLongPress::update` so a held finger
        /// consumed during edit-menu presentation cannot tap through and clear the text selection.
        pub fn filter_raw_input(&self, raw_input: &mut egui::RawInput) {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.filter_events(&mut raw_input.events);
            }
        }

        /// Consumes the currently held long-press finger on `raw_input.events` (offscreen primary
        /// release without a `Secondary` click) so lifting it never taps through to egui.
        pub fn consume_held_finger(&self, raw_input: &mut egui::RawInput) {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.consume_held_finger(&mut raw_input.events);
            }
        }

        /// Presents the native edit menu anchored to `target_rect` (in egui logical points,
        /// converted to UIKit view points via `zoom_factor`) with the given capabilities.
        pub fn present(&self, view: &UIView, target_rect: egui::Rect, zoom_factor: f32, has_selection: bool, can_paste: bool, is_writable: bool) {
            let rect_opt = (target_rect.min.x.is_finite()
                && target_rect.min.y.is_finite()
                && target_rect.width().is_finite()
                && target_rect.height().is_finite())
            .then(|| [target_rect.min.x, target_rect.min.y, target_rect.width().max(0.0), target_rect.height().max(0.0)]);
            let resolved_rect = compute_menu_target_rect_in_view_points(rect_opt, None, zoom_factor);
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

        /// Consumes the held long-press finger on `raw_input` and presents the native edit menu
        /// anchored to `target_rect` (scaled by `zoom_factor` into UIKit view points).
        pub fn present_for_long_press(
            &self,
            view: &UIView,
            target_rect: egui::Rect,
            zoom_factor: f32,
            has_selection: bool,
            can_paste: bool,
            is_writable: bool,
            raw_input: &mut egui::RawInput,
        ) {
            self.consume_held_finger(raw_input);
            self.present(view, target_rect, zoom_factor, has_selection, can_paste, is_writable);
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

    #[test]
    fn target_rect_in_view_points_scales_by_zoom_factor_and_sanitizes_bad_zoom() {
        use super::compute_menu_target_rect_in_view_points;

        assert_eq!(compute_menu_target_rect_in_view_points(Some([120.0, 240.0, 2.0, 18.0]), None, 1.0), [120.0, 240.0, 2.0, 18.0]);
        assert_eq!(compute_menu_target_rect_in_view_points(Some([120.0, 240.0, 2.0, 18.0]), None, 1.25), [150.0, 300.0, 2.5, 22.5]);
        assert_eq!(compute_menu_target_rect_in_view_points(None, Some([50.0, 60.0]), 1.5), [73.5, 88.5, 3.0, 3.0]);
        assert_eq!(compute_menu_target_rect_in_view_points(None, None, 1.25), [0.0, 0.0, 1.0, 1.0]);

        for bad_zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(compute_menu_target_rect_in_view_points(Some([120.0, 240.0, 2.0, 18.0]), None, bad_zoom), [120.0, 240.0, 2.0, 18.0]);
        }

        let mut state = EditMenuBridgeState::new();
        state.set_presentation_scaled(
            [80.0, 160.0, 4.0, 20.0],
            1.25,
            EditMenuCapabilities { has_selection: true, can_paste: true, is_writable: true },
        );
        assert_eq!(state.last_target_rect, [100.0, 200.0, 5.0, 25.0]);
    }

    fn touch(id: u64, phase: egui::TouchPhase, pos: egui::Pos2, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos, force }
    }

    fn primary_btn(pressed: bool, pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn edit_menu_consumes_held_finger_without_secondary_click_or_tap_through_on_release() {
        use super::EDIT_MENU_OFFSCREEN_RELEASE_POS;

        let mut state = EditMenuBridgeState::new();
        let pos = egui::pos2(240.0, 280.0);

        // Frame 0: finger presses down on selected text.
        let mut down = vec![touch(1, egui::TouchPhase::Start, pos, None), egui::Event::PointerMoved(pos), primary_btn(true, pos)];
        state.filter_events(&mut down);
        assert_eq!(down.len(), 3);
        assert!(!state.is_consuming_finger());

        // Frame 1: long-press fires near caret -> present native edit menu and consume held finger.
        let mut present_frame = Vec::new();
        state.filter_events(&mut present_frame);
        state.consume_held_finger(&mut present_frame);
        assert!(state.is_consuming_finger());
        assert_eq!(
            present_frame,
            vec![
                egui::Event::PointerMoved(EDIT_MENU_OFFSCREEN_RELEASE_POS),
                primary_btn(false, EDIT_MENU_OFFSCREEN_RELEASE_POS),
                egui::Event::PointerGone,
            ],
            "must release primary offscreen without emitting a Secondary click"
        );

        // Frame 2: held finger wobbles slightly while user looks at native edit menu -> dropped,
        // while an Apple Pencil touch passes through untouched.
        let pencil_move = touch(9, egui::TouchPhase::Move, egui::pos2(400.0, 400.0), Some(0.5));
        let mut wobble = vec![
            touch(1, egui::TouchPhase::Move, pos + egui::vec2(2.0, 1.0), None),
            egui::Event::PointerMoved(pos + egui::vec2(2.0, 1.0)),
            pencil_move.clone(),
        ];
        state.filter_events(&mut wobble);
        assert_eq!(wobble, vec![pencil_move]);
        assert!(state.is_consuming_finger());

        // Frame 3: finger lifts -> Touch::End rewritten to Cancel, primary release dropped (no tap-through!).
        let mut lift = vec![touch(1, egui::TouchPhase::End, pos, None), primary_btn(false, pos), egui::Event::PointerGone];
        state.filter_events(&mut lift);
        assert_eq!(lift, vec![touch(1, egui::TouchPhase::Cancel, pos, None), egui::Event::PointerGone]);
        assert!(!state.is_consuming_finger());

        // Frame 4: next ordinary tap passes through cleanly.
        let mut next_tap = vec![touch(2, egui::TouchPhase::Start, pos, None), egui::Event::PointerMoved(pos), primary_btn(true, pos)];
        state.filter_events(&mut next_tap);
        assert_eq!(next_tap.len(), 3);
    }

    #[test]
    fn edit_menu_held_finger_consumption_prevents_egui_click_on_release() {
        let ctx = egui::Context::default();
        let mut lp = nocturne_ui_egui::touch_ux::TouchLongPress::default();
        let mut menu_state = EditMenuBridgeState::new();
        let target_pos = egui::pos2(160.0, 120.0);

        let run_frame = |time: f64,
                         mut events: Vec<egui::Event>,
                         lp: &mut nocturne_ui_egui::touch_ux::TouchLongPress,
                         menu: &mut EditMenuBridgeState|
         -> (bool, bool) {
            menu.filter_events(&mut events);
            lp.filter_consumed(&mut events);
            let mut presented = false;
            if lp.update(time, &events).is_some() {
                menu.consume_held_finger(&mut events);
                presented = true;
            }
            let mut clicked = false;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                let rect = egui::Rect::from_center_size(target_pos, egui::vec2(200.0, 60.0));
                clicked = ui.interact(rect, ui.scope_id().with("selected-text-block"), egui::Sense::click()).clicked();
            });
            out.textures_delta.clear();
            (presented, clicked)
        };

        // Warmup layout frame.
        assert_eq!(run_frame(0.0, Vec::new(), &mut lp, &mut menu_state), (false, false));

        // Touch down over selected text block.
        assert_eq!(
            run_frame(
                0.05,
                vec![touch(1, egui::TouchPhase::Start, target_pos, None), egui::Event::PointerMoved(target_pos), primary_btn(true, target_pos)],
                &mut lp,
                &mut menu_state,
            ),
            (false, false)
        );

        // Long-press fires at t = 0.60 -> native edit menu presented, held finger consumed, NO click!
        assert_eq!(run_frame(0.60, Vec::new(), &mut lp, &mut menu_state), (true, false));

        // Finger lifts at t = 0.72 -> MUST NOT click the text block (which would clear text selection)!
        assert_eq!(
            run_frame(
                0.72,
                vec![touch(1, egui::TouchPhase::End, target_pos, None), primary_btn(false, target_pos), egui::Event::PointerGone],
                &mut lp,
                &mut menu_state,
            ),
            (false, false)
        );

        // Subsequent deliberate tap still clicks cleanly.
        assert_eq!(
            run_frame(
                1.00,
                vec![touch(2, egui::TouchPhase::Start, target_pos, None), egui::Event::PointerMoved(target_pos), primary_btn(true, target_pos)],
                &mut lp,
                &mut menu_state,
            ),
            (false, false)
        );
        assert_eq!(
            run_frame(
                1.08,
                vec![touch(2, egui::TouchPhase::End, target_pos, None), primary_btn(false, target_pos), egui::Event::PointerGone],
                &mut lp,
                &mut menu_state,
            ),
            (false, true)
        );
    }

    #[test]
    fn edit_menu_touch_consumer_pointer_only_fallback_drops_release_and_unblocks_next_tap() {
        use super::EditMenuTouchConsumer;

        let mut consumer = EditMenuTouchConsumer::new();
        let pos = egui::pos2(100.0, 100.0);

        // Consume without prior Touch::Start (fallback path).
        let mut frame1 = Vec::new();
        consumer.consume_held_finger(&mut frame1);
        assert!(consumer.is_consuming_finger());

        let mut wobble = vec![egui::Event::PointerMoved(pos + egui::vec2(1.0, 1.0))];
        consumer.filter_events(&mut wobble);
        assert!(wobble.is_empty());
        assert!(consumer.is_consuming_finger());

        let mut release = vec![primary_btn(false, pos), egui::Event::PointerGone];
        consumer.filter_events(&mut release);
        assert_eq!(release, vec![egui::Event::PointerGone]);
        assert!(!consumer.is_consuming_finger());
    }
}
