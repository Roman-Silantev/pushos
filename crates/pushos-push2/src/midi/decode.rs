//! Wire bytes to normalised input.
//!
//! Decoding is pure and allocation-free: it takes a MIDI message and a
//! timestamp and returns at most one [`ControlEvent`]. Anything unrecognised is
//! dropped rather than guessed at.

use std::time::Instant;

use pushos_domain::controls::ControlId;
use pushos_domain::input::{ControlEvent, InputPhase};

use super::map;

/// Status nibble for note off.
const NOTE_OFF: u8 = 0x80;
/// Status nibble for note on.
const NOTE_ON: u8 = 0x90;
/// Status nibble for polyphonic key pressure.
const POLY_PRESSURE: u8 = 0xA0;
/// Status nibble for control change.
const CONTROL_CHANGE: u8 = 0xB0;
/// Status nibble for pitch bend, which the touch strip uses.
const PITCH_BEND: u8 = 0xE0;

/// Decodes one MIDI message into normalised input.
///
/// Returns `None` for messages PushOS does not act on, including clock,
/// system-exclusive replies and any control the surface does not expose.
pub(crate) fn decode(message: &[u8], at: Instant) -> Option<ControlEvent> {
    let (&status, data) = message.split_first()?;
    // Push 2 uses the channel nibble for LED animation, never for routing input.
    match status & 0xF0 {
        NOTE_ON | NOTE_OFF => decode_note(status & 0xF0, data, at),
        CONTROL_CHANGE => decode_control_change(data, at),
        POLY_PRESSURE => decode_poly_pressure(data, at),
        PITCH_BEND => decode_pitch_bend(data, at),
        // Channel pressure is global rather than per control, and everything
        // else on the wire is clock, sense or system exclusive.
        _ => None,
    }
}

fn decode_note(status: u8, data: &[u8], at: Instant) -> Option<ControlEvent> {
    let (&note, rest) = data.split_first()?;
    let velocity = rest.first().copied().unwrap_or(0);
    // A note-on with zero velocity is the conventional note-off.
    let released = status == NOTE_OFF || velocity == 0;

    if let Some(pad) = map::note_to_pad(note) {
        let phase = if released {
            InputPhase::Up
        } else {
            InputPhase::Down { velocity }
        };
        return Some(ControlEvent::new(ControlId::Pad(pad), phase, at));
    }

    if let Some(encoder) = map::note_to_encoder_touch(note) {
        let phase = if released {
            InputPhase::TouchRelease
        } else {
            InputPhase::Touch
        };
        return Some(ControlEvent::new(ControlId::Encoder(encoder), phase, at));
    }

    if note == map::TOUCH_STRIP_NOTE {
        let phase = if released {
            InputPhase::TouchRelease
        } else {
            InputPhase::Touch
        };
        return Some(ControlEvent::new(ControlId::TouchStrip, phase, at));
    }

    None
}

fn decode_control_change(data: &[u8], at: Instant) -> Option<ControlEvent> {
    let (&controller, rest) = data.split_first()?;
    let value = rest.first().copied().unwrap_or(0);

    if let Some(button) = map::cc_to_button(controller) {
        let phase = if value == 0 {
            InputPhase::Up
        } else {
            InputPhase::Down { velocity: value }
        };
        return Some(ControlEvent::new(ControlId::Button(button), phase, at));
    }

    if let Some(encoder) = map::cc_to_encoder(controller) {
        let delta = decode_relative(value)?;
        return Some(ControlEvent::new(
            ControlId::Encoder(encoder),
            InputPhase::Turn { delta },
            at,
        ));
    }

    None
}

fn decode_poly_pressure(data: &[u8], at: Instant) -> Option<ControlEvent> {
    let (&note, rest) = data.split_first()?;
    let amount = rest.first().copied().unwrap_or(0);
    let pad = map::note_to_pad(note)?;
    Some(ControlEvent::new(
        ControlId::Pad(pad),
        InputPhase::Pressure { amount },
        at,
    ))
}

fn decode_pitch_bend(data: &[u8], at: Instant) -> Option<ControlEvent> {
    let low = u16::from(*data.first()?);
    let high = u16::from(*data.get(1)?);
    let value = (high << 7) | low;
    Some(ControlEvent::new(
        ControlId::TouchStrip,
        InputPhase::Position { value },
        at,
    ))
}

