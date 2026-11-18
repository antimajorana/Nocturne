//! Apple Pencil double-tap and squeeze interaction bridge (`UIPencilInteraction`).
//!
//! Splits pure-Rust action/phase resolution and pending-toggle accounting (testable on all
//! hosts) from the iPadOS-only `#[cfg(target_os = "ios")]` `UIPencilInteractionDelegate` glue.

/// Pure-Rust mirror of `UIKit`'s `UIPencilPreferredAction` settings enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PencilPreferredAction {
    /// User disabled Apple Pencil double-tap/squeeze or selected "Off" in Settings.
    Ignore,
    /// Switch between the current tool and the eraser.
    SwitchEraser,
    /// Switch between the current tool and the previously used writing tool.
    SwitchPrevious,
    /// Show/hide the color palette (unsupported in Nocturne's Pencil bridge).
    ShowColorPalette,
    /// Show the ink attributes palette (unsupported in Nocturne's Pencil bridge).
    ShowInkAttributes,
    /// Show a contextual palette (iPadOS 17.5+; unsupported in Nocturne's Pencil bridge).
    ShowContextualPalette,
    /// Run a system shortcut (iPadOS 17.5+; handled by system / unsupported in-app).
    RunSystemShortcut,
    /// Forward-compatible fallback for unrecognized future `UIPencilPreferredAction` values.
    Unknown(isize),
}

impl PencilPreferredAction {
    /// Converts a raw `NSInteger` (`isize`) `UIPencilPreferredAction` discriminant.
    #[must_use]
    pub const fn from_raw(raw: isize) -> Self {
        match raw {
            0 => Self::Ignore,
            1 => Self::SwitchEraser,
            2 => Self::SwitchPrevious,
            3 => Self::ShowColorPalette,
            4 => Self::ShowInkAttributes,
            5 => Self::ShowContextualPalette,
            6 => Self::RunSystemShortcut,
            other => Self::Unknown(other),
        }
    }
}

/// Pure-Rust mirror of `UIKit`'s `UIPencilInteractionPhase` (iPadOS 17.5+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PencilSqueezePhase {
    /// Squeeze gesture began.
    Began,
    /// Continuous squeeze gesture updated.
    Changed,
    /// Squeeze gesture completed (or discrete gesture recognized).
    Ended,
    /// Squeeze gesture was cancelled.
    Cancelled,
    /// Forward-compatible fallback for unrecognized future `UIPencilInteractionPhase` values.
    Unknown(usize),
}

impl PencilSqueezePhase {
    /// Converts a raw `NSUInteger` (`usize`) `UIPencilInteractionPhase` discriminant.
    #[must_use]
    pub const fn from_raw(raw: usize) -> Self {
        match raw {
            0 => Self::Began,
            1 => Self::Changed,
            2 => Self::Ended,
            3 => Self::Cancelled,
            other => Self::Unknown(other),
        }
    }
}

/// Platform-neutral command produced by an Apple Pencil tap or squeeze interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PencilCommand {
    /// Dispatch `NocturneApp::toggle_eraser_tool()` (`ink.toggle_eraser`).
    ToggleEraser,
    /// Do nothing for this interaction event.
    Ignore,
}

/// Maps a double-tap `UIPencilPreferredAction` to a [`PencilCommand`].
///
/// Both [`PencilPreferredAction::SwitchEraser`] and [`PencilPreferredAction::SwitchPrevious`]
/// map to [`PencilCommand::ToggleEraser`] because Nocturne's `toggle_eraser_tool()` toggles
/// between the eraser and the active/previous writing tool. [`PencilPreferredAction::Ignore`]
/// and unmapped palette/shortcut actions (`ShowColorPalette`, `ShowInkAttributes`,
/// `ShowContextualPalette`, `RunSystemShortcut`, `Unknown`) resolve to [`PencilCommand::Ignore`]
/// so system settings are respected without unexpectedly switching to the eraser.
#[must_use]
pub fn resolve_tap_command(action: PencilPreferredAction) -> PencilCommand {
    match action {
        PencilPreferredAction::SwitchEraser | PencilPreferredAction::SwitchPrevious => PencilCommand::ToggleEraser,
        PencilPreferredAction::Ignore
        | PencilPreferredAction::ShowColorPalette
        | PencilPreferredAction::ShowInkAttributes
        | PencilPreferredAction::ShowContextualPalette
        | PencilPreferredAction::RunSystemShortcut
        | PencilPreferredAction::Unknown(_) => PencilCommand::Ignore,
    }
}

