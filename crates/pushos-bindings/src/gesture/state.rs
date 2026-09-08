//! Per-control gesture state.
//!
//! One small state machine per engaged control. Nothing here reads a clock:
//! time always arrives as an argument, which is what makes the recogniser
//! testable without sleeping.

use std::time::Instant;

/// Where one control is in its press cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ControlState {
    /// The control is held.
    Held(HeldState),
    /// The control was tapped and a second tap could still arrive.
    AwaitingSecondTap {
        /// When the double-tap window closes.
        expires_at: Instant,
        /// Whether Shift was held when the first press began.
        shift: bool,
    },
}

/// What is known about a control that is currently down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldState {
    /// When the press began.
    pub(crate) pressed_at: Instant,
    /// Whether Shift was held at the moment of the press.
    ///
    /// Captured once, so releasing Shift mid-press cannot change what the
    /// gesture turns out to mean.
    pub(crate) shift: bool,
    /// When the press becomes a hold, if a hold binding exists for the control.
    pub(crate) hold_at: Option<Instant>,
    /// Whether the hold has already been reported.
    pub(crate) hold_reported: bool,
    /// Whether this press is the second half of a potential double tap.
    pub(crate) is_second_tap: bool,
}

impl HeldState {
    /// Begins a press.
    pub(crate) const fn new(
        pressed_at: Instant,
        shift: bool,
        hold_at: Option<Instant>,
        is_second_tap: bool,
    ) -> Self {
        Self {
            pressed_at,
            shift,
            hold_at,
            hold_reported: false,
            is_second_tap,
        }
    }

    /// Whether the hold threshold has passed and has not yet been reported.
    pub(crate) fn hold_is_due(&self, now: Instant) -> bool {
        !self.hold_reported && self.hold_at.is_some_and(|due| now >= due)
    }
}

impl ControlState {
    /// The next moment at which this state produces a gesture on its own.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Held(held) if !held.hold_reported => held.hold_at,
            Self::Held(_) => None,
            Self::AwaitingSecondTap { expires_at, .. } => Some(*expires_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_hold_becomes_due_only_once_its_deadline_passes() {
        let start = Instant::now();
        let due = start + Duration::from_millis(400);
        let held = HeldState::new(start, false, Some(due), false);

        assert!(!held.hold_is_due(start));
        assert!(!held.hold_is_due(start + Duration::from_millis(399)));
        assert!(held.hold_is_due(due));
    }

    #[test]
    fn a_press_without_a_hold_binding_never_becomes_due() {
        let start = Instant::now();
        let held = HeldState::new(start, false, None, false);
        assert!(!held.hold_is_due(start + Duration::from_secs(60)));
        assert!(ControlState::Held(held).deadline().is_none());
    }

    #[test]
    fn a_reported_hold_stops_asking_to_be_woken() {
        let start = Instant::now();
        let mut held = HeldState::new(start, false, Some(start), false);
        assert!(ControlState::Held(held).deadline().is_some());
        held.hold_reported = true;
        assert!(!held.hold_is_due(start));
        assert!(ControlState::Held(held).deadline().is_none());
    }
}
