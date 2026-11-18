//! Pen pressure, as far as the window layer hands it to us.
//!
//! This is the measuring end of the pen and drawing work (`work\requests\pen-and-drawing.md`),
//! not the drawing end. It answers one question: does a real pen's pressure reach this
//! window, and what is it worth?
//!
//! What the stack below us does, read in the sources of the versions we build against
//! (eframe and egui 0.36.2, egui-winit 0.36.2, winit 0.30.13):
//!
//! - **Windows**: winit reads `WM_POINTER`, recognises `PT_PEN` and normalises
//!   `POINTER_PEN_INFO::pressure` over 1024 levels; egui-winit forwards that as
//!   `egui::Event::Touch { force }` beside the ordinary pointer events, so the pointer still
//!   moves the caret and the force arrives alongside it.
//! - **macOS, Linux and Wayland**: winit drops the tablet fields entirely, so no force
//!   arrives and a pen is indistinguishable from a mouse here. Covering those needs a
//!   per-platform reader, which is decision 1 of the request.
//! - **Tilt, barrel rotation and the eraser end**: not exposed by winit on any platform.
//!   The word "tilt" does not appear in its source.
//!
//! So on Windows this reports real pressure, and elsewhere it honestly reports a mouse.
//! Nothing here guesses: a device that sends no force is a mouse and writes at full
//! pressure, which is what a mouse should do.

use std::collections::BTreeMap;

/// What the pen is doing now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// 0 to 1. A mouse, and any device that reports no force, is always 1.
    pub pressure: f32,
    /// A device that reports pressure has been used in this window.
    pub pen: bool,
    /// A contact is on the glass now.
    pub down: bool,
}

/// Which physical input class owns the current pointer interaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerOrigin {
    #[default]
    Mouse,
    Finger,
    Stylus,
}

#[derive(Clone, Copy, Debug)]
struct ContactState {
    is_stylus: bool,
    pressure: f32,
    pos: egui::Pos2,
    #[allow(dead_code)]
    device_id: egui::TouchDeviceId,
}

/// Counters used to distinguish a pressure-capable pen from a mouse-shaped fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub frames: u64,
    pub events: u64,
    pub pointer_moved: u64,
    pub mouse_moved: u64,
    pub pointer_buttons: u64,
    pub touch_events: u64,
    pub touch_with_force: u64,
    pub touch_without_force: u64,
}

impl Default for Sample {
    fn default() -> Self {
        Self { pressure: 1.0, pen: false, down: false }
    }
}

/// Follows the touch events of a window per `TouchId`, isolating stylus contacts from concurrent
/// finger/palm contacts and keeping the latest pen reading.
#[derive(Clone, Debug, Default)]
pub struct Stylus {
    pen: bool,
    contacts: BTreeMap<egui::TouchId, ContactState>,
    pointer_touch: Option<egui::TouchId>,
    highest: f32,
    samples: u64,
    diagnostics: Diagnostics,
    last_event: &'static str,
    /// Where a pen came down on the glass this frame, if one did: a touch that reports a force
    /// as it starts. What the window uses to pick the pen up by itself (issue #9).
    landed: Option<egui::Pos2>,
    finger_landed: Option<(egui::TouchId, egui::Pos2)>,
    any_touch_this_frame: bool,
    finger_ended_frame: bool,
    stylus_ended_frame: bool,
    last_origin: PointerOrigin,
}

