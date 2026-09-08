//! The mascot.
//!
//! Drawn as geometry rather than as text. The artwork is written in quadrant
//! block characters, and every one of those is just a rectangle, so PushOS
//! draws them directly: crisp at any size, no dependency on the typeface
//! carrying the glyphs, and each quadrant addressable for animation.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};

/// The artwork, in quadrant block characters.
///
/// Each character is two by two quadrants, so the grid below is twice as fine
/// as it looks.
const ART: &[&str] = &["▛███▜▌", "▜█████▛▘ ▘▘ ▝▝"];

/// The mascot's colour.
///
/// Warm orange, in the family Claude is drawn in, pushed a little brighter
/// because a 960 by 160 panel is read from further away than a screen is.
pub const MASCOT: Rgb = Rgb::new(226, 128, 84);

/// How far a quadrant may be dimmed at the bottom of its cycle.
const FLOOR: f32 = 0.45;

/// How many animation steps one full sweep takes.
const SWEEP_STEPS: u32 = 96;

/// A quadrant of one character cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Quadrant {
    /// Column in the quadrant grid, from the left.
    x: u16,
    /// Row in the quadrant grid, from the top.
    y: u16,
    /// Whether this quadrant twinkles rather than taking part in the sweep.
    sparkle: bool,
}

/// The mascot, resolved into the quadrants that make it up.
#[derive(Clone, Debug)]
pub struct Mascot {
    quadrants: Vec<Quadrant>,
    width: u16,
    height: u16,
}

impl Mascot {
    /// Builds the mascot from its artwork.
    pub fn new() -> Self {
        Self::from_art(ART)
    }

    fn from_art(art: &[&str]) -> Self {
        let mut quadrants = Vec::new();
        let mut width = 0;

        for (line, row) in art.iter().enumerate() {
            for (cell, character) in row.chars().enumerate() {
                let filled = quadrants_of(character);
                if filled == [false; 4] {
                    continue;
                }

                // A detached mark is a sparkle: the trailing flecks of the
                // artwork, which read better twinkling than sweeping.
                let sparkle = filled.iter().filter(|on| **on).count() == 1;

                for (index, on) in filled.iter().enumerate() {
                    if !on {
                        continue;
                    }
                    let x = u16::try_from(cell * 2 + index % 2).unwrap_or(u16::MAX);
                    let y = u16::try_from(line * 2 + index / 2).unwrap_or(u16::MAX);
                    width = width.max(x + 1);
                    quadrants.push(Quadrant { x, y, sparkle });
                }
            }
        }

        let height = u16::try_from(art.len() * 2).unwrap_or(0);
        Self {
            quadrants,
            width,
            height,
        }
    }

    /// The mascot's size in quadrants.
    pub const fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// How many quadrants make up the artwork.
    pub fn quadrant_count(&self) -> usize {
        self.quadrants.len()
    }

    /// The size in pixels at a given quadrant size, including the gaps.
    pub fn measure(&self, quadrant: f32, gap: f32) -> (f32, f32) {
        let span = |count: u16| f32::from(count) * (quadrant + gap) - gap;
        (span(self.width), span(self.height))
    }

    /// Draws the mascot with its top-left corner at `origin`.
    ///
    /// `phase` advances the animation. It is taken as a plain counter so the
    /// caller decides the frame rate and the mascot has no clock of its own.
    pub fn draw(
        &self,
        canvas: &mut Canvas,
        origin: (f32, f32),
        quadrant: f32,
        gap: f32,
        phase: u32,
        color: Rgb,
    ) {
        let (left, top) = origin;
        let step = quadrant + gap;

        for cell in &self.quadrants {
            let area = Area::new(
                left + f32::from(cell.x) * step,
                top + f32::from(cell.y) * step,
                quadrant,
                quadrant,
            );
            canvas.fill_rounded(
                area,
                quadrant * 0.18,
                color.dimmed(self.brightness(*cell, phase)),
            );
        }
    }

    /// How bright one quadrant is at a given point in the animation.
    fn brightness(&self, cell: Quadrant, phase: u32) -> f32 {
        if cell.sparkle {
            // Sparkles blink out of step with each other, so the flecks look
            // scattered rather than synchronised.
            let offset = u32::from(cell.x) * 7 + u32::from(cell.y) * 13;
            let lit = (phase + offset) % SWEEP_STEPS < SWEEP_STEPS / 2;
            return if lit { 1.0 } else { FLOOR * 0.5 };
        }

        // A highlight travels left to right across the body and wraps.
        let width = f32::from(self.width.max(1));
        #[allow(clippy::cast_precision_loss)]
        let position = (phase % SWEEP_STEPS) as f32 / SWEEP_STEPS as f32 * (width + 4.0) - 2.0;
        let distance = (f32::from(cell.x) - position).abs();
        let nearness = (1.0 - distance / 3.0).clamp(0.0, 1.0);

        FLOOR + (1.0 - FLOOR) * nearness
    }
}

impl Default for Mascot {
    fn default() -> Self {
        Self::new()
    }
}

