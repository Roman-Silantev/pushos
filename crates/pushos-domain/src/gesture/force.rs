//! How hard a pad was struck.
//!
//! A Push 2 pad reports a strike velocity, which a binding may narrow itself
//! to. Three bands rather than a number: an operator can learn to hit a pad
//! three ways and feel which one they chose, and cannot aim at a figure.
//!
//! Only pads report a usable force. Push 2 buttons send a fixed value, so a
//! banded binding on one would never match; the validator refuses that rather
//! than letting it fail silently.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How hard a strike was, as a binding can be written against it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ForceBand {
    /// Brushed.
    Soft,
    /// Pressed deliberately. What an ordinary tap produces.
    Firm,
    /// Struck.
    Hard,
}

impl ForceBand {
    /// Every band, for exhaustive validation and documentation.
    pub const ALL: [Self; 3] = [Self::Soft, Self::Firm, Self::Hard];

    /// The stable textual form used in configuration.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Soft => "soft",
            Self::Firm => "firm",
            Self::Hard => "hard",
        }
    }
}

impl fmt::Display for ForceBand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl fmt::Debug for ForceBand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl FromStr for ForceBand {
    type Err = UnknownForceBand;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|band| band.slug() == s)
            .ok_or_else(|| UnknownForceBand(s.to_owned()))
    }
}

impl TryFrom<String> for ForceBand {
    type Error = UnknownForceBand;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ForceBand> for String {
    fn from(value: ForceBand) -> Self {
        value.slug().to_owned()
    }
}

/// A configured force band that PushOS does not recognise.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown force `{0}`, expected one of `soft`, `firm` or `hard`")]
pub struct UnknownForceBand(pub String);

/// Where one band ends and the next begins.
///
/// Adjustable because pads differ and so do hands: an operator who finds every
/// strike landing in one band moves the edges rather than changing how they
/// play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForceThresholds {
    firm_at: u8,
    hard_at: u8,
}

impl ForceThresholds {
    /// The edges used when configuration says nothing.
    ///
    /// Chosen so an ordinary, unthinking tap lands in `firm`: that is what
    /// every existing binding already means, and the bands an operator adds
    /// are the two deliberate ones either side of it.
    pub const DEFAULT: Self = Self {
        firm_at: 46,
        hard_at: 101,
    };

    /// Builds thresholds, rejecting edges that leave a band unreachable.
    pub const fn new(firm_at: u8, hard_at: u8) -> Result<Self, ImpossibleForce> {
        if firm_at == 0 {
            return Err(ImpossibleForce::SoftUnreachable);
        }
        if hard_at <= firm_at {
            return Err(ImpossibleForce::OutOfOrder { firm_at, hard_at });
        }
        if hard_at > 127 {
            return Err(ImpossibleForce::HardUnreachable { hard_at });
        }
        Ok(Self { firm_at, hard_at })
    }

    /// Where `firm` begins.
    pub const fn firm_at(self) -> u8 {
        self.firm_at
    }

    /// Where `hard` begins.
    pub const fn hard_at(self) -> u8 {
        self.hard_at
    }

    /// The band a strike velocity falls in.
    pub const fn band_of(self, velocity: u8) -> ForceBand {
        if velocity >= self.hard_at {
            ForceBand::Hard
        } else if velocity >= self.firm_at {
            ForceBand::Firm
        } else {
            ForceBand::Soft
        }
    }
}

impl Default for ForceThresholds {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Force thresholds that would leave a band impossible to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImpossibleForce {
    /// `firm` starts at zero, so nothing could ever be soft.
    #[error("`firm_at` must be above zero, or no strike could ever be soft")]
    SoftUnreachable,
    /// The edges are the wrong way round, or equal.
    #[error("`hard_at` ({hard_at}) must be above `firm_at` ({firm_at})")]
    OutOfOrder {
        /// Where firm was said to begin.
        firm_at: u8,
        /// Where hard was said to begin.
        hard_at: u8,
    },
    /// `hard` starts above the highest velocity a pad can send.
    #[error("`hard_at` ({hard_at}) is above 127, so no strike could ever be hard")]
    HardUnreachable {
        /// Where hard was said to begin.
        hard_at: u8,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_round_trips_and_is_listed_once() {
        let mut slugs: Vec<_> = ForceBand::ALL.iter().map(|band| band.slug()).collect();
        slugs.sort_unstable();
        let total = slugs.len();
        slugs.dedup();
        assert_eq!(total, slugs.len(), "force slugs must be unique");

        for band in ForceBand::ALL {
            assert_eq!(band.slug().parse::<ForceBand>(), Ok(band));
        }
        assert!("crushed".parse::<ForceBand>().is_err());
    }

    #[test]
    fn bands_are_ordered_by_how_hard_they_are() {
        assert!(ForceBand::Soft < ForceBand::Firm);
        assert!(ForceBand::Firm < ForceBand::Hard);
    }

    #[test]
    fn the_default_edges_put_an_ordinary_tap_in_firm() {
        let thresholds = ForceThresholds::DEFAULT;
        assert_eq!(thresholds.band_of(0), ForceBand::Soft);
        assert_eq!(thresholds.band_of(45), ForceBand::Soft);
        assert_eq!(thresholds.band_of(46), ForceBand::Firm);
        assert_eq!(thresholds.band_of(100), ForceBand::Firm);
        assert_eq!(thresholds.band_of(101), ForceBand::Hard);
        assert_eq!(thresholds.band_of(127), ForceBand::Hard);
    }

    #[test]
    fn every_velocity_lands_in_exactly_one_band() {
        let thresholds = ForceThresholds::DEFAULT;
        for velocity in 0..=127u8 {
            let band = thresholds.band_of(velocity);
            let expected = match velocity {
                0..=45 => ForceBand::Soft,
                46..=100 => ForceBand::Firm,
                _ => ForceBand::Hard,
            };
            assert_eq!(band, expected, "velocity {velocity} landed in {band}");
        }
    }

    #[test]
    fn edges_that_would_strand_a_band_are_refused() {
        assert_eq!(
            ForceThresholds::new(0, 90),
            Err(ImpossibleForce::SoftUnreachable)
        );
        assert_eq!(
            ForceThresholds::new(90, 90),
            Err(ImpossibleForce::OutOfOrder {
                firm_at: 90,
                hard_at: 90
            })
        );
        assert_eq!(
            ForceThresholds::new(90, 50),
            Err(ImpossibleForce::OutOfOrder {
                firm_at: 90,
                hard_at: 50
            })
        );
        assert_eq!(
            ForceThresholds::new(40, 200),
            Err(ImpossibleForce::HardUnreachable { hard_at: 200 })
        );
        assert!(ForceThresholds::new(1, 127).is_ok());
    }

    #[test]
    fn custom_edges_are_honoured() {
        let thresholds = ForceThresholds::new(20, 60).expect("edges are in order");
        assert_eq!(thresholds.band_of(19), ForceBand::Soft);
        assert_eq!(thresholds.band_of(20), ForceBand::Firm);
        assert_eq!(thresholds.band_of(59), ForceBand::Firm);
        assert_eq!(thresholds.band_of(60), ForceBand::Hard);
    }
}