impl Stylus {
    /// Takes this frame's input events. Call it once per frame, before anything asks.
    pub fn update(&mut self, events: &[egui::Event]) {
        self.diagnostics.frames += 1;
        self.landed = None;
        self.finger_landed = None;
        self.any_touch_this_frame = false;
        self.finger_ended_frame = false;
        self.stylus_ended_frame = false;
        let mut finite_touch_this_frame = false;
        let mut mouse_moved = false;
        let mut pointer_pressed = false;
        for e in events {
            self.diagnostics.events += 1;
            match e {
                egui::Event::PointerMoved(_) => self.diagnostics.pointer_moved += 1,
                egui::Event::MouseMoved(_) => {
                    self.diagnostics.mouse_moved += 1;
                    mouse_moved = true;
                }
                egui::Event::PointerButton { pressed, .. } => {
                    self.diagnostics.pointer_buttons += 1;
                    if *pressed {
                        pointer_pressed = true;
                    }
                }
                egui::Event::Touch { .. } => self.diagnostics.touch_events += 1,
                _ => {}
            }
            let egui::Event::Touch { device_id, id, phase, force, pos } = e else { continue };
            self.any_touch_this_frame = true;
            if pos.is_finite() {
                finite_touch_this_frame = true;
            }
            self.last_event = if force.is_some() { "touch with force" } else { "touch without force" };
            if force.is_some() {
                self.diagnostics.touch_with_force += 1;
            } else {
                self.diagnostics.touch_without_force += 1;
            }
            // A finite force at all means this contact is a pen; non-finite readings (e.g. NaN)
            // are ignored so they never promote a finger/junk reading to stylus mode.
            let reported = force.filter(|f| f.is_finite()).map(|f| f.clamp(0.0, 1.0));
            if reported.is_some() {
                self.pen = true;
            }
            match phase {
                egui::TouchPhase::Start => {
                    if self.pointer_touch.is_none() {
                        self.pointer_touch = Some(*id);
                    }
                    let is_stylus = reported.is_some();
                    if is_stylus {
                        self.landed = Some(*pos);
                    } else if self.finger_landed.is_none() {
                        self.finger_landed = Some((*id, *pos));
                    }
                    let pressure = if is_stylus {
                        let p = reported.unwrap_or(0.0);
                        self.highest = self.highest.max(p);
                        self.samples += 1;
                        p
                    } else {
                        1.0
                    };
                    self.contacts.insert(*id, ContactState { is_stylus, pressure, pos: *pos, device_id: *device_id });
                }
                egui::TouchPhase::Move => {
                    let entry =
                        self.contacts.entry(*id).or_insert(ContactState { is_stylus: false, pressure: 1.0, pos: *pos, device_id: *device_id });
                    entry.pos = *pos;
                    entry.device_id = *device_id;
                    if reported.is_some() {
                        entry.is_stylus = true;
                    }
                    if entry.is_stylus {
                        // Once this contact is known to be a stylus, a reading with no force is 0.0
                        // (winit reports light pen presses that way).
                        let p = reported.unwrap_or(0.0);
                        entry.pressure = p;
                        self.highest = self.highest.max(p);
                        self.samples += 1;
                    } else {
                        entry.pressure = 1.0;
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    let removed = self.contacts.remove(id);
                    let was_stylus = removed.is_some_and(|c| c.is_stylus) || reported.is_some();
                    if was_stylus {
                        self.stylus_ended_frame = true;
                    } else {
                        self.finger_ended_frame = true;
                    }
                    if self.pointer_touch == Some(*id) {
                        self.pointer_touch = None;
                    }
                }
            }
        }
        if self.has_active_stylus() || self.stylus_ended_frame {
            self.last_origin = PointerOrigin::Stylus;
        } else if finite_touch_this_frame || !self.contacts.is_empty() || self.finger_ended_frame {
            self.last_origin = PointerOrigin::Finger;
        } else if mouse_moved || (pointer_pressed && !self.any_touch_this_frame && self.contacts.is_empty()) {
            self.last_origin = PointerOrigin::Mouse;
        }
        if self.diagnostics.frames == 1 || self.diagnostics.frames.is_multiple_of(60) {
            let sample = self.sample();
            log::debug!(
                target: "nocturne::stylus",
                "input diagnostics: events={} pointer_moved={} mouse_moved={} pointer_buttons={} touch={} force={} no_force={} pen={} pressure={:.3} highest={:.3} readings={} contacts={} last={}",
                self.diagnostics.events,
                self.diagnostics.pointer_moved,
                self.diagnostics.mouse_moved,
                self.diagnostics.pointer_buttons,
                self.diagnostics.touch_events,
                self.diagnostics.touch_with_force,
                self.diagnostics.touch_without_force,
                self.pen,
                sample.pressure,
                self.highest,
                self.samples,
                self.contacts.len(),
                self.last_event,
            );
        }
    }

    /// The reading now. Active stylus contacts report their pressure; when only finger contacts are
    /// down (e.g. finger drawing), pressure is 1.0 even if a stylus was previously used. When no
    /// contact is down, reports 1.0 before any pen has been seen and 0.0 once a pen has been seen.
    pub fn sample(&self) -> Sample {
        if let Some(stylus) = self.contacts.values().rev().find(|c| c.is_stylus) {
            return Sample { pressure: stylus.pressure, pen: true, down: true };
        }
        if !self.contacts.is_empty() {
            return Sample { pressure: 1.0, pen: self.pen, down: true };
        }
        Sample { pressure: if self.pen { 0.0 } else { 1.0 }, pen: self.pen, down: false }
    }

    /// True while at least one stylus contact is currently touching the glass.
    pub fn has_active_stylus(&self) -> bool {
        self.contacts.values().any(|c| c.is_stylus)
    }

    /// Number of active non-stylus (finger/palm) contacts currently touching the glass.
    pub fn active_finger_count(&self) -> usize {
        self.contacts.values().filter(|c| !c.is_stylus).count()
    }

    /// Total active touch contacts currently touching the glass.
    pub fn active_contact_count(&self) -> usize {
        self.contacts.len()
    }

    /// Classifies whether this frame's pointer interaction comes from a mouse, a finger, or a stylus.
    pub fn pointer_origin(&self) -> PointerOrigin {
        if self.has_active_stylus() || self.stylus_ended_frame {
            PointerOrigin::Stylus
        } else if self.pointer_touch.is_some() || !self.contacts.is_empty() || self.finger_ended_frame || self.any_touch_this_frame {
            PointerOrigin::Finger
        } else {
            PointerOrigin::Mouse
        }
    }

    /// Sticky pointer origin across idle frames after touch or stylus interaction.
    /// Resets to `Mouse` only on explicit mouse movement or a non-touch pointer press.
    pub fn last_origin(&self) -> PointerOrigin {
        self.last_origin
    }

    /// True when the most recent pointer mode was touch or stylus rather than mouse.
    pub fn touch_affordance_active(&self) -> bool {
        self.last_origin != PointerOrigin::Mouse
    }

    /// Where a non-stylus (finger) touch started this frame, if one did.
    pub fn finger_landed(&self) -> Option<(egui::TouchId, egui::Pos2)> {
        self.finger_landed
    }

    /// True if a non-stylus (finger) touch ended or was cancelled this frame.
    pub fn finger_ended_this_frame(&self) -> bool {
        self.finger_ended_frame
    }

    /// Returns the primary active finger contact `(TouchId, Pos2)`, if any.
    pub fn primary_finger_touch(&self) -> Option<(egui::TouchId, egui::Pos2)> {
        if let Some(id) = self.pointer_touch
            && let Some(c) = self.contacts.get(&id)
            && !c.is_stylus
        {
            return Some((id, c.pos));
        }
        self.contacts.iter().find(|(_, c)| !c.is_stylus).map(|(id, c)| (*id, c.pos))
    }

    /// The hardest this pen has been pressed in this window, and how many readings have
    /// arrived. Both are for the diagnostic: a pen that only ever reports 0 or 1 is a pen
    /// the platform is not reading properly.
    pub fn seen(&self) -> (f32, u64) {
        (self.highest, self.samples)
    }

    /// Where a pen touched down this frame, in points: only a contact that reports a force as it
    /// starts, so a mouse or a finger on a touch screen never counts.
    pub fn landed(&self) -> Option<egui::Pos2> {
        self.landed
    }

    /// Event counters for the pressure diagnostic canvas and troubleshooting logs.
    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(phase: egui::TouchPhase, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(0), phase, pos: egui::Pos2::ZERO, force }
    }

