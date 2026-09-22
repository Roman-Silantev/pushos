//! What the recogniser produces.

use std::time::Instant;

use pushos_domain::controls::ControlId;
use pushos_domain::gesture::{ForceBand, Gesture};

/// A recognised gesture on a control, with whatever analogue detail came with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureEvent {
    /// The control that was operated.
    pub control: ControlId,
    /// What the operator did.
    pub gesture: Gesture,
    /// Analogue detail, when the control produced any.
    pub detail: GestureDetail,
    /// When the gesture was recognised.
    pub at: Instant,
}

impl GestureEvent {
    /// Builds a gesture event carrying no analogue detail.
    pub const fn simple(control: ControlId, gesture: Gesture, at: Instant) -> Self {
        Self {
            control,
            gesture,
            detail: GestureDetail::None,
            at,
        }
    }

    /// Builds a gesture event with analogue detail attached.
    pub const fn detailed(
        control: ControlId,
        gesture: Gesture,
        detail: GestureDetail,
        at: Instant,
    ) -> Self {
        Self {
            control,
            gesture,
            detail,
            at,
        }
    }
}

/// Analogue information a gesture carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureDetail {
    /// The control is purely digital.
    None,
    /// How hard a pad was struck.
    ///
    /// Both the raw reading and the band it falls in, because the recogniser
    /// is the only place allowed to decide where the bands are and nothing
    /// downstream should have to know the edges to act on a strike.
    Struck {
        /// Strike velocity, `0..=127`.
        velocity: u8,
        /// Which band that velocity falls in.
        force: ForceBand,
    },
    /// How far an encoder moved. Positive is clockwise.
    Delta(i8),
    /// Absolute position along the touch strip.
    Position(u16),
}

impl GestureDetail {
    /// The encoder movement, when the gesture was a turn.
    pub const fn delta(self) -> Option<i8> {
        match self {
            Self::Delta(value) => Some(value),
            _ => None,
        }
    }

    /// The strike velocity, when the gesture came from a pad.
    pub const fn velocity(self) -> Option<u8> {
        match self {
            Self::Struck { velocity, .. } => Some(velocity),
            _ => None,
        }
    }

    /// The band the strike fell in, when the gesture began with one.
    pub const fn force(self) -> Option<ForceBand> {
        match self {
            Self::Struck { force, .. } => Some(force),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_accessors_only_answer_for_their_own_variant() {
        let struck = GestureDetail::Struck {
            velocity: 120,
            force: ForceBand::Hard,
        };

        assert_eq!(GestureDetail::Delta(-3).delta(), Some(-3));
        assert_eq!(GestureDetail::Delta(-3).velocity(), None);
        assert_eq!(GestureDetail::Delta(-3).force(), None);
        assert_eq!(struck.velocity(), Some(120));
        assert_eq!(struck.force(), Some(ForceBand::Hard));
        assert_eq!(struck.delta(), None);
        assert_eq!(GestureDetail::None.delta(), None);
        assert_eq!(GestureDetail::None.force(), None);
    }
}
