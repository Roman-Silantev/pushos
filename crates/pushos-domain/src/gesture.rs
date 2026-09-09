//! The gesture vocabulary that bindings are written against.
//!
//! Recognition lives in `pushos-bindings`. This module only defines the shapes,
//! so no feature can invent its own hold or double-tap semantics.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A recognised interaction, richer than a raw input phase.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Gesture {
    /// The control went down. Fires immediately, before tap or hold is known.
    Press,
    /// The control came back up.
    Release,
    /// A short press that was not followed by a second press.
    Tap,
    /// Two taps within the double-tap window.
    DoubleTap,
    /// The control stayed down past the hold threshold.
    Hold,
    /// A press while Shift was held.
    ShiftPress,
    /// A hold while Shift was held.
    ShiftHold,
    /// An encoder moved in either direction.
    Turn,
    /// An encoder moved anticlockwise.
    TurnLeft,
    /// An encoder moved clockwise.
    TurnRight,
    /// An encoder moved while Shift was held, for fine adjustment.
    ShiftTurn,
    /// A touch-sensitive control was touched.
    Touch,
    /// A touch-sensitive control was released.
    TouchRelease,
    /// A finger moved to a place along a touch strip.
    ///
    /// Absolute rather than relative: the strip reports where the finger is,
    /// not how far it travelled, so a binding on this is a position and not a
    /// nudge. The reading travels with the gesture.
    Slide,
}

impl Gesture {
    /// Every gesture, for exhaustive validation and documentation.
    pub const ALL: [Self; 14] = [
        Self::Press,
        Self::Release,
        Self::Tap,
        Self::DoubleTap,
        Self::Hold,
        Self::ShiftPress,
        Self::ShiftHold,
        Self::Turn,
        Self::TurnLeft,
        Self::TurnRight,
        Self::ShiftTurn,
        Self::Touch,
        Self::TouchRelease,
        Self::Slide,
    ];

    /// The stable textual form used in configuration.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Release => "release",
            Self::Tap => "tap",
            Self::DoubleTap => "double_tap",
            Self::Hold => "hold",
            Self::ShiftPress => "shift_press",
            Self::ShiftHold => "shift_hold",
            Self::Turn => "turn",
            Self::TurnLeft => "turn_left",
            Self::TurnRight => "turn_right",
            Self::ShiftTurn => "shift_turn",
            Self::Touch => "touch",
            Self::TouchRelease => "touch_release",
            Self::Slide => "slide",
        }
    }

    /// Whether a binding written for `self` should fire for `produced`.
    ///
    /// The only widening rule is directional: a binding on `turn` accepts both
    /// rotation directions. Everything else must match exactly, so that
    /// precedence stays predictable.
    pub const fn accepts(self, produced: Self) -> bool {
        if matches!(
            (self, produced),
            (Self::Turn, Self::TurnLeft | Self::TurnRight)
        ) {
            return true;
        }
        self as u8 == produced as u8
    }

    /// Whether the gesture carries an analogue reading of its own.
    ///
    /// A binding on one of these is incomplete without the value the hardware
    /// sent, so the reading is attached to the action before it is dispatched.
    pub const fn is_analogue(self) -> bool {
        matches!(self, Self::Slide)
    }

    /// Whether the gesture requires Shift to have been held.
    pub const fn requires_shift(self) -> bool {
        matches!(self, Self::ShiftPress | Self::ShiftHold | Self::ShiftTurn)
    }
}

impl fmt::Display for Gesture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl fmt::Debug for Gesture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl FromStr for Gesture {
    type Err = UnknownGesture;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|gesture| gesture.slug() == s)
            .ok_or_else(|| UnknownGesture(s.to_owned()))
    }
}

impl TryFrom<String> for Gesture {
    type Error = UnknownGesture;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Gesture> for String {
    fn from(value: Gesture) -> Self {
        value.slug().to_owned()
    }
}

/// A configured gesture name that PushOS does not recognise.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown gesture `{0}`")]
pub struct UnknownGesture(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gesture_round_trips_and_is_listed_once() {
        let mut slugs: Vec<_> = Gesture::ALL.iter().map(|g| g.slug()).collect();
        slugs.sort_unstable();
        let total = slugs.len();
        slugs.dedup();
        assert_eq!(total, slugs.len(), "gesture slugs must be unique");

        for gesture in Gesture::ALL {
            assert_eq!(gesture.slug().parse::<Gesture>(), Ok(gesture));
        }
    }

    #[test]
    fn turn_is_the_only_widening_binding() {
        assert!(Gesture::Turn.accepts(Gesture::TurnLeft));
        assert!(Gesture::Turn.accepts(Gesture::TurnRight));
        assert!(Gesture::Turn.accepts(Gesture::Turn));

        assert!(!Gesture::TurnLeft.accepts(Gesture::TurnRight));
        assert!(!Gesture::Tap.accepts(Gesture::DoubleTap));
        assert!(!Gesture::Press.accepts(Gesture::Tap));
        assert!(!Gesture::Hold.accepts(Gesture::ShiftHold));
    }

    #[test]
    fn shift_gestures_are_flagged() {
        for gesture in Gesture::ALL {
            assert_eq!(
                gesture.requires_shift(),
                gesture.slug().starts_with("shift_"),
                "{gesture} shift flag disagrees with its name"
            );
        }
    }
}