    #[test]
    fn a_window_with_no_pen_writes_at_full_pressure() {
        let mut s = Stylus::default();
        assert_eq!(s.sample(), Sample { pressure: 1.0, pen: false, down: false });
        // A touch screen that reports no force is not a pen, and must not write at nothing.
        s.update(&[touch(egui::TouchPhase::Start, None), touch(egui::TouchPhase::Move, None)]);
        assert_eq!(s.sample(), Sample { pressure: 1.0, pen: false, down: true });
        assert_eq!(s.seen(), (0.0, 0));
    }

    #[test]
    fn a_pen_is_believed_once_it_reports_a_force() {
        let mut s = Stylus::default();
        s.update(&[touch(egui::TouchPhase::Start, Some(0.25)), touch(egui::TouchPhase::Move, Some(0.8))]);
        assert_eq!(s.sample(), Sample { pressure: 0.8, pen: true, down: true });
        assert_eq!(s.seen(), (0.8, 2), "the hardest press and how many readings arrived");

        // Once a pen is known, a reading with no force is a reading of nothing, not a mouse:
        // winit reports a press lighter than the pen's first level that way.
        s.update(&[touch(egui::TouchPhase::Move, None)]);
        assert_eq!(s.sample().pressure, 0.0);
        assert!(s.sample().pen, "it is still a pen");

        // Lifting ends the contact.
        s.update(&[touch(egui::TouchPhase::End, Some(0.1))]);
        assert_eq!(s.sample(), Sample { pressure: 0.0, pen: true, down: false });
    }

