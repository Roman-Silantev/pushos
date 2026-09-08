//! Stable, semantic identities for every Push 2 control.
//!
//! Nothing in this module knows about MIDI. Adapters translate wire numbering
//! into these ids at the edge, so pages, bindings and actions can be written
//! against names that survive a hardware revision.

mod button;
mod encoder;
mod pad;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use button::{ALL_BUTTONS, ButtonId};
pub use encoder::{EncoderId, TRACK_ENCODER_COUNT};
pub use pad::{PAD_COLUMNS, PAD_COUNT, PAD_ROWS, PadIndex};

/// Any addressable control on the surface.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ControlId {
    /// One of the 64 grid pads.
    Pad(PadIndex),
    /// A named button.
    Button(ButtonId),
    /// A rotary encoder.
    Encoder(EncoderId),
    /// The capacitive touch strip to the left of the pads.
    TouchStrip,
}

impl ControlId {
    /// The broad category a control belongs to.
    pub const fn kind(self) -> ControlKind {
        match self {
            Self::Pad(_) => ControlKind::Pad,
            Self::Button(_) => ControlKind::Button,
            Self::Encoder(_) => ControlKind::Encoder,
            Self::TouchStrip => ControlKind::TouchStrip,
        }
    }

    /// Whether the control carries a light that PushOS can drive.
    pub const fn is_illuminated(self) -> bool {
        !matches!(self, Self::TouchStrip)
    }

    /// Iterates every control the surface exposes.
    pub fn all() -> impl Iterator<Item = Self> {
        PadIndex::all()
            .map(Self::Pad)
            .chain(ALL_BUTTONS.into_iter().map(Self::Button))
            .chain(EncoderId::all().map(Self::Encoder))
            .chain(std::iter::once(Self::TouchStrip))
    }
}

/// The category of a control, used when a page needs to lay out by shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    /// A grid pad.
    Pad,
    /// A named button.
    Button,
    /// A rotary encoder.
    Encoder,
    /// The touch strip.
    TouchStrip,
}

impl fmt::Display for ControlId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pad(pad) => fmt::Display::fmt(pad, f),
            Self::Button(button) => fmt::Display::fmt(button, f),
            Self::Encoder(encoder) => fmt::Display::fmt(encoder, f),
            Self::TouchStrip => f.write_str("touchstrip"),
        }
    }
}

impl fmt::Debug for ControlId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for ControlId {
    type Err = ParseControlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "touchstrip" {
            return Ok(Self::TouchStrip);
        }
        let (namespace, rest) = s
            .split_once('.')
            .ok_or_else(|| ParseControlError::unknown_control(s))?;
        match namespace {
            "pad" => rest.parse().map(Self::Pad),
            "button" => rest.parse().map(Self::Button),
            "encoder" => rest.parse().map(Self::Encoder),
            _ => Err(ParseControlError::unknown_control(s)),
        }
    }
}

impl TryFrom<String> for ControlId {
    type Error = ParseControlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ControlId> for String {
    fn from(value: ControlId) -> Self {
        value.to_string()
    }
}

/// Why a textual control name could not be understood.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseControlError {
    /// The name did not match any known control.
    #[error("unknown control `{name}`")]
    UnknownControl {
        /// The rejected text.
        name: String,
    },
    /// The name used the `button.` namespace but no such button exists.
    #[error("unknown button `button.{name}`")]
    UnknownButton {
        /// The rejected suffix.
        name: String,
    },
    /// The name used the `encoder.` namespace but no such encoder exists.
    #[error("unknown encoder `encoder.{name}`")]
    UnknownEncoder {
        /// The rejected suffix.
        name: String,
    },
    /// A pad index fell outside the 8x8 grid.
    #[error("pad index {index} is outside the 8x8 grid")]
    PadOutOfRange {
        /// The rejected index.
        index: u8,
    },
    /// An encoder position fell outside the eight-encoder strip.
    #[error("encoder position {position} is outside the eight-encoder strip")]
    EncoderOutOfRange {
        /// The rejected position.
        position: u8,
    },
}

impl ParseControlError {
    pub(crate) fn unknown_control(name: &str) -> Self {
        Self::UnknownControl {
            name: name.to_owned(),
        }
    }

    pub(crate) fn unknown_button(name: &str) -> Self {
        Self::UnknownButton {
            name: name.to_owned(),
        }
    }

    pub(crate) fn unknown_encoder(name: &str) -> Self {
        Self::UnknownEncoder {
            name: name.to_owned(),
        }
    }

    pub(crate) fn unknown_pad(name: &str) -> Self {
        Self::UnknownControl {
            name: format!("pad.{name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_survives_a_text_round_trip() {
        for control in ControlId::all() {
            let rendered = control.to_string();
            assert_eq!(
                rendered.parse::<ControlId>(),
                Ok(control),
                "control `{rendered}` did not round-trip"
            );
        }
    }

    #[test]
    fn the_surface_exposes_the_expected_control_count() {
        // 64 pads + 65 buttons + 11 encoders + 1 touch strip.
        assert_eq!(ControlId::all().count(), 141);
    }

    #[test]
    fn malformed_names_are_rejected_rather_than_guessed() {
        assert!("pad".parse::<ControlId>().is_err());
        assert!("pad.64".parse::<ControlId>().is_err());
        assert!("knob.3".parse::<ControlId>().is_err());
        assert!("button.nope".parse::<ControlId>().is_err());
    }

    #[test]
    fn only_the_touch_strip_lacks_a_controllable_light() {
        assert!(!ControlId::TouchStrip.is_illuminated());
        assert!(ControlId::Button(ButtonId::Play).is_illuminated());
    }
}
