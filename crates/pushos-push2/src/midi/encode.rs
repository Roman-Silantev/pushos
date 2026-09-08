//! Normalised light state to wire bytes.
//!
//! A light is set by sending a palette index to the control's note or
//! controller number. The MIDI channel selects the animation, so an animated
//! light needs two messages: the resting colour on channel zero, then the
//! target colour on the animation channel.

use pushos_domain::color::{AnimationRate, LedAnimation, LedState, Rgb};
use pushos_domain::controls::ControlId;
use pushos_domain::ports::PushSurfaceError;

use super::map::{self, LedAddress};
use super::palette;

/// A three-byte MIDI message.
pub(crate) type MidiMessage = [u8; 3];

/// How many messages one light change can produce.
pub(crate) const MAX_MESSAGES_PER_LED: usize = 2;

/// The channel that means "no animation".
const CHANNEL_STATIC: u8 = 0;
/// The first pulsing channel, for the fastest rate.
const CHANNEL_PULSE_BASE: u8 = 6;
/// The first blinking channel, for the fastest rate.
const CHANNEL_BLINK_BASE: u8 = 11;

/// How dim the resting half of a pulse is, relative to the lit half.
const PULSE_FLOOR: f32 = 0.15;

/// Appends the messages that put one control's light into the given state.
///
/// Nothing is allocated: the caller owns the buffer, which the LED update path
/// reuses across frames.
pub(crate) fn led_messages(
    control: ControlId,
    state: LedState,
    out: &mut Vec<MidiMessage>,
) -> Result<(), PushSurfaceError> {
    let address = map::led_address(control).ok_or(PushSurfaceError::NotIlluminated { control })?;

    match state.animation {
        LedAnimation::Solid => {
            out.push(message(
                address,
                CHANNEL_STATIC,
                palette::index_of(state.color),
            ));
        }
        LedAnimation::Pulse(rate) => {
            // The animation moves between the channel-zero colour and the
            // target, so the resting colour is a dimmed version of the same hue.
            out.push(message(
                address,
                CHANNEL_STATIC,
                palette::index_of(state.color.dimmed(PULSE_FLOOR)),
            ));
            out.push(message(
                address,
                CHANNEL_PULSE_BASE + rate_offset(rate),
                palette::index_of(state.color),
            ));
        }
        LedAnimation::Blink(rate) => {
            out.push(message(
                address,
                CHANNEL_STATIC,
                palette::index_of(Rgb::BLACK),
            ));
            out.push(message(
                address,
                CHANNEL_BLINK_BASE + rate_offset(rate),
                palette::index_of(state.color),
            ));
        }
    }
    Ok(())
}

/// The message that turns a control's light off.
pub(crate) fn led_off(control: ControlId) -> Option<MidiMessage> {
    map::led_address(control).map(|address| message(address, CHANNEL_STATIC, 0))
}

const fn message(address: LedAddress, channel: u8, value: u8) -> MidiMessage {
    match address {
        LedAddress::Note(note) => [0x90 | channel, note, value],
        LedAddress::Controller(controller) => [0xB0 | channel, controller, value],
    }
}

