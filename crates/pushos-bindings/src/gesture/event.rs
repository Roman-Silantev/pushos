//! What the recogniser produces.

use std::time::Instant;

use pushos_domain::action::READING;
use pushos_domain::controls::ControlId;
use pushos_domain::gesture::Gesture;

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
    /// How hard a pad was struck, `0..=127`.
    Velocity(u8),
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
            Self::Velocity(value) => Some(value),
            _ => None,
        }
    }

    /// Where along a strip the finger was, as a whole percentage.
    pub const fn percent(self) -> Option<u8> {
        match self {
            Self::Position(value) => Some(percent_of(value)),
            _ => None,
        }
    }

    /// The reading that has to travel with the gesture, and what to call it.
    ///
    /// Only an absolute position qualifies. A strike and a rotation are already
    /// described by the gesture a binding is written against, but a place along
    /// a strip appears nowhere in configuration and the action is meaningless
    /// without it.
    pub const fn reading(self) -> Option<(&'static str, u8)> {
        match self.percent() {
            Some(percent) => Some((READING, percent)),
            None => None,
        }
    }
}

/// The widest reading the strip sends, which stands for the far end of it.
const FULL_SCALE: u32 = 16_383;

/// Where a raw strip reading falls, as a whole percentage from the bottom.
///
/// A finger is about a percent of the strip wide, so a whole percent is as fine
/// as the reading can honestly be, and coarse enough that one sweep is a
/// hundred events rather than a thousand.
// Narrowed only on the branch where the value is already at most a hundred,
// which is the one thing a byte is certain to hold.
#[allow(clippy::cast_possible_truncation)]
const fn percent_of(value: u16) -> u8 {
    // Widened before multiplying, so the top of the strip cannot wrap.
    let scaled = (value as u32) * 100 / FULL_SCALE;
    // Saturating rather than wrapping: a reading past full scale would be a
    // hardware fault, and the far end of the strip is the honest answer to it.
    if scaled > 100 { 100 } else { scaled as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_accessors_only_answer_for_their_own_variant() {
        assert_eq!(GestureDetail::Delta(-3).delta(), Some(-3));
        assert_eq!(GestureDetail::Delta(-3).velocity(), None);
        assert_eq!(GestureDetail::Velocity(120).velocity(), Some(120));
        assert_eq!(GestureDetail::None.delta(), None);
    }

    #[test]
    fn a_strip_reading_spans_nought_to_a_hundred_percent() {
        assert_eq!(GestureDetail::Position(0).percent(), Some(0));
        assert_eq!(GestureDetail::Position(16_383).percent(), Some(100));
        assert_eq!(GestureDetail::Position(8_191).percent(), Some(49));
        assert_eq!(GestureDetail::Position(u16::MAX).percent(), Some(100));
        assert_eq!(GestureDetail::Velocity(100).percent(), None);
    }

    #[test]
    fn only_a_position_has_to_travel_with_its_gesture() {
        assert_eq!(GestureDetail::Position(0).reading(), Some((READING, 0)));
        assert_eq!(GestureDetail::Delta(3).reading(), None);
        assert_eq!(GestureDetail::Velocity(3).reading(), None);
        assert_eq!(GestureDetail::None.reading(), None);
    }
}
