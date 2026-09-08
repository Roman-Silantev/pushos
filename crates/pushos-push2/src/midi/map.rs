//! The hardware numbering table.
//!
//! Generated from Ableton's published `Push2-map.json`. This is the only place
//! in PushOS where a MIDI note or controller number appears next to a semantic
//! control id; everything above the adapter speaks in [`ControlId`] alone.

use pushos_domain::controls::{ButtonId, ControlId, EncoderId, PAD_COLUMNS, PAD_ROWS, PadIndex};

/// The note number of the bottom-left pad. The grid runs upward from there.
pub(crate) const PAD_NOTE_ORIGIN: u8 = 36;
/// The note number sent when the touch strip is touched.
pub(crate) const TOUCH_STRIP_NOTE: u8 = 12;

/// Every button, paired with the controller number it sends and receives.
pub(crate) const BUTTON_CC: [(ButtonId, u8); 65] = [
    (ButtonId::TapTempo, 3),
    (ButtonId::Metronome, 9),
    (ButtonId::Delete, 118),
    (ButtonId::Undo, 119),
    (ButtonId::Mute, 60),
    (ButtonId::Solo, 61),
    (ButtonId::Stop, 29),
    (ButtonId::Convert, 35),
    (ButtonId::DoubleLoop, 117),
    (ButtonId::Quantize, 116),
    (ButtonId::Duplicate, 88),
    (ButtonId::New, 87),
    (ButtonId::FixedLength, 90),
    (ButtonId::Automate, 89),
    (ButtonId::Record, 86),
    (ButtonId::Play, 85),
    (ButtonId::Upper1, 102),
    (ButtonId::Upper2, 103),
    (ButtonId::Upper3, 104),
    (ButtonId::Upper4, 105),
    (ButtonId::Upper5, 106),
    (ButtonId::Upper6, 107),
    (ButtonId::Upper7, 108),
    (ButtonId::Upper8, 109),
    (ButtonId::Lower1, 20),
    (ButtonId::Lower2, 21),
    (ButtonId::Lower3, 22),
    (ButtonId::Lower4, 23),
    (ButtonId::Lower5, 24),
    (ButtonId::Lower6, 25),
    (ButtonId::Lower7, 26),
    (ButtonId::Lower8, 27),
    (ButtonId::Div1_32T, 43),
    (ButtonId::Div1_32, 42),
    (ButtonId::Div1_16T, 41),
    (ButtonId::Div1_16, 40),
    (ButtonId::Div1_8T, 39),
    (ButtonId::Div1_8, 38),
    (ButtonId::Div1_4T, 37),
    (ButtonId::Div1_4, 36),
    (ButtonId::Setup, 30),
    (ButtonId::User, 59),
    (ButtonId::AddDevice, 52),
    (ButtonId::AddTrack, 53),
    (ButtonId::Device, 110),
    (ButtonId::Mix, 112),
    (ButtonId::Browse, 111),
    (ButtonId::Clip, 113),
    (ButtonId::Master, 28),
    (ButtonId::Up, 46),
    (ButtonId::Down, 47),
    (ButtonId::Left, 44),
    (ButtonId::Right, 45),
    (ButtonId::Repeat, 56),
    (ButtonId::Accent, 57),
    (ButtonId::Scale, 58),
    (ButtonId::Layout, 31),
    (ButtonId::Note, 50),
    (ButtonId::Session, 51),
    (ButtonId::OctaveUp, 55),
    (ButtonId::OctaveDown, 54),
    (ButtonId::PageLeft, 62),
    (ButtonId::PageRight, 63),
    (ButtonId::Shift, 49),
    (ButtonId::Select, 48),
];

/// Every encoder, paired with its turn controller number and its touch note.
pub(crate) const ENCODER_CC_TOUCH: [(EncoderId, u8, u8); 11] = [
    (EncoderId::Tempo, 14, 10),
    (EncoderId::Swing, 15, 9),
    (EncoderId::Track(0), 71, 0),
    (EncoderId::Track(1), 72, 1),
    (EncoderId::Track(2), 73, 2),
    (EncoderId::Track(3), 74, 3),
    (EncoderId::Track(4), 75, 4),
    (EncoderId::Track(5), 76, 5),
    (EncoderId::Track(6), 77, 6),
    (EncoderId::Track(7), 78, 7),
    (EncoderId::Master, 79, 8),
];

/// Translates a pad's grid position into the note the hardware uses.
///
/// PushOS numbers pads in reading order from the top-left, because that is how
/// a page layout is written. The hardware numbers them upward from the
/// bottom-left, so the row is mirrored here and nowhere else.
pub(crate) const fn pad_to_note(pad: PadIndex) -> u8 {
    let row_from_bottom = PAD_ROWS - 1 - pad.row();
    PAD_NOTE_ORIGIN + row_from_bottom * PAD_COLUMNS + pad.column()
}

/// Translates a note the hardware sent into a pad position.
pub(crate) fn note_to_pad(note: u8) -> Option<PadIndex> {
    let offset = note.checked_sub(PAD_NOTE_ORIGIN)?;
    let row_from_bottom = offset / PAD_COLUMNS;
    let column = offset % PAD_COLUMNS;
    if row_from_bottom >= PAD_ROWS {
        return None;
    }
    PadIndex::from_row_column(PAD_ROWS - 1 - row_from_bottom, column).ok()
}

