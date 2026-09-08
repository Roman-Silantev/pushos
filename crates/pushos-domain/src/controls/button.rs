//! The Push 2 button set.
//!
//! Variants and their stable textual ids are generated from Ableton's published
//! `Push2-map.json`. MIDI control-change numbers deliberately do **not** appear
//! here: this module is the semantic vocabulary, and the hardware numbering
//! lives in the `pushos-push2` adapter.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::ParseControlError;

/// A named, non-grid control on the Push 2 surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ButtonId {
    /// `button.tap_tempo`
    TapTempo,
    /// `button.metronome`
    Metronome,
    /// `button.delete`
    Delete,
    /// `button.undo`
    Undo,
    /// `button.mute`
    Mute,
    /// `button.solo`
    Solo,
    /// `button.stop`
    Stop,
    /// `button.convert`
    Convert,
    /// `button.double_loop`
    DoubleLoop,
    /// `button.quantize`
    Quantize,
    /// `button.duplicate`
    Duplicate,
    /// `button.new`
    New,
    /// `button.fixed_length`
    FixedLength,
    /// `button.automate`
    Automate,
    /// `button.record`
    Record,
    /// `button.play`
    Play,
    /// `button.upper_1`
    Upper1,
    /// `button.upper_2`
    Upper2,
    /// `button.upper_3`
    Upper3,
    /// `button.upper_4`
    Upper4,
    /// `button.upper_5`
    Upper5,
    /// `button.upper_6`
    Upper6,
    /// `button.upper_7`
    Upper7,
    /// `button.upper_8`
    Upper8,
    /// `button.lower_1`
    Lower1,
    /// `button.lower_2`
    Lower2,
    /// `button.lower_3`
    Lower3,
    /// `button.lower_4`
    Lower4,
    /// `button.lower_5`
    Lower5,
    /// `button.lower_6`
    Lower6,
    /// `button.lower_7`
    Lower7,
    /// `button.lower_8`
    Lower8,
    /// `button.div_1_32t`
    Div1_32T,
    /// `button.div_1_32`
    Div1_32,
    /// `button.div_1_16t`
    Div1_16T,
    /// `button.div_1_16`
    Div1_16,
    /// `button.div_1_8t`
    Div1_8T,
    /// `button.div_1_8`
    Div1_8,
    /// `button.div_1_4t`
    Div1_4T,
    /// `button.div_1_4`
    Div1_4,
    /// `button.setup`
    Setup,
    /// `button.user`
    User,
    /// `button.add_device`
    AddDevice,
    /// `button.add_track`
    AddTrack,
    /// `button.device`
    Device,
    /// `button.mix`
    Mix,
    /// `button.browse`
    Browse,
    /// `button.clip`
    Clip,
    /// `button.master`
    Master,
    /// `button.up`
    Up,
    /// `button.down`
    Down,
    /// `button.left`
    Left,
    /// `button.right`
    Right,
    /// `button.repeat`
    Repeat,
    /// `button.accent`
    Accent,
    /// `button.scale`
    Scale,
    /// `button.layout`
    Layout,
    /// `button.note`
    Note,
    /// `button.session`
    Session,
    /// `button.octave_up`
    OctaveUp,
    /// `button.octave_down`
    OctaveDown,
    /// `button.page_left`
    PageLeft,
    /// `button.page_right`
    PageRight,
    /// `button.shift`
    Shift,
    /// `button.select`
    Select,
}

/// Every button, in the order published by Ableton.
pub const ALL_BUTTONS: [ButtonId; 65] = [
    ButtonId::TapTempo,
    ButtonId::Metronome,
    ButtonId::Delete,
    ButtonId::Undo,
    ButtonId::Mute,
    ButtonId::Solo,
    ButtonId::Stop,
    ButtonId::Convert,
    ButtonId::DoubleLoop,
    ButtonId::Quantize,
    ButtonId::Duplicate,
    ButtonId::New,
    ButtonId::FixedLength,
    ButtonId::Automate,
    ButtonId::Record,
    ButtonId::Play,
    ButtonId::Upper1,
    ButtonId::Upper2,
    ButtonId::Upper3,
    ButtonId::Upper4,
    ButtonId::Upper5,
    ButtonId::Upper6,
    ButtonId::Upper7,
    ButtonId::Upper8,
    ButtonId::Lower1,
    ButtonId::Lower2,
    ButtonId::Lower3,
    ButtonId::Lower4,
    ButtonId::Lower5,
    ButtonId::Lower6,
    ButtonId::Lower7,
    ButtonId::Lower8,
    ButtonId::Div1_32T,
    ButtonId::Div1_32,
    ButtonId::Div1_16T,
    ButtonId::Div1_16,
    ButtonId::Div1_8T,
    ButtonId::Div1_8,
    ButtonId::Div1_4T,
    ButtonId::Div1_4,
    ButtonId::Setup,
    ButtonId::User,
    ButtonId::AddDevice,
    ButtonId::AddTrack,
    ButtonId::Device,
    ButtonId::Mix,
    ButtonId::Browse,
    ButtonId::Clip,
    ButtonId::Master,
    ButtonId::Up,
    ButtonId::Down,
    ButtonId::Left,
    ButtonId::Right,
    ButtonId::Repeat,
    ButtonId::Accent,
    ButtonId::Scale,
    ButtonId::Layout,
    ButtonId::Note,
    ButtonId::Session,
    ButtonId::OctaveUp,
    ButtonId::OctaveDown,
    ButtonId::PageLeft,
    ButtonId::PageRight,
    ButtonId::Shift,
    ButtonId::Select,
];