/// How far a rate sits from the fastest one, matching the hardware's ordering.
const fn rate_offset(rate: AnimationRate) -> u8 {
    match rate {
        AnimationRate::TwentyFourth => 0,
        AnimationRate::Sixteenth => 1,
        AnimationRate::Eighth => 2,
        AnimationRate::Quarter => 3,
        AnimationRate::Half => 4,
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::controls::{ButtonId, EncoderId, PadIndex};

    use super::*;

    fn messages(control: ControlId, state: LedState) -> Vec<MidiMessage> {
        let mut out = Vec::new();
        led_messages(control, state, &mut out).expect("control has a light");
        out
    }

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    #[test]
    fn a_solid_pad_light_is_one_message_on_the_static_channel() {
        let out = messages(pad(0), LedState::solid(Rgb::WHITE));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0][0], 0x90, "static animation is channel zero");
        assert_eq!(out[0][1], 92, "pad zero is the top-left pad");
        assert_eq!(out[0][2], palette::index_of(Rgb::WHITE));
    }

    #[test]
    fn a_button_light_is_addressed_by_controller_number() {
        let out = messages(
            ControlId::Button(ButtonId::Play),
            LedState::solid(Rgb::WHITE),
        );
        assert_eq!(out[0][0], 0xB0);
        assert_eq!(out[0][1], 85);
    }

    #[test]
    fn an_animated_light_sets_its_resting_colour_first() {
        let out = messages(
            pad(0),
            LedState::new(
                Rgb::new(0, 0, 255),
                LedAnimation::Blink(AnimationRate::Eighth),
            ),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0][0] & 0x0F,
            CHANNEL_STATIC,
            "the resting colour comes first"
        );
        assert_eq!(out[0][2], 0, "a blink rests at black");
        assert_eq!(out[1][0] & 0x0F, 13, "eighth-note blinking is channel 13");
        assert_eq!(out[1][2], palette::index_of(Rgb::new(0, 0, 255)));
    }

    #[test]
    fn a_pulse_rests_on_a_dimmed_version_of_its_own_colour() {
        let color = Rgb::new(0, 255, 0);
        let out = messages(
            pad(0),
            LedState::new(color, LedAnimation::Pulse(AnimationRate::Quarter)),
        );
        assert_eq!(out[1][0] & 0x0F, 9, "quarter-note pulsing is channel 9");
        assert_ne!(
            out[0][2], out[1][2],
            "the two halves of a pulse must differ"
        );
        assert_ne!(out[0][2], 0, "a pulse rests dim, not dark");
    }

    #[test]
    fn every_animation_rate_maps_into_its_documented_channel_block() {
        for rate in [
            AnimationRate::TwentyFourth,
            AnimationRate::Sixteenth,
            AnimationRate::Eighth,
            AnimationRate::Quarter,
            AnimationRate::Half,
        ] {
            let pulse = messages(pad(0), LedState::new(Rgb::WHITE, LedAnimation::Pulse(rate)));
            let blink = messages(pad(0), LedState::new(Rgb::WHITE, LedAnimation::Blink(rate)));
            assert!((6..=10).contains(&(pulse[1][0] & 0x0F)));
            assert!((11..=15).contains(&(blink[1][0] & 0x0F)));
        }
    }

    #[test]
    fn a_light_change_never_exceeds_the_declared_message_budget() {
        for animation in [
            LedAnimation::Solid,
            LedAnimation::Pulse(AnimationRate::Half),
            LedAnimation::Blink(AnimationRate::Half),
        ] {
            let out = messages(pad(0), LedState::new(Rgb::WHITE, animation));
            assert!(out.len() <= MAX_MESSAGES_PER_LED);
        }
    }

    #[test]
    fn controls_without_a_light_are_reported_rather_than_silently_skipped() {
        let mut out = Vec::new();
        let error = led_messages(
            ControlId::Encoder(EncoderId::Master),
            LedState::solid(Rgb::WHITE),
            &mut out,
        )
        .expect_err("encoders have no addressable light");
        assert!(matches!(error, PushSurfaceError::NotIlluminated { .. }));
        assert!(
            out.is_empty(),
            "a rejected control must not leave partial output"
        );

        assert!(led_off(ControlId::TouchStrip).is_none());
    }

    #[test]
    fn every_emitted_status_byte_is_a_valid_channel_message() {
        let out = messages(
            pad(0),
            LedState::new(Rgb::WHITE, LedAnimation::Pulse(AnimationRate::Half)),
        );
        for message in out {
            assert!(message[0] & 0x80 != 0, "status bytes set their top bit");
            assert!(message[1] & 0x80 == 0, "data bytes do not");
            assert!(message[2] & 0x80 == 0, "data bytes do not");
        }
    }
}