/// Decodes the seven-bit two's-complement value an encoder sends.
///
/// Values `1..=63` are clockwise; `64..=127` are anticlockwise, counting down
/// from `127` as minus one. Zero means the encoder did not move.
fn decode_relative(value: u8) -> Option<i8> {
    match value {
        0 => None,
        1..=63 => i8::try_from(value).ok(),
        _ => i8::try_from(i16::from(value) - 128).ok(),
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::controls::{ButtonId, EncoderId, PadIndex};

    use super::*;

    fn at() -> Instant {
        Instant::now()
    }

    #[test]
    fn the_bottom_left_pad_decodes_from_its_documented_note() {
        let event = decode(&[0x90, 36, 120], at()).expect("note 36 is the bottom-left pad");
        assert_eq!(
            event.control,
            ControlId::Pad(PadIndex::from_row_column(7, 0).expect("in range"))
        );
        assert_eq!(event.phase, InputPhase::Down { velocity: 120 });
    }

    #[test]
    fn a_zero_velocity_note_on_is_a_release() {
        let event = decode(&[0x90, 36, 0], at()).expect("still a pad message");
        assert_eq!(event.phase, InputPhase::Up);

        let explicit = decode(&[0x80, 36, 64], at()).expect("explicit note off");
        assert_eq!(explicit.phase, InputPhase::Up);
    }

    #[test]
    fn a_button_decodes_from_its_controller_number() {
        let event = decode(&[0xB0, 85, 127], at()).expect("controller 85 is Play");
        assert_eq!(event.control, ControlId::Button(ButtonId::Play));
        assert_eq!(event.phase, InputPhase::Down { velocity: 127 });

        let release = decode(&[0xB0, 85, 0], at()).expect("still the Play button");
        assert_eq!(release.phase, InputPhase::Up);
    }

    #[test]
    fn encoder_turns_decode_as_signed_detents() {
        let right = decode(&[0xB0, 71, 1], at()).expect("controller 71 is the first encoder");
        assert_eq!(right.control, ControlId::Encoder(EncoderId::Track(0)));
        assert_eq!(right.phase, InputPhase::Turn { delta: 1 });

        let left = decode(&[0xB0, 71, 127], at()).expect("127 is minus one");
        assert_eq!(left.phase, InputPhase::Turn { delta: -1 });

        let fast_left = decode(&[0xB0, 71, 124], at()).expect("124 is minus four");
        assert_eq!(fast_left.phase, InputPhase::Turn { delta: -4 });
    }

    #[test]
    fn an_encoder_reporting_no_movement_produces_nothing() {
        assert!(decode(&[0xB0, 71, 0], at()).is_none());
    }

    #[test]
    fn encoder_touch_decodes_from_its_note() {
        let touch = decode(&[0x90, 0, 127], at()).expect("note 0 is the first encoder's touch");
        assert_eq!(touch.control, ControlId::Encoder(EncoderId::Track(0)));
        assert_eq!(touch.phase, InputPhase::Touch);

        let release = decode(&[0x90, 0, 0], at()).expect("still the touch sensor");
        assert_eq!(release.phase, InputPhase::TouchRelease);
    }

    #[test]
    fn pad_aftertouch_decodes_as_pressure() {
        let event = decode(&[0xA0, 36, 90], at()).expect("polyphonic pressure on a pad");
        assert_eq!(event.phase, InputPhase::Pressure { amount: 90 });
    }

    #[test]
    fn the_touch_strip_reports_an_absolute_position() {
        let event = decode(&[0xE0, 0x00, 0x40], at()).expect("pitch bend is the touch strip");
        assert_eq!(event.control, ControlId::TouchStrip);
        assert_eq!(event.phase, InputPhase::Position { value: 8_192 });
    }

    #[test]
    fn the_animation_channel_does_not_change_which_control_is_addressed() {
        for channel in 0..16u8 {
            let event = decode(&[0x90 | channel, 36, 100], at()).expect("channel is animation");
            assert_eq!(
                event.control,
                ControlId::Pad(PadIndex::from_row_column(7, 0).expect("in range"))
            );
        }
    }

    #[test]
    fn unrecognised_traffic_is_dropped_rather_than_guessed_at() {
        assert!(decode(&[], at()).is_none(), "empty message");
        assert!(decode(&[0xF8], at()).is_none(), "clock");
        assert!(
            decode(&[0xF0, 0x00, 0x21], at()).is_none(),
            "system exclusive"
        );
        assert!(
            decode(&[0xD0, 90], at()).is_none(),
            "channel aftertouch is not per control"
        );
        assert!(
            decode(&[0xB0, 5, 64], at()).is_none(),
            "controller 5 is not on the surface"
        );
        assert!(decode(&[0x90], at()).is_none(), "truncated note message");
    }
}