    /// Issue #9: a pen coming down is noticed for the one frame it happens in; a finger with no
    /// force, or a pen that is already down and moving, is not a pen landing.
    #[test]
    fn a_pen_landing_is_seen_for_its_frame_only() {
        let mut s = Stylus::default();
        s.update(&[touch(egui::TouchPhase::Start, None)]);
        assert_eq!(s.landed(), None, "a finger is not a pen");
        s.update(&[touch(egui::TouchPhase::End, None)]);
        s.update(&[touch(egui::TouchPhase::Start, Some(0.4))]);
        assert_eq!(s.landed(), Some(egui::Pos2::ZERO), "a pen came down");
        s.update(&[touch(egui::TouchPhase::Move, Some(0.6))]);
        assert_eq!(s.landed(), None, "only the frame it came down in");
        s.update(&[]);
        assert_eq!(s.landed(), None);
    }

    #[test]
    fn junk_readings_do_not_get_through() {
        let mut s = Stylus::default();
        s.update(&[touch(egui::TouchPhase::Start, Some(f32::NAN))]);
        assert!(!s.sample().pen, "a NaN is not a reading");
        s.update(&[touch(egui::TouchPhase::Move, Some(9.0)), touch(egui::TouchPhase::Move, Some(-3.0))]);
        let p = s.sample().pressure;
        assert!((0.0..=1.0).contains(&p), "pressure stays in range: {p}");
        assert_eq!(s.seen().0, 1.0, "the out of range high reading was clamped, not dropped");
    }