impl ButtonId {
    /// The stable textual name used in configuration, without the `button.` prefix.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::TapTempo => "tap_tempo",
            Self::Metronome => "metronome",
            Self::Delete => "delete",
            Self::Undo => "undo",
            Self::Mute => "mute",
            Self::Solo => "solo",
            Self::Stop => "stop",
            Self::Convert => "convert",
            Self::DoubleLoop => "double_loop",
            Self::Quantize => "quantize",
            Self::Duplicate => "duplicate",
            Self::New => "new",
            Self::FixedLength => "fixed_length",
            Self::Automate => "automate",
            Self::Record => "record",
            Self::Play => "play",
            Self::Upper1 => "upper_1",
            Self::Upper2 => "upper_2",
            Self::Upper3 => "upper_3",
            Self::Upper4 => "upper_4",
            Self::Upper5 => "upper_5",
            Self::Upper6 => "upper_6",
            Self::Upper7 => "upper_7",
            Self::Upper8 => "upper_8",
            Self::Lower1 => "lower_1",
            Self::Lower2 => "lower_2",
            Self::Lower3 => "lower_3",
            Self::Lower4 => "lower_4",
            Self::Lower5 => "lower_5",
            Self::Lower6 => "lower_6",
            Self::Lower7 => "lower_7",
            Self::Lower8 => "lower_8",
            Self::Div1_32T => "div_1_32t",
            Self::Div1_32 => "div_1_32",
            Self::Div1_16T => "div_1_16t",
            Self::Div1_16 => "div_1_16",
            Self::Div1_8T => "div_1_8t",
            Self::Div1_8 => "div_1_8",
            Self::Div1_4T => "div_1_4t",
            Self::Div1_4 => "div_1_4",
            Self::Setup => "setup",
            Self::User => "user",
            Self::AddDevice => "add_device",
            Self::AddTrack => "add_track",
            Self::Device => "device",
            Self::Mix => "mix",
            Self::Browse => "browse",
            Self::Clip => "clip",
            Self::Master => "master",
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Repeat => "repeat",
            Self::Accent => "accent",
            Self::Scale => "scale",
            Self::Layout => "layout",
            Self::Note => "note",
            Self::Session => "session",
            Self::OctaveUp => "octave_up",
            Self::OctaveDown => "octave_down",
            Self::PageLeft => "page_left",
            Self::PageRight => "page_right",
            Self::Shift => "shift",
            Self::Select => "select",
        }
    }

    /// Whether the button has a full-colour LED rather than a white-only LED.
    ///
    /// White-only buttons still accept a colour index, but it is resolved
    /// through the white palette, so only brightness is observable.
    pub const fn has_rgb_led(self) -> bool {
        matches!(
            self,
            Self::Mute
                | Self::Solo
                | Self::Stop
                | Self::Automate
                | Self::Record
                | Self::Play
                | Self::Upper1
                | Self::Upper2
                | Self::Upper3
                | Self::Upper4
                | Self::Upper5
                | Self::Upper6
                | Self::Upper7
                | Self::Upper8
                | Self::Lower1
                | Self::Lower2
                | Self::Lower3
                | Self::Lower4
                | Self::Lower5
                | Self::Lower6
                | Self::Lower7
                | Self::Lower8
                | Self::Div1_32T
                | Self::Div1_32
                | Self::Div1_16T
                | Self::Div1_16
                | Self::Div1_8T
                | Self::Div1_8
                | Self::Div1_4T
                | Self::Div1_4
        )
    }
}

impl fmt::Display for ButtonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "button.{}", self.slug())
    }
}

impl FromStr for ButtonId {
    type Err = ParseControlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL_BUTTONS
            .into_iter()
            .find(|button| button.slug() == s)
            .ok_or_else(|| ParseControlError::unknown_button(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_unique_and_parse_back() {
        let mut slugs: Vec<_> = ALL_BUTTONS.iter().map(|b| b.slug()).collect();
        slugs.sort_unstable();
        let unique = slugs.len();
        slugs.dedup();
        assert_eq!(unique, slugs.len(), "button slugs must be unique");

        for button in ALL_BUTTONS {
            assert_eq!(button.slug().parse::<ButtonId>(), Ok(button));
        }
    }

    #[test]
    fn display_uses_the_configuration_prefix() {
        assert_eq!(ButtonId::Play.to_string(), "button.play");
        assert_eq!(ButtonId::Upper1.to_string(), "button.upper_1");
        assert_eq!(ButtonId::Div1_32T.to_string(), "button.div_1_32t");
    }

    #[test]
    fn colour_capability_matches_the_hardware_map() {
        assert!(ButtonId::Play.has_rgb_led());
        assert!(!ButtonId::Undo.has_rgb_led());
    }
}
