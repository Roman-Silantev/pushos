//! System-exclusive message construction.

use pushos_domain::color::Rgb;

use crate::identity::{PortRole, SYSEX_END, SYSEX_PREFIX};

/// The `Set MIDI Mode` command identifier.
const CMD_SET_MIDI_MODE: u8 = 0x0A;
/// The `Set LED Color Palette Entry` command identifier.
const CMD_SET_PALETTE_ENTRY: u8 = 0x03;
/// The `Reapply Color Palette` command identifier.
const CMD_REAPPLY_PALETTE: u8 = 0x05;
/// The `Set LED Brightness` command identifier.
const CMD_SET_LED_BRIGHTNESS: u8 = 0x06;
/// The `Set Display Brightness` command identifier.
const CMD_SET_DISPLAY_BRIGHTNESS: u8 = 0x08;
/// The brightest the lights go, as the hardware counts.
const LED_BRIGHTNESS_MAX: u32 = 127;
/// The brightest the backlight goes, as the hardware counts.
const DISPLAY_BRIGHTNESS_MAX: u32 = 255;

/// The `Set Touch Strip Configuration` command identifier.
const CMD_SET_TOUCH_STRIP: u8 = 0x17;
/// The `Set Touch Strip LEDs` command identifier.
const CMD_SET_TOUCH_STRIP_LEDS: u8 = 0x19;

/// The configuration that hands PushOS the touch strip's lights.
///
/// Bit 0 takes the lights off the Push and gives them to the host; bit 1 says
/// the host will set them by system-exclusive rather than by sending values.
/// Together they mean the strip lights nothing on its own, which is what lets
/// PushOS hold it dark.
const TOUCH_STRIP_HOST_LEDS: u8 = 0b000_0011;

/// How many lights the strip has.
const TOUCH_STRIP_LEDS: usize = 31;

/// How many of them one system-exclusive byte carries.
///
/// Three bits each, two to a byte, because a data byte cannot use its top bit.
const LEDS_PER_BYTE: usize = 2;

/// Builds the message that routes control traffic to one MIDI port.
pub(crate) fn set_midi_mode(role: PortRole) -> Vec<u8> {
    message(CMD_SET_MIDI_MODE, &[role.midi_mode()])
}

/// Builds the message that redefines one entry of the colour palette.
///
/// Each channel is split into a low seven bits and a high bit, because
/// system-exclusive data bytes cannot have their top bit set. The white value
/// is what buttons without an RGB LED actually display.
pub(crate) fn set_palette_entry(index: u8, color: Rgb, white: u8) -> Vec<u8> {
    let mut args = Vec::with_capacity(9);
    args.push(index & 0x7F);
    for channel in [color.r, color.g, color.b, white] {
        args.push(channel & 0x7F);
        args.push(channel >> 7);
    }
    message(CMD_SET_PALETTE_ENTRY, &args)
}

/// Builds the message that applies palette changes to lights already lit.
pub(crate) fn reapply_palette() -> Vec<u8> {
    message(CMD_REAPPLY_PALETTE, &[])
}

/// Builds the message that sets every light's brightness, from a percentage.
///
/// The hardware counts nought to 127. On USB power alone it quietly caps this
/// at 8 whatever is asked for, to stay inside the current USB allows.
pub(crate) fn set_led_brightness(percent: u8) -> Vec<u8> {
    message(
        CMD_SET_LED_BRIGHTNESS,
        &[scaled(percent, LED_BRIGHTNESS_MAX)],
    )
}

/// Builds the message that sets the screen's backlight, from a percentage.
///
/// The hardware counts nought to 255, which does not fit in a data byte, so
/// it goes as the low seven bits and then the high one.
pub(crate) fn set_display_brightness(percent: u8) -> Vec<u8> {
    let value = scaled(percent, DISPLAY_BRIGHTNESS_MAX);
    message(CMD_SET_DISPLAY_BRIGHTNESS, &[value & 0x7F, value >> 7])
}

/// A percentage of the hardware's range, rounded to the nearest step.
///
/// Rounded rather than truncated so that a hundred percent is the maximum and
/// any percentage above nought lights something.
fn scaled(percent: u8, max: u32) -> u8 {
    let percent = u32::from(percent.min(100));
    let value = (percent * max + 50) / 100;
    let value = if percent > 0 { value.max(1) } else { 0 };
    u8::try_from(value.min(max)).unwrap_or(u8::MAX)
}