    #[test]
    fn concurrent_finger_and_stylus_contacts_stay_isolated() {
        let mut s = Stylus::default();
        let finger =
            |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(1), phase, pos: egui::pos2(20.0, 20.0), force: None };
        let stylus = |phase, force| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(2),
            phase,
            pos: egui::pos2(120.0, 140.0),
            force,
        };
        s.update(&[finger(egui::TouchPhase::Start), stylus(egui::TouchPhase::Start, Some(0.65))]);
        assert!(s.has_active_stylus());
        assert_eq!(s.active_finger_count(), 1);
        assert_eq!(s.pointer_origin(), PointerOrigin::Stylus);
        assert_eq!(s.landed(), Some(egui::pos2(120.0, 140.0)));
        assert_eq!(s.sample(), Sample { pressure: 0.65, pen: true, down: true });

        // Lifting the finger contact does NOT drop stylus down state or zero its pressure.
        s.update(&[finger(egui::TouchPhase::End), stylus(egui::TouchPhase::Move, Some(0.75))]);
        assert!(s.has_active_stylus());
        assert_eq!(s.active_finger_count(), 0);
        assert_eq!(s.sample(), Sample { pressure: 0.75, pen: true, down: true });

        // Lifting stylus ends stylus contact.
        s.update(&[stylus(egui::TouchPhase::End, Some(0.1))]);
        assert!(!s.has_active_stylus());
        assert_eq!(s.sample(), Sample { pressure: 0.0, pen: true, down: false });

        // Subsequent finger-only drawing reports full 1.0 pressure even though pen was seen earlier.
        s.update(&[finger(egui::TouchPhase::Start)]);
        assert_eq!(s.pointer_origin(), PointerOrigin::Finger);
        assert_eq!(s.sample(), Sample { pressure: 1.0, pen: true, down: true });
        s.update(&[finger(egui::TouchPhase::End)]);
        assert_eq!(s.pointer_origin(), PointerOrigin::Finger);
        s.update(&[]);
        assert_eq!(s.pointer_origin(), PointerOrigin::Mouse);
    }

    #[test]
    fn three_way_palm_finger_stylus_and_cancel_cleanup() {
        let mut s = Stylus::default();
        let ev = |dev, id, phase, force| egui::Event::Touch {
            device_id: egui::TouchDeviceId(dev),
            id: egui::TouchId(id),
            phase,
            pos: egui::pos2(id as f32 * 10.0, 40.0),
            force,
        };
        s.update(&[
            ev(0, 10, egui::TouchPhase::Start, None),
            ev(0, 11, egui::TouchPhase::Start, None),
            ev(1, 12, egui::TouchPhase::Start, Some(0.5)),
        ]);
        assert!(s.has_active_stylus());
        assert_eq!(s.active_finger_count(), 2);
        assert_eq!(s.active_contact_count(), 3);
        assert_eq!(s.sample().pressure, 0.5);

        s.update(&[ev(0, 11, egui::TouchPhase::Cancel, None), ev(0, 10, egui::TouchPhase::End, None), ev(1, 12, egui::TouchPhase::Move, Some(0.8))]);
        assert!(s.has_active_stylus());
        assert_eq!(s.active_finger_count(), 0);
        assert_eq!(s.sample(), Sample { pressure: 0.8, pen: true, down: true });

        s.update(&[ev(1, 12, egui::TouchPhase::End, Some(0.2))]);
        assert!(!s.has_active_stylus());
        assert_eq!(s.active_contact_count(), 0);
        assert_eq!(s.pointer_origin(), PointerOrigin::Stylus);
        assert_eq!(s.sample(), Sample { pressure: 0.0, pen: true, down: false });
    }

    #[test]
    fn sticky_last_origin_persists_across_idle_and_hover_then_resets_on_mouse() {
        let mut s = Stylus::default();
        assert_eq!(s.last_origin(), PointerOrigin::Mouse);
        assert!(!s.touch_affordance_active());

        // Finger touch + pointer press in same frame stays Finger across idle frames.
        s.update(&[
            touch(egui::TouchPhase::Start, None),
            egui::Event::PointerButton {
                pos: egui::Pos2::ZERO,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(s.last_origin(), PointerOrigin::Finger);
        assert!(s.touch_affordance_active());

        s.update(&[touch(egui::TouchPhase::End, None)]);
        assert_eq!(s.last_origin(), PointerOrigin::Finger);
        s.update(&[]);
        assert_eq!(s.pointer_origin(), PointerOrigin::Mouse);
        assert_eq!(s.last_origin(), PointerOrigin::Finger);
        assert!(s.touch_affordance_active());

        // PointerMoved (such as pen hover) does NOT clobber sticky touch/stylus origin.
        s.update(&[egui::Event::PointerMoved(egui::pos2(12.0, 14.0))]);
        assert_eq!(s.last_origin(), PointerOrigin::Finger);

        // Stylus touch promotes to Stylus and persists across lift + idle.
        s.update(&[touch(egui::TouchPhase::Start, Some(0.4))]);
        assert_eq!(s.last_origin(), PointerOrigin::Stylus);
        s.update(&[touch(egui::TouchPhase::End, Some(0.1))]);
        s.update(&[egui::Event::PointerMoved(egui::pos2(30.0, 40.0))]);
        assert_eq!(s.last_origin(), PointerOrigin::Stylus);
        assert!(s.touch_affordance_active());

        // MouseMoved resets back to Mouse.
        s.update(&[egui::Event::MouseMoved(egui::vec2(2.0, 0.0))]);
        assert_eq!(s.last_origin(), PointerOrigin::Mouse);
        assert!(!s.touch_affordance_active());

        // Standalone non-touch PointerButton press also resets back to Mouse.
        s.update(&[touch(egui::TouchPhase::Start, None)]);
        s.update(&[touch(egui::TouchPhase::End, None)]);
        assert_eq!(s.last_origin(), PointerOrigin::Finger);
        s.update(&[egui::Event::PointerButton {
            pos: egui::pos2(50.0, 50.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(s.last_origin(), PointerOrigin::Mouse);
    }
}