/// Translates a controller number into the button that sends it.
pub(crate) fn cc_to_button(cc: u8) -> Option<ButtonId> {
    BUTTON_CC
        .iter()
        .find_map(|(button, number)| (*number == cc).then_some(*button))
}

/// Translates a button into the controller number it sends and receives.
pub(crate) fn button_to_cc(button: ButtonId) -> u8 {
    // Every variant appears in the generated table, so the fallback is
    // unreachable; returning zero keeps the function total rather than
    // introducing a panic on the input path.
    BUTTON_CC
        .iter()
        .find_map(|(candidate, cc)| (*candidate == button).then_some(*cc))
        .unwrap_or(0)
}

/// Translates a controller number into the encoder that turns it.
pub(crate) fn cc_to_encoder(cc: u8) -> Option<EncoderId> {
    ENCODER_CC_TOUCH
        .iter()
        .find_map(|(encoder, number, _)| (*number == cc).then_some(*encoder))
}

/// Translates a note into the encoder whose touch sensor sent it.
pub(crate) fn note_to_encoder_touch(note: u8) -> Option<EncoderId> {
    ENCODER_CC_TOUCH
        .iter()
        .find_map(|(encoder, _, touch)| (*touch == note).then_some(*encoder))
}

/// Translates a control into the note or controller number that addresses its
/// light, and whether that number is a note.
pub(crate) fn led_address(control: ControlId) -> Option<LedAddress> {
    match control {
        ControlId::Pad(pad) => Some(LedAddress::Note(pad_to_note(pad))),
        ControlId::Button(button) => Some(LedAddress::Controller(button_to_cc(button))),
        // Encoders have no individually addressable light, and the touch strip
        // is driven by a separate system-exclusive message.
        ControlId::Encoder(_) | ControlId::TouchStrip => None,
    }
}

/// How a light is addressed on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LedAddress {
    /// Addressed by note number, as pads are.
    Note(u8),
    /// Addressed by controller number, as buttons are.
    Controller(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pad_grid_maps_to_the_documented_note_range() {
        let top_left = PadIndex::new(0).expect("0 is in range");
        let bottom_left = PadIndex::from_row_column(7, 0).expect("bottom-left is in range");
        let top_right = PadIndex::from_row_column(0, 7).expect("top-right is in range");

        // `Pad S8 T1` in the hardware map is the bottom-left pad.
        assert_eq!(pad_to_note(bottom_left), 36);
        // `Pad S1 T8` is the top-right pad.
        assert_eq!(pad_to_note(top_right), 99);
        assert_eq!(pad_to_note(top_left), 92);
    }

    #[test]
    fn every_pad_survives_a_round_trip_through_its_note() {
        for pad in PadIndex::all() {
            assert_eq!(note_to_pad(pad_to_note(pad)), Some(pad));
        }
    }

    #[test]
    fn notes_outside_the_grid_are_rejected() {
        assert!(note_to_pad(35).is_none());
        assert!(note_to_pad(100).is_none());
        assert!(note_to_pad(0).is_none());
    }

    #[test]
    fn every_button_survives_a_round_trip_through_its_controller_number() {
        for (button, cc) in BUTTON_CC {
            assert_eq!(button_to_cc(button), cc);
            assert_eq!(cc_to_button(cc), Some(button));
        }
    }

    #[test]
    fn button_controller_numbers_are_unique() {
        let mut numbers: Vec<_> = BUTTON_CC.iter().map(|(_, cc)| *cc).collect();
        numbers.sort_unstable();
        let total = numbers.len();
        numbers.dedup();
        assert_eq!(total, numbers.len());
    }

    #[test]
    fn button_and_encoder_numbering_never_collide() {
        for (_, cc) in BUTTON_CC {
            assert!(
                cc_to_encoder(cc).is_none(),
                "controller {cc} is claimed by both a button and an encoder"
            );
        }
    }

    #[test]
    fn every_encoder_has_a_distinct_turn_and_touch_number() {
        for (encoder, cc, touch) in ENCODER_CC_TOUCH {
            assert_eq!(cc_to_encoder(cc), Some(encoder));
            assert_eq!(note_to_encoder_touch(touch), Some(encoder));
        }
        assert!(
            note_to_encoder_touch(TOUCH_STRIP_NOTE).is_none(),
            "the touch strip must not be mistaken for an encoder"
        );
    }

    #[test]
    fn encoder_touch_notes_never_collide_with_pad_notes() {
        for (_, _, touch) in ENCODER_CC_TOUCH {
            assert!(note_to_pad(touch).is_none());
        }
    }

    /// The domain decides which controls PushOS may light; this table decides
    /// how to address them. They must agree, or a light would be silently lost.
    #[test]
    fn the_hardware_map_agrees_with_the_domain_about_which_controls_light_up() {
        for control in ControlId::all() {
            assert_eq!(
                led_address(control).is_some(),
                control.is_illuminated(),
                "{control} disagrees about having an addressable light"
            );
        }
    }

    #[test]
    fn only_pads_and_buttons_have_addressable_lights() {
        assert!(matches!(
            led_address(ControlId::Pad(PadIndex::new(0).expect("in range"))),
            Some(LedAddress::Note(92))
        ));
        assert!(matches!(
            led_address(ControlId::Button(ButtonId::Play)),
            Some(LedAddress::Controller(85))
        ));
        assert!(led_address(ControlId::Encoder(EncoderId::Master)).is_none());
        assert!(led_address(ControlId::TouchStrip).is_none());
    }
}
