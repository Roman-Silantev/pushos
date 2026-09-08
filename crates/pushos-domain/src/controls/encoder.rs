//! The eleven rotary encoders.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::ParseControlError;

/// Number of encoders positioned above the display.
pub const TRACK_ENCODER_COUNT: u8 = 8;

/// A rotary encoder.
///
/// The eight encoders above the display are addressed positionally as
/// `encoder.0` through `encoder.7`; the three fixed-function encoders keep
/// their hardware names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderId {
    /// The leftmost encoder, labelled "Tempo".
    Tempo,
    /// The second encoder, labelled "Swing".
    Swing,
    /// One of the eight encoders above the display, zero-based from the left.
    Track(u8),
    /// The rightmost encoder, labelled "Master".
    Master,
}

impl EncoderId {
    /// Builds a track encoder, rejecting positions outside the strip.
    pub const fn track(position: u8) -> Result<Self, ParseControlError> {
        if position < TRACK_ENCODER_COUNT {
            Ok(Self::Track(position))
        } else {
            Err(ParseControlError::EncoderOutOfRange { position })
        }
    }

    /// Iterates every encoder in left-to-right hardware order.
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Tempo, Self::Swing]
            .into_iter()
            .chain((0..TRACK_ENCODER_COUNT).map(Self::Track))
            .chain(std::iter::once(Self::Master))
    }
}

impl fmt::Display for EncoderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tempo => f.write_str("encoder.tempo"),
            Self::Swing => f.write_str("encoder.swing"),
            Self::Track(position) => write!(f, "encoder.{position}"),
            Self::Master => f.write_str("encoder.master"),
        }
    }
}

impl fmt::Debug for EncoderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for EncoderId {
    type Err = ParseControlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tempo" => Ok(Self::Tempo),
            "swing" => Ok(Self::Swing),
            "master" => Ok(Self::Master),
            other => other
                .parse::<u8>()
                .map_err(|_| ParseControlError::unknown_encoder(other))
                .and_then(Self::track),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encoder_renders_and_parses() {
        for encoder in EncoderId::all() {
            let rendered = encoder.to_string();
            let suffix = rendered
                .strip_prefix("encoder.")
                .expect("encoders render with their prefix");
            assert_eq!(suffix.parse::<EncoderId>(), Ok(encoder));
        }
    }

    #[test]
    fn track_positions_beyond_the_strip_are_rejected() {
        assert!(EncoderId::track(7).is_ok());
        assert!(EncoderId::track(8).is_err());
        assert!("8".parse::<EncoderId>().is_err());
    }
}
