//! The timing thresholds that define what a gesture means.
//!
//! These live in one place so that no feature can invent its own hold or
//! double-tap window and make the surface feel inconsistent.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a press must last, and how close two taps must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureTiming {
    /// A press held at least this long becomes a hold rather than a tap.
    pub hold_threshold: Duration,
    /// A second press starting within this window of the first release
    /// becomes a double tap.
    pub double_tap_window: Duration,
}

impl GestureTiming {
    /// The defaults, chosen so a deliberate hold is unambiguous while a quick
    /// double tap stays comfortable.
    pub const DEFAULT: Self = Self {
        hold_threshold: Duration::from_millis(400),
        double_tap_window: Duration::from_millis(280),
    };

    /// Rejects timings that would make gestures indistinguishable.
    ///
    /// A hold threshold at or below the double-tap window would let a single
    /// slow double tap register as a hold, which is exactly the kind of
    /// ambiguity the resolver refuses to guess about.
    pub const fn validate(self) -> Result<(), InvalidTiming> {
        if self.hold_threshold.is_zero() {
            return Err(InvalidTiming::ZeroHoldThreshold);
        }
        if self.double_tap_window.is_zero() {
            return Err(InvalidTiming::ZeroDoubleTapWindow);
        }
        if self.hold_threshold.as_millis() <= self.double_tap_window.as_millis() {
            return Err(InvalidTiming::HoldNotLongerThanDoubleTap);
        }
        Ok(())
    }
}

impl Default for GestureTiming {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A configured gesture timing that would produce ambiguous gestures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTiming {
    /// Every press would immediately be a hold.
    #[error("hold threshold must be greater than zero")]
    ZeroHoldThreshold,
    /// No two taps could ever pair.
    #[error("double tap window must be greater than zero")]
    ZeroDoubleTapWindow,
    /// A slow double tap would be indistinguishable from a hold.
    #[error("hold threshold must be longer than the double tap window")]
    HoldNotLongerThanDoubleTap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_self_consistent() {
        assert!(GestureTiming::DEFAULT.validate().is_ok());
    }

    #[test]
    fn ambiguous_timings_are_rejected() {
        let overlapping = GestureTiming {
            hold_threshold: Duration::from_millis(200),
            double_tap_window: Duration::from_millis(300),
        };
        assert_eq!(
            overlapping.validate(),
            Err(InvalidTiming::HoldNotLongerThanDoubleTap)
        );

        let zero_hold = GestureTiming {
            hold_threshold: Duration::ZERO,
            ..GestureTiming::DEFAULT
        };
        assert_eq!(zero_hold.validate(), Err(InvalidTiming::ZeroHoldThreshold));

        let zero_window = GestureTiming {
            double_tap_window: Duration::ZERO,
            ..GestureTiming::DEFAULT
        };
        assert_eq!(
            zero_window.validate(),
            Err(InvalidTiming::ZeroDoubleTapWindow)
        );
    }
}
