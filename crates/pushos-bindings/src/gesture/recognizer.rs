//! The single owner of gesture timing.
//!
//! Every hold, double tap and Shift decision in PushOS is made here. Features
//! never run their own timers, so the surface behaves the same everywhere.

use std::collections::HashMap;
use std::time::Instant;

use pushos_domain::controls::{ButtonId, ControlId};
use pushos_domain::gesture::{ForceThresholds, Gesture};
use pushos_domain::input::{ControlEvent, InputPhase};

use super::event::{GestureDetail, GestureEvent};
use super::interest::GestureInterest;
use super::state::{ControlState, HeldState};
use super::timing::GestureTiming;

/// How much aftertouch makes one step.
///
/// Sixteen gives eight steps across the pad's range, which is about as many
/// as a finger can aim at without looking, and few enough that rolling a
/// thumb through the whole range dispatches no more work than spinning an
/// encoder a turn.
const PRESSURE_STEP: u8 = 16;

/// How far past a step's edge a reading must go before the step changes.
///
/// Aftertouch is noisy and a finger is not still. Without this, a pad resting
/// on an edge would report a stream of alternating steps and scroll a session
/// back and forth under the operator.
const PRESSURE_MARGIN: u8 = 4;

/// Turns normalised input into gestures.
///
/// The recogniser is driven by two calls: [`Self::observe`] for hardware
/// activity and [`Self::poll`] for elapsed time. It never reads a clock itself,
/// and it never allocates per event: recognised gestures are appended to a
/// buffer the caller owns.
#[derive(Debug)]
pub struct GestureRecognizer {
    timing: GestureTiming,
    forces: ForceThresholds,
    interest: GestureInterest,
    states: HashMap<ControlId, ControlState>,
    shift_held: bool,
}

impl GestureRecognizer {
    /// Builds a recogniser for the given timings, force edges and bindings.
    pub fn new(timing: GestureTiming, forces: ForceThresholds, interest: GestureInterest) -> Self {
        Self {
            timing,
            forces,
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

    /// Replaces the force band edges, after a configuration swap.
    ///
    /// A pad already down keeps the velocity it was struck with, so the band
    /// a press is reported in is the one that was in force when the operator
    /// hit it, not the one that arrived while their finger was still on it.
    pub fn set_forces(&mut self, forces: ForceThresholds) {
        self.forces = forces;
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
            InputPhase::Pressure { amount } => {
                self.on_pressure(event.control, event.at, amount, out);
            }
            // The touch strip is not used by PushOS at all: the adapter puts
            // its lights out at connect, and its position is a spring-loaded
            // pitch bend rather than anything an operator could hold a place
            // with.
            InputPhase::Position { .. } => {}
        }
    }

    /// Advances time, appending any gestures that have now become due.
    pub fn poll(&mut self, now: Instant, out: &mut Vec<GestureEvent>) {
        let mut settled = Vec::new();
        let forces = self.forces;

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
                ControlState::AwaitingSecondTap {
                    expires_at,
                    velocity,
                    ..
                } if now >= *expires_at => {
                    out.push(GestureEvent::detailed(
                        control,
                        Gesture::Tap,
                        GestureDetail::Struck {
                            velocity: *velocity,
                            force: forces.band_of(*velocity),
                        },
                        now,
                    ));
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
            self.struck(velocity),
            at,
        ));

        let hold_at = self
            .interest
            .wants_hold(control)
            .then(|| at + self.timing.hold_threshold);
        self.states.insert(
            control,
            ControlState::Held(HeldState::new(at, shift, hold_at, pending_tap, velocity)),
        );
    }

    /// The analogue detail a strike of this velocity carries.
    ///
    /// The one place the edges between bands are consulted, so no feature can
    /// disagree about what counts as a hard strike.
    fn struck(&self, velocity: u8) -> GestureDetail {
        GestureDetail::Struck {
            velocity,
            force: self.forces.band_of(velocity),
        }
    }

    /// Turns the aftertouch stream from one held pad into steps.
    ///
    /// Ignored entirely for a pad that is not held, and for one nothing binds:
    /// a hand resting on the surface must not wake the rest of PushOS.
    fn on_pressure(
        &mut self,
        control: ControlId,
        at: Instant,
        amount: u8,
        out: &mut Vec<GestureEvent>,
    ) {
        if !self.interest.wants_pressure(control) {
            return;
        }
        let Some(ControlState::Held(held)) = self.states.get_mut(&control) else {
            // Pressure after a release, or from a press lost to a disconnect.
            return;
        };

        let settled = settled_level(amount, held.level);
        if settled == held.level {
            return;
        }

        let steps = i16::from(settled) - i16::from(held.level);
        held.level = settled;

        let gesture = if steps.is_positive() {
            Gesture::PressHarder
        } else {
            Gesture::PressSofter
        };
        // The step count fits: there are eight levels, so the largest jump is
        // seven in either direction.
        let delta = i8::try_from(steps).unwrap_or(if steps.is_positive() {
            i8::MAX
        } else {
            i8::MIN
        });
        out.push(GestureEvent::detailed(
            control,
            gesture,
            GestureDetail::Delta(delta),
            at,
        ));
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

        let detail = self.struck(held.velocity);

        if held.is_second_tap {
            out.push(GestureEvent::detailed(
                control,
                Gesture::DoubleTap,
                detail,
                at,
            ));
            return;
        }

        if self.interest.wants_double_tap(control) {
            self.states.insert(
                control,
                ControlState::AwaitingSecondTap {
                    expires_at: at + self.timing.double_tap_window,
                    shift: held.shift,
                    velocity: held.velocity,
                },
            );
        } else {
            out.push(GestureEvent::detailed(control, Gesture::Tap, detail, at));
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

/// Which aftertouch step a reading belongs to, given the step it is leaving.
///
/// A reading only changes step once it is clear of the current step's edges by
/// the margin, which is what stops a resting finger chattering between two.
const fn settled_level(amount: u8, current: u8) -> u8 {
    let step = current as u16 * PRESSURE_STEP as u16;
    let above = step + PRESSURE_STEP as u16 + PRESSURE_MARGIN as u16;
    let below = step.saturating_sub(PRESSURE_MARGIN as u16);
    let reading = amount as u16;

    if reading >= above || reading < below {
        amount / PRESSURE_STEP
    } else {
        current
    }
}
