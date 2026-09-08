//! The single owner of gesture timing.
//!
//! Every hold, double tap and Shift decision in PushOS is made here. Features
//! never run their own timers, so the surface behaves the same everywhere.

use std::collections::HashMap;
use std::time::Instant;

use pushos_domain::controls::{ButtonId, ControlId};
use pushos_domain::gesture::Gesture;
use pushos_domain::input::{ControlEvent, InputPhase};

use super::event::{GestureDetail, GestureEvent};
use super::interest::GestureInterest;
use super::state::{ControlState, HeldState};
use super::timing::GestureTiming;

/// Turns normalised input into gestures.
///
/// The recogniser is driven by two calls: [`Self::observe`] for hardware
/// activity and [`Self::poll`] for elapsed time. It never reads a clock itself,
/// and it never allocates per event: recognised gestures are appended to a
/// buffer the caller owns.
#[derive(Debug)]
pub struct GestureRecognizer {
    timing: GestureTiming,
    interest: GestureInterest,
    states: HashMap<ControlId, ControlState>,
    shift_held: bool,
}

impl GestureRecognizer {
    /// Builds a recogniser for the given timings and bindings.
    pub fn new(timing: GestureTiming, interest: GestureInterest) -> Self {
        Self {
            timing,
            interest,
            states: HashMap::new(),
            shift_held: false,
        }
    }

    /// Whether Shift is currently down.
    pub const fn shift_held(&self) -> bool {
        self.shift_held
    }

    /// Replaces the set of controls needing timers, after a configuration swap.
    ///
    /// Controls already mid-gesture keep the timers they were armed with, so a
    /// reload cannot strand a press that is already in flight.
    pub fn set_interest(&mut self, interest: GestureInterest) {
        self.interest = interest;
    }

    /// The next moment at which [`Self::poll`] would produce a gesture.
    ///
    /// `None` means nothing is pending and the caller may sleep until the next
    /// hardware event.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.states
            .values()
            .filter_map(ControlState::deadline)
            .min()
    }

    /// Forgets all in-flight gestures.
    ///
    /// Called when the surface disappears, so that a press interrupted by a
    /// disconnect cannot complete as a hold against a device that is gone.
    pub fn reset(&mut self) {
        self.states.clear();
        self.shift_held = false;
    }

    /// Feeds one hardware event in, appending any gestures it completes.
    pub fn observe(&mut self, event: ControlEvent, out: &mut Vec<GestureEvent>) {
        self.track_shift(&event);
        match event.phase {
            InputPhase::Down { velocity } => self.on_down(event.control, event.at, velocity, out),
            InputPhase::Up => self.on_up(event.control, event.at, out),
            InputPhase::Turn { delta } => Self::on_turn(self.shift_held, &event, delta, out),
            InputPhase::Touch => {
                out.push(GestureEvent::simple(
                    event.control,
                    Gesture::Touch,
                    event.at,
                ));
            }
            InputPhase::TouchRelease => {
                out.push(GestureEvent::simple(
                    event.control,
                    Gesture::TouchRelease,
                    event.at,
                ));
            }
            // Continuous streams drive widgets directly; they are not gestures.
            InputPhase::Pressure { .. } | InputPhase::Position { .. } => {}
        }
    }

    /// Advances time, appending any gestures that have now become due.
    pub fn poll(&mut self, now: Instant, out: &mut Vec<GestureEvent>) {
        let mut settled = Vec::new();

        for (&control, state) in &mut self.states {
            match state {
                ControlState::Held(held) if held.hold_is_due(now) => {
                    held.hold_reported = true;
                    let gesture = if held.shift {
                        Gesture::ShiftHold
                    } else {
                        Gesture::Hold
                    };
                    out.push(GestureEvent::simple(control, gesture, now));
                }
                ControlState::AwaitingSecondTap { expires_at, .. } if now >= *expires_at => {
                    out.push(GestureEvent::simple(control, Gesture::Tap, now));
                    settled.push(control);
                }
                ControlState::Held(_) | ControlState::AwaitingSecondTap { .. } => {}
            }
        }

        for control in settled {
            self.states.remove(&control);
        }
    }

    fn track_shift(&mut self, event: &ControlEvent) {
        if event.control == ControlId::Button(ButtonId::Shift) {
            match event.phase {
                InputPhase::Down { .. } => self.shift_held = true,
                InputPhase::Up => self.shift_held = false,
                _ => {}
            }
        }
    }

    fn on_down(
        &mut self,
        control: ControlId,
        at: Instant,
        velocity: u8,
        out: &mut Vec<GestureEvent>,
    ) {
        let shift = self.shift_held;
        let pending_tap = matches!(
            self.states.get(&control),
            Some(ControlState::AwaitingSecondTap { expires_at, .. }) if at < *expires_at
        );

        let press = if shift {
            Gesture::ShiftPress
        } else {
            Gesture::Press
        };
        out.push(GestureEvent::detailed(
            control,
            press,
            GestureDetail::Velocity(velocity),
            at,
        ));

        let hold_at = self
            .interest
            .wants_hold(control)
            .then(|| at + self.timing.hold_threshold);
        self.states.insert(
            control,
            ControlState::Held(HeldState::new(at, shift, hold_at, pending_tap)),
        );
    }

    fn on_up(&mut self, control: ControlId, at: Instant, out: &mut Vec<GestureEvent>) {
        out.push(GestureEvent::simple(control, Gesture::Release, at));

        let Some(ControlState::Held(held)) = self.states.remove(&control) else {
            // A release with no matching press: the press was lost to a
            // disconnect or a reset. Report the release and nothing more.
            return;
        };

        if held.hold_reported {
            return;
        }

        if held.is_second_tap {
            out.push(GestureEvent::simple(control, Gesture::DoubleTap, at));
            return;
        }

        if self.interest.wants_double_tap(control) {
            self.states.insert(
                control,
                ControlState::AwaitingSecondTap {
                    expires_at: at + self.timing.double_tap_window,
                    shift: held.shift,
                },
            );
        } else {
            out.push(GestureEvent::simple(control, Gesture::Tap, at));
        }
    }

    fn on_turn(shift_held: bool, event: &ControlEvent, delta: i8, out: &mut Vec<GestureEvent>) {
        if delta == 0 {
            return;
        }
        let gesture = if shift_held {
            Gesture::ShiftTurn
        } else if delta.is_negative() {
            Gesture::TurnLeft
        } else {
            Gesture::TurnRight
        };
        out.push(GestureEvent::detailed(
            event.control,
            gesture,
            GestureDetail::Delta(delta),
            event.at,
        ));
    }
}