/// Builds the message that takes the touch strip's lights off the Push.
///
/// Sent so that [`darken_touch_strip`] is obeyed. Left to itself the Push
/// lights the strip wherever a finger lands, and PushOS has nothing for the
/// strip to do.
pub(crate) fn host_owns_touch_strip() -> Vec<u8> {
    message(CMD_SET_TOUCH_STRIP, &[TOUCH_STRIP_HOST_LEDS])
}

/// Builds the message that puts every light on the touch strip out.
///
/// Colour index zero is black in the strip's own palette, so a body of zeroes
/// is a dark strip. Only meaningful once [`host_owns_touch_strip`] has been
/// sent.
pub(crate) fn darken_touch_strip() -> Vec<u8> {
    let bytes = TOUCH_STRIP_LEDS.div_ceil(LEDS_PER_BYTE);
    message(CMD_SET_TOUCH_STRIP_LEDS, &vec![0; bytes])
}

fn message(command: u8, args: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(SYSEX_PREFIX.len() + 2 + args.len());
    bytes.extend_from_slice(&SYSEX_PREFIX);
    bytes.push(command);
    bytes.extend_from_slice(args);
    bytes.push(SYSEX_END);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_user_mode_matches_the_documented_example() {
        assert_eq!(
            set_midi_mode(PortRole::User),
            [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]
        );
    }

    #[test]
    fn a_palette_entry_matches_the_documented_example() {
        // The manual's worked example sets entry 125 to pure blue with white 126.
        let message = set_palette_entry(125, Rgb::new(0, 0, 255), 126);
        assert_eq!(
            message,
            [
                0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, //
                0x03, // set palette entry
                0x7D, // index 125
                0x00, 0x00, // red 0
                0x00, 0x00, // green 0
                0x7F, 0x01, // blue 255
                0x7E, 0x00, // white 126
                0xF7,
            ]
        );
    }

    #[test]
    fn every_message_stays_inside_the_seven_bit_data_range() {
        let message = set_palette_entry(255, Rgb::new(255, 255, 255), 255);
        let body = &message[1..message.len() - 1];
        assert!(
            body.iter().all(|byte| byte & 0x80 == 0),
            "system-exclusive data bytes must never set their top bit"
        );
    }

    #[test]
    fn light_brightness_matches_the_documented_example() {
        // The manual's example sets 64 of 127; half is the nearest percentage.
        assert_eq!(
            set_led_brightness(50),
            [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x06, 0x40, 0xF7]
        );
        assert_eq!(set_led_brightness(100)[7], 127);
        assert_eq!(set_led_brightness(0)[7], 0);
    }

    #[test]
    fn screen_brightness_matches_the_documented_example() {
        // The manual's example sets 255, split as 0x7F then 0x01.
        assert_eq!(
            set_display_brightness(100),
            [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x08, 0x7F, 0x01, 0xF7]
        );
        assert_eq!(
            &set_display_brightness(0)[7..9],
            [0x00, 0x00],
            "nought is a dark screen"
        );
    }

    #[test]
    fn any_brightness_above_nought_lights_something() {
        assert!(set_led_brightness(1)[7] > 0);
        let dim = set_display_brightness(1);
        assert!(dim[7] > 0 || dim[8] > 0);
    }

    #[test]
    fn the_touch_strip_is_taken_over_and_put_out() {
        // The manual's own worked example for host-controlled lights.
        assert_eq!(
            host_owns_touch_strip(),
            [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x17, 0x03, 0xF7]
        );

        // Thirty-one lights, two to a byte, all of them black.
        let dark = darken_touch_strip();
        assert_eq!(dark.len(), SYSEX_PREFIX.len() + 2 + 16);
        assert!(
            dark[SYSEX_PREFIX.len() + 1..dark.len() - 1]
                .iter()
                .all(|byte| *byte == 0),
            "a strip PushOS does not use should not be lit"
        );
    }

    #[test]
    fn reapplying_the_palette_takes_no_arguments() {
        assert_eq!(
            reapply_palette(),
            [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
        );
    }
}