/// Which quadrants a block character fills, as top-left, top-right,
/// bottom-left, bottom-right.
const fn quadrants_of(character: char) -> [bool; 4] {
    match character {
        '\u{2588}' => [true, true, true, true],    // full block
        '\u{259B}' => [true, true, true, false],   // upper left, upper right, lower left
        '\u{259C}' => [true, true, false, true],   // upper left, upper right, lower right
        '\u{2599}' => [true, false, true, true],   // upper left, lower left, lower right
        '\u{259F}' => [false, true, true, true],   // upper right, lower left, lower right
        '\u{258C}' => [true, false, true, false],  // left half
        '\u{2590}' => [false, true, false, true],  // right half
        '\u{2580}' => [true, true, false, false],  // upper half
        '\u{2584}' => [false, false, true, true],  // lower half
        '\u{2598}' => [true, false, false, false], // upper left
        '\u{259D}' => [false, true, false, false], // upper right
        '\u{2596}' => [false, false, true, false], // lower left
        '\u{2597}' => [false, false, false, true], // lower right
        _ => [false; 4],
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::DisplayFrame;

    use super::*;

    #[test]
    fn the_artwork_resolves_into_quadrants() {
        let mascot = Mascot::new();
        let (width, height) = mascot.size();

        // The longer line is fourteen characters ending in a right-hand
        // quadrant, so twenty-eight across, and two lines is four down.
        assert_eq!(width, 28);
        assert_eq!(height, 4);
        assert!(mascot.quadrant_count() > 30, "the body should be solid");
    }

    #[test]
    fn a_full_block_fills_all_four_quadrants_and_a_space_fills_none() {
        assert_eq!(quadrants_of('\u{2588}'), [true, true, true, true]);
        assert_eq!(quadrants_of(' '), [false; 4]);
        assert_eq!(quadrants_of('x'), [false; 4]);
    }

    #[test]
    fn every_quadrant_character_in_the_artwork_is_understood() {
        for line in ART {
            for character in line.chars() {
                if character == ' ' {
                    continue;
                }
                assert_ne!(
                    quadrants_of(character),
                    [false; 4],
                    "`{character}` is in the artwork but draws nothing"
                );
            }
        }
    }

    #[test]
    fn a_half_block_fills_the_half_it_names() {
        assert_eq!(quadrants_of('\u{258C}'), [true, false, true, false]);
        assert_eq!(quadrants_of('\u{2590}'), [false, true, false, true]);
        assert_eq!(quadrants_of('\u{2580}'), [true, true, false, false]);
        assert_eq!(quadrants_of('\u{2584}'), [false, false, true, true]);
    }

    #[test]
    fn measuring_accounts_for_the_gaps_between_quadrants() {
        let mascot = Mascot::new();
        let (width, height) = mascot.measure(10.0, 2.0);

        // Twenty-eight quadrants of ten with twenty-seven two-pixel gaps.
        assert!((width - (28.0 * 12.0 - 2.0)).abs() < f32::EPSILON);
        assert!((height - (4.0 * 12.0 - 2.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn brightness_stays_inside_its_range_at_every_point_in_the_cycle() {
        let mascot = Mascot::new();
        for phase in 0..SWEEP_STEPS * 3 {
            for cell in &mascot.quadrants {
                let brightness = mascot.brightness(*cell, phase);
                assert!(
                    (0.0..=1.0).contains(&brightness),
                    "phase {phase} gave {brightness}"
                );
            }
        }
    }

    #[test]
    fn the_animation_actually_changes_what_is_drawn() {
        let mascot = Mascot::new();
        let mut first = DisplayFrame::blank();
        let mut later = DisplayFrame::blank();

        for (phase, frame) in [(0, &mut first), (SWEEP_STEPS / 3, &mut later)] {
            let mut canvas = Canvas::new().expect("the display size is valid");
            canvas.clear(Rgb::BLACK);
            mascot.draw(&mut canvas, (40.0, 40.0), 12.0, 3.0, phase, MASCOT);
            canvas.present_into(frame);
        }

        assert_ne!(
            first.pixels(),
            later.pixels(),
            "the sweep should have moved"
        );
    }

    #[test]
    fn the_animation_returns_to_where_it_started() {
        let mascot = Mascot::new();
        let mut first = DisplayFrame::blank();
        let mut wrapped = DisplayFrame::blank();

        for (phase, frame) in [(5, &mut first), (5 + SWEEP_STEPS, &mut wrapped)] {
            let mut canvas = Canvas::new().expect("the display size is valid");
            canvas.clear(Rgb::BLACK);
            mascot.draw(&mut canvas, (40.0, 40.0), 12.0, 3.0, phase, MASCOT);
            canvas.present_into(frame);
        }

        assert_eq!(first.pixels(), wrapped.pixels(), "one cycle should close");
    }

    #[test]
    fn drawing_off_the_edge_does_not_panic() {
        let mascot = Mascot::new();
        let mut canvas = Canvas::new().expect("the display size is valid");
        mascot.draw(&mut canvas, (-500.0, -500.0), 12.0, 3.0, 0, MASCOT);
        mascot.draw(&mut canvas, (5_000.0, 5_000.0), 12.0, 3.0, 0, MASCOT);
    }

    #[test]
    fn the_mascot_is_drawn_in_a_warm_orange() {
        const { assert!(MASCOT.r > MASCOT.g, "orange is red-dominant") };
        const { assert!(MASCOT.g > MASCOT.b, "and warmer than it is blue") };
    }
}