/// Maps a squeeze `(action, phase)` pair to a [`PencilCommand`].
///
/// Only [`PencilSqueezePhase::Ended`] can trigger [`PencilCommand::ToggleEraser`], preventing
/// repeated toggles across `Began` / `Changed` updates and ignoring `Cancelled` gestures.
#[must_use]
pub fn resolve_squeeze_command(action: PencilPreferredAction, phase: PencilSqueezePhase) -> PencilCommand {
    match phase {
        PencilSqueezePhase::Ended => resolve_tap_command(action),
        PencilSqueezePhase::Began | PencilSqueezePhase::Changed | PencilSqueezePhase::Cancelled | PencilSqueezePhase::Unknown(_) => {
            PencilCommand::Ignore
        }
    }
}

/// Accumulates Apple Pencil tap and squeeze events for frame-by-frame draining into `NocturneApp`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PencilBridgeState {
    pending_toggle_eraser: usize,
    tap_events: u64,
    squeeze_events: u64,
}

impl PencilBridgeState {
    /// Creates an empty [`PencilBridgeState`].
    #[must_use]
    pub const fn new() -> Self {
        Self { pending_toggle_eraser: 0, tap_events: 0, squeeze_events: 0 }
    }

    /// Records a double-tap interaction with the system's preferred tap action.
    pub fn record_tap(&mut self, action: PencilPreferredAction) -> PencilCommand {
        self.tap_events = self.tap_events.saturating_add(1);
        let command = resolve_tap_command(action);
        if command == PencilCommand::ToggleEraser {
            self.pending_toggle_eraser = self.pending_toggle_eraser.saturating_add(1);
        }
        command
    }

    /// Records a squeeze interaction phase with the system's preferred squeeze action.
    pub fn record_squeeze(&mut self, action: PencilPreferredAction, phase: PencilSqueezePhase) -> PencilCommand {
        self.squeeze_events = self.squeeze_events.saturating_add(1);
        let command = resolve_squeeze_command(action, phase);
        if command == PencilCommand::ToggleEraser {
            self.pending_toggle_eraser = self.pending_toggle_eraser.saturating_add(1);
        }
        command
    }

    /// Drains and returns the number of queued eraser-toggle commands since the last drain.
    pub fn take_toggle_eraser_count(&mut self) -> usize {
        std::mem::take(&mut self.pending_toggle_eraser)
    }

    /// Returns the number of queued eraser-toggle commands without draining them.
    #[must_use]
    pub const fn pending_toggle_eraser(&self) -> usize {
        self.pending_toggle_eraser
    }

    /// Returns the total number of tap delegate events observed.
    #[must_use]
    pub const fn tap_events(&self) -> u64 {
        self.tap_events
    }

    /// Returns the total number of squeeze delegate events observed.
    #[must_use]
    pub const fn squeeze_events(&self) -> u64 {
        self.squeeze_events
    }
}

#[cfg(target_os = "ios")]
pub mod ios {
    //! UIKit `UIPencilInteraction` and `UIPencilInteractionDelegate` bridge.

