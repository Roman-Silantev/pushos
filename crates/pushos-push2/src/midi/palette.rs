//! The colour palette PushOS installs on the device.
//!
//! Push 2 lights take a palette index, not a colour, so PushOS replaces the
//! factory palette with one of its own: a uniform 5x5x5 cube. That makes the
//! colour-to-index step exact arithmetic rather than a search, and it gives the
//! white-only buttons a sensible brightness for free, because each entry's
//! white value is set to the luminance of its colour.

use pushos_domain::color::Rgb;

/// Quantisation levels per channel.
const LEVELS: u8 = 5;
/// How many palette entries the cube occupies.
pub(crate) const CUBE_ENTRIES: u8 = LEVELS * LEVELS * LEVELS;
/// How many entries the hardware palette holds in total.
pub(crate) const PALETTE_ENTRIES: u8 = 128;

/// The colour stored at a palette index.
///
/// Indices beyond the cube are unused and stay black.
pub(crate) fn entry(index: u8) -> Rgb {
    if index >= CUBE_ENTRIES {
        return Rgb::BLACK;
    }
    let red = index / (LEVELS * LEVELS);
    let green = (index / LEVELS) % LEVELS;
    let blue = index % LEVELS;
    Rgb::new(level_value(red), level_value(green), level_value(blue))
}

/// The palette index that best represents a colour.
///
/// The cube is uniform, so this is a rounding operation with no search and no
/// allocation, which keeps it usable on the LED update path.
pub(crate) fn index_of(color: Rgb) -> u8 {
    quantise(color.r) * LEVELS * LEVELS + quantise(color.g) * LEVELS + quantise(color.b)
}

/// The white-LED brightness stored alongside a palette entry.
///
/// Buttons without an RGB light resolve their index through the white palette,
/// so mirroring the colour's luminance here means one index means the same
/// thing on every control.
pub(crate) fn white_value(color: Rgb) -> u8 {
    // Rec. 601 luma, in integer arithmetic.
    let luma = (u16::from(color.r) * 77 + u16::from(color.g) * 150 + u16::from(color.b) * 29) >> 8;
    u8::try_from(luma).unwrap_or(u8::MAX)
}

/// Rounds a channel to its nearest quantisation level.
fn quantise(channel: u8) -> u8 {
    let scaled = (u16::from(channel) * u16::from(LEVELS - 1) + 127) / 255;
    u8::try_from(scaled).unwrap_or(LEVELS - 1)
}

/// The colour value a quantisation level represents.
fn level_value(level: u8) -> u8 {
    let value = (u16::from(level) * 255 + u16::from(LEVELS - 1) / 2) / u16::from(LEVELS - 1);
    u8::try_from(value).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_is_index_zero_so_an_unassigned_control_is_dark() {
        assert_eq!(index_of(Rgb::BLACK), 0);
        assert_eq!(entry(0), Rgb::BLACK);
    }

    #[test]
    fn the_extremes_of_each_channel_are_represented_exactly() {
        assert_eq!(entry(index_of(Rgb::WHITE)), Rgb::WHITE);
        assert_eq!(entry(index_of(Rgb::new(255, 0, 0))), Rgb::new(255, 0, 0));
        assert_eq!(entry(index_of(Rgb::new(0, 255, 0))), Rgb::new(0, 255, 0));
        assert_eq!(entry(index_of(Rgb::new(0, 0, 255))), Rgb::new(0, 0, 255));
    }

    #[test]
    fn every_index_the_cube_produces_stays_inside_the_palette() {
        for r in 0..=255u8 {
            for channel in [Rgb::new(r, 0, 0), Rgb::new(0, r, 0), Rgb::new(0, 0, r)] {
                assert!(index_of(channel) < CUBE_ENTRIES);
            }
        }
        assert!(index_of(Rgb::WHITE) < CUBE_ENTRIES);
    }

    #[test]
    fn quantisation_error_stays_within_one_cube_step() {
        // The cube has five levels per channel, so no channel may be off by
        // more than half a step, which is 32 of 255.
        let mut worst = 0i32;
        for value in 0..=255u8 {
            let round_tripped = entry(index_of(Rgb::new(value, value, value)));
            worst = worst.max((i32::from(round_tripped.r) - i32::from(value)).abs());
        }
        assert!(worst <= 32, "worst-case channel error was {worst}");
    }

    #[test]
    fn white_brightness_tracks_luminance() {
        assert_eq!(white_value(Rgb::BLACK), 0);
        assert!(white_value(Rgb::WHITE) > 250);
        assert!(white_value(Rgb::new(0, 255, 0)) > white_value(Rgb::new(0, 0, 255)));
    }

    #[test]
    fn entries_beyond_the_cube_are_black() {
        for index in CUBE_ENTRIES..PALETTE_ENTRIES {
            assert_eq!(entry(index), Rgb::BLACK);
        }
    }
}
