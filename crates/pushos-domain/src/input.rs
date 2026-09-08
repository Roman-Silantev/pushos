//! Normalised hardware input.
//!
//! [`ControlEvent`] is the only shape in which physical activity enters PushOS.
//! It carries semantic control ids and monotonic timestamps, never MIDI bytes.

use std::time::Instant;

use crate::controls::ControlId;

/// One normalised interaction with a physical control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlEvent {
    /// Which control produced the event.
    pub control: ControlId,
    /// What happened to it.
    pub phase: InputPhase,
    /// When the adapter observed it, on the host's monotonic clock.
    pub at: Instant,
}

impl ControlEvent {
    /// Builds an event.
    pub const fn new(control: ControlId, phase: InputPhase, at: Instant) -> Self {
        Self { control, phase, at }
    }
}

/// The kind of interaction a control reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputPhase {
    /// A pad or button went down. Buttons report full velocity.
    Down {
        /// Strike velocity, `0..=127`.
        velocity: u8,
    },
    /// A pad or button was released.
    Up,
    /// Continuous pressure while a pad is held.
    Pressure {
        /// Aftertouch amount, `0..=127`.
        amount: u8,
    },
    /// An encoder was rotated by a signed detent count.
    Turn {
        /// Positive is clockwise. Never zero.
        delta: i8,
    },
    /// A touch-sensitive control was touched.
    Touch,
    /// A touch-sensitive control was released.
    TouchRelease,
    /// The touch strip reported an absolute position, `0..=16383`.
    Position {
        /// Distance along the strip, from the bottom.
        value: u16,
    },
}

impl InputPhase {
    /// Whether the phase begins an interaction that a later phase must close.
    pub const fn is_engaging(self) -> bool {
        matches!(self, Self::Down { .. } | Self::Touch)
    }

    /// Whether the phase closes an interaction opened by [`Self::is_engaging`].
    pub const fn is_releasing(self) -> bool {
        matches!(self, Self::Up | Self::TouchRelease)
    }

    /// Whether the phase is a high-frequency stream that may be coalesced.
    ///
    /// Coalescing these must never drop an engage or release transition.
    pub const fn is_continuous(self) -> bool {
        matches!(self, Self::Pressure { .. } | Self::Position { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engage_and_release_phases_are_disjoint() {
        let phases = [
            InputPhase::Down { velocity: 100 },
            InputPhase::Up,
            InputPhase::Pressure { amount: 40 },
            InputPhase::Turn { delta: -1 },
            InputPhase::Touch,
            InputPhase::TouchRelease,
            InputPhase::Position { value: 8192 },
        ];
        for phase in phases {
            assert!(
                !(phase.is_engaging() && phase.is_releasing()),
                "{phase:?} cannot both open and close an interaction"
            );
        }
    }

    #[test]
    fn only_pressure_and_position_may_be_coalesced() {
        assert!(InputPhase::Pressure { amount: 1 }.is_continuous());
        assert!(InputPhase::Position { value: 1 }.is_continuous());
        assert!(!InputPhase::Down { velocity: 1 }.is_continuous());
        assert!(!InputPhase::Up.is_continuous());
        assert!(!InputPhase::Turn { delta: 1 }.is_continuous());
    }
}