    use super::{PencilBridgeState, PencilPreferredAction, PencilSqueezePhase};
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, available, define_class, msg_send};
    use objc2_ui_kit::{
        UIInteraction, UIPencilInteraction, UIPencilInteractionDelegate, UIPencilInteractionPhase, UIPencilInteractionSqueeze,
        UIPencilInteractionTap, UIPencilPreferredAction, UIView,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    fn map_preferred_action(action: UIPencilPreferredAction) -> PencilPreferredAction {
        match action {
            UIPencilPreferredAction::Ignore => PencilPreferredAction::Ignore,
            UIPencilPreferredAction::SwitchEraser => PencilPreferredAction::SwitchEraser,
            UIPencilPreferredAction::SwitchPrevious => PencilPreferredAction::SwitchPrevious,
            UIPencilPreferredAction::ShowColorPalette => PencilPreferredAction::ShowColorPalette,
            UIPencilPreferredAction::ShowInkAttributes => PencilPreferredAction::ShowInkAttributes,
            UIPencilPreferredAction::ShowContextualPalette => PencilPreferredAction::ShowContextualPalette,
            UIPencilPreferredAction::RunSystemShortcut => PencilPreferredAction::RunSystemShortcut,
            other => PencilPreferredAction::Unknown(other.0),
        }
    }

    fn map_squeeze_phase(phase: UIPencilInteractionPhase) -> PencilSqueezePhase {
        match phase {
            UIPencilInteractionPhase::Began => PencilSqueezePhase::Began,
            UIPencilInteractionPhase::Changed => PencilSqueezePhase::Changed,
            UIPencilInteractionPhase::Ended => PencilSqueezePhase::Ended,
            UIPencilInteractionPhase::Cancelled => PencilSqueezePhase::Cancelled,
            other => PencilSqueezePhase::Unknown(other.0),
        }
    }

    struct PencilDelegateIvars {
        state: Rc<RefCell<PencilBridgeState>>,
        egui_ctx: egui::Context,
    }

    define_class!(
        // SAFETY: `NSObject` has no subclassing invariants beyond valid ObjC inheritance, and
        // `NocturnePencilDelegate` is `MainThreadOnly` with an `Rc<RefCell<_>>` ivar and no custom `Drop`.
        #[allow(unsafe_code)]
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = PencilDelegateIvars]
        struct NocturnePencilDelegate;

        // SAFETY: `NocturnePencilDelegate` subclasses `NSObject`, which conforms to `NSObjectProtocol`.
        #[allow(unsafe_code)]
        unsafe impl NSObjectProtocol for NocturnePencilDelegate {}

        // SAFETY: Selector signatures match UIKit's `UIPencilInteractionDelegate` protocol on the main thread.
        #[allow(unsafe_code)]
        unsafe impl UIPencilInteractionDelegate for NocturnePencilDelegate {
            #[allow(deprecated)]
            #[unsafe(method(pencilInteractionDidTap:))]
            fn __pencil_interaction_did_tap(&self, _interaction: &UIPencilInteraction) {
                let mtm = self.mtm();
                let action = map_preferred_action(UIPencilInteraction::preferredTapAction(mtm));
                if let Ok(mut state) = self.ivars().state.try_borrow_mut() {
                    state.record_tap(action);
                }
                // A Pencil tap reaches no winit view, so ask eframe for the frame that drains the toggle
                // instead of leaving it queued until the next touch.
                self.ivars().egui_ctx.request_repaint();
            }

            #[unsafe(method(pencilInteraction:didReceiveTap:))]
            fn __pencil_interaction_did_receive_tap(&self, _interaction: &UIPencilInteraction, _tap: &UIPencilInteractionTap) {
                let mtm = self.mtm();
                let action = map_preferred_action(UIPencilInteraction::preferredTapAction(mtm));
                if let Ok(mut state) = self.ivars().state.try_borrow_mut() {
                    state.record_tap(action);
                }
                self.ivars().egui_ctx.request_repaint();
            }

            #[unsafe(method(pencilInteraction:didReceiveSqueeze:))]
            fn __pencil_interaction_did_receive_squeeze(&self, _interaction: &UIPencilInteraction, squeeze: &UIPencilInteractionSqueeze) {
                if !available!(ios = 17.5) {
                    return;
                }
                let mtm = self.mtm();
                let action = map_preferred_action(UIPencilInteraction::preferredSqueezeAction(mtm));
                let phase = map_squeeze_phase(squeeze.phase());
                if let Ok(mut state) = self.ivars().state.try_borrow_mut() {
                    state.record_squeeze(action, phase);
                }
                self.ivars().egui_ctx.request_repaint();
            }
        }
    );

    impl NocturnePencilDelegate {
        fn new(mtm: MainThreadMarker, state: Rc<RefCell<PencilBridgeState>>, egui_ctx: egui::Context) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(PencilDelegateIvars { state, egui_ctx });
            #[allow(unsafe_code)]
            // SAFETY: `NSObject::init` is the designated initializer of superclass `NSObject`,
            // and `this` has initialized all `PencilDelegateIvars` fields via `set_ivars`.
            unsafe {
                msg_send![super(this), init]
            }
        }
    }

    /// Owns an attached `UIPencilInteraction` and its retained `UIPencilInteractionDelegate`.
    pub struct PencilBridge {
        interaction: Retained<UIPencilInteraction>,
        _delegate: Retained<NocturnePencilDelegate>,
        state: Rc<RefCell<PencilBridgeState>>,
    }

    impl PencilBridge {
        /// Creates a `UIPencilInteraction`, attaches it to `view`, and retains its delegate.
        pub fn attach(view: &UIView, mtm: MainThreadMarker, egui_ctx: &egui::Context) -> Self {
            let state = Rc::new(RefCell::new(PencilBridgeState::default()));
            let delegate = NocturnePencilDelegate::new(mtm, Rc::clone(&state), egui_ctx.clone());
            // `initWithDelegate:` only exists on iPadOS 17.5+, but we deploy to 15.0, so we build the
            // interaction with plain `new` and set the delegate afterwards (both exist since 12.1).
            // The interaction holds the delegate weakly, so the `_delegate` field keeps it alive.
            let interaction = UIPencilInteraction::new(mtm);
            interaction.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.addInteraction(ProtocolObject::from_ref(&*interaction));
            Self { interaction, _delegate: delegate, state }
        }

        /// Drains and returns the number of pending eraser-toggle commands.
        pub fn take_toggle_eraser_count(&self) -> usize {
            self.state.try_borrow_mut().map(|mut state| state.take_toggle_eraser_count()).unwrap_or(0)
        }
    }

    impl Drop for PencilBridge {
        fn drop(&mut self) {
            self.interaction.setDelegate(None);
            if let Some(view) = self.interaction.view() {
                view.removeInteraction(ProtocolObject::from_ref(&*self.interaction));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PencilBridgeState, PencilCommand, PencilPreferredAction, PencilSqueezePhase, resolve_squeeze_command, resolve_tap_command};

    #[test]
    fn tap_switch_eraser_and_switch_previous_toggle_eraser() {
        assert_eq!(resolve_tap_command(PencilPreferredAction::SwitchEraser), PencilCommand::ToggleEraser);
        assert_eq!(resolve_tap_command(PencilPreferredAction::SwitchPrevious), PencilCommand::ToggleEraser);
    }

    #[test]
    fn tap_ignore_and_unmapped_actions_are_ignored() {
        for action in [
            PencilPreferredAction::Ignore,
            PencilPreferredAction::ShowColorPalette,
            PencilPreferredAction::ShowInkAttributes,
            PencilPreferredAction::ShowContextualPalette,
            PencilPreferredAction::RunSystemShortcut,
            PencilPreferredAction::Unknown(-1),
            PencilPreferredAction::Unknown(99),
        ] {
            assert_eq!(resolve_tap_command(action), PencilCommand::Ignore);
        }
    }

    #[test]
    fn squeeze_only_triggers_on_ended_phase() {
        for phase in [PencilSqueezePhase::Began, PencilSqueezePhase::Changed, PencilSqueezePhase::Cancelled, PencilSqueezePhase::Unknown(42)] {
            assert_eq!(resolve_squeeze_command(PencilPreferredAction::SwitchEraser, phase), PencilCommand::Ignore);
            assert_eq!(resolve_squeeze_command(PencilPreferredAction::SwitchPrevious, phase), PencilCommand::Ignore);
        }

        assert_eq!(resolve_squeeze_command(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Ended), PencilCommand::ToggleEraser);
        assert_eq!(resolve_squeeze_command(PencilPreferredAction::SwitchPrevious, PencilSqueezePhase::Ended), PencilCommand::ToggleEraser);
        assert_eq!(resolve_squeeze_command(PencilPreferredAction::Ignore, PencilSqueezePhase::Ended), PencilCommand::Ignore);
        assert_eq!(resolve_squeeze_command(PencilPreferredAction::ShowContextualPalette, PencilSqueezePhase::Ended), PencilCommand::Ignore);
    }

    #[test]
    fn state_accumulates_taps_and_drains_cleanly() {
        let mut state = PencilBridgeState::new();
        assert_eq!(state.tap_events(), 0);
        assert_eq!(state.squeeze_events(), 0);
        assert_eq!(state.pending_toggle_eraser(), 0);
        assert_eq!(state.take_toggle_eraser_count(), 0);

        assert_eq!(state.record_tap(PencilPreferredAction::SwitchEraser), PencilCommand::ToggleEraser);
        assert_eq!(state.record_tap(PencilPreferredAction::Ignore), PencilCommand::Ignore);
        assert_eq!(state.record_tap(PencilPreferredAction::SwitchPrevious), PencilCommand::ToggleEraser);

        assert_eq!(state.tap_events(), 3);
        assert_eq!(state.squeeze_events(), 0);
        assert_eq!(state.pending_toggle_eraser(), 2);

        assert_eq!(state.take_toggle_eraser_count(), 2);
        assert_eq!(state.pending_toggle_eraser(), 0);
        assert_eq!(state.take_toggle_eraser_count(), 0);
        assert_eq!(state.tap_events(), 3);
    }

    #[test]
    fn state_squeeze_lifecycle_toggles_once_on_ended_and_skips_cancelled() {
        let mut state = PencilBridgeState::default();

        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Began), PencilCommand::Ignore);
        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Changed), PencilCommand::Ignore);
        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Changed), PencilCommand::Ignore);
        assert_eq!(state.pending_toggle_eraser(), 0);

        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchEraser, PencilSqueezePhase::Ended), PencilCommand::ToggleEraser);
        assert_eq!(state.squeeze_events(), 4);
        assert_eq!(state.take_toggle_eraser_count(), 1);
        assert_eq!(state.take_toggle_eraser_count(), 0);

        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchPrevious, PencilSqueezePhase::Began), PencilCommand::Ignore);
        assert_eq!(state.record_squeeze(PencilPreferredAction::SwitchPrevious, PencilSqueezePhase::Cancelled), PencilCommand::Ignore);
        assert_eq!(state.squeeze_events(), 6);
        assert_eq!(state.take_toggle_eraser_count(), 0);
    }

    #[test]
    fn raw_discriminant_conversions_cover_known_and_unknown_values() {
        assert_eq!(PencilPreferredAction::from_raw(0), PencilPreferredAction::Ignore);
        assert_eq!(PencilPreferredAction::from_raw(1), PencilPreferredAction::SwitchEraser);
        assert_eq!(PencilPreferredAction::from_raw(2), PencilPreferredAction::SwitchPrevious);
        assert_eq!(PencilPreferredAction::from_raw(3), PencilPreferredAction::ShowColorPalette);
        assert_eq!(PencilPreferredAction::from_raw(4), PencilPreferredAction::ShowInkAttributes);
        assert_eq!(PencilPreferredAction::from_raw(5), PencilPreferredAction::ShowContextualPalette);
        assert_eq!(PencilPreferredAction::from_raw(6), PencilPreferredAction::RunSystemShortcut);
        assert_eq!(PencilPreferredAction::from_raw(-7), PencilPreferredAction::Unknown(-7));

        assert_eq!(PencilSqueezePhase::from_raw(0), PencilSqueezePhase::Began);
        assert_eq!(PencilSqueezePhase::from_raw(1), PencilSqueezePhase::Changed);
        assert_eq!(PencilSqueezePhase::from_raw(2), PencilSqueezePhase::Ended);
        assert_eq!(PencilSqueezePhase::from_raw(3), PencilSqueezePhase::Cancelled);
        assert_eq!(PencilSqueezePhase::from_raw(99), PencilSqueezePhase::Unknown(99));
    }
}
