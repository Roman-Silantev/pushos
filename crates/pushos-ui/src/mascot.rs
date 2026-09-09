//! The mascot.
//!
//! Drawn as geometry rather than as text. The artwork is written in quadrant
//! block characters, and every one of those is just a rectangle, so PushOS
//! draws them directly: crisp at any size, no dependency on the typeface
//! carrying the glyphs, and each quadrant addressable for animation.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};

/// How much taller than wide one quadrant is drawn.
///
/// The artwork was written in terminal block characters, and a terminal cell
/// is about twice as tall as it is wide. Drawn as squares the mascot comes out
/// seven times wider than it is tall, which reads as a strip cut out of
/// something rather than as a shape. Drawn at the proportions it was written
/// for, it is the shape somebody drew.
const TALLNESS: f32 = 2.0;

/// The mascot, one character per quadrant.
///
/// Clawd, as the Claude Code command line draws him: three lines of block
/// characters, with his feet on the third. Drawn here as the quadrants those
/// characters are made of, because every block element is a rectangle and
/// PushOS can draw a rectangle at any size crisply, which a typeface carrying
/// the glyphs could not.
///
/// The feet were once on the same line as the body, which made him two lines
/// tall with his feet trailing off to the right. He read as a strip with
/// debris beside it, which is exactly what that is.
///
/// `#` is the body, which sweeps. `+` is a foot, which twinkles. Both hop:
/// they are the same creature, and feet that stayed on the ground while he
/// jumped would read as him coming apart.
///
/// The feet are spaced as two pairs standing under him rather than where the
/// character cells happened to put them. A terminal has no choice about that
/// and PushOS does, and a creature whose legs were all bunched under one end
/// looks like it is falling over.
const ART: &[&str] = &[
    ".############...",
    ".##.######.##...",
    "###############.",
    ".############...",
    "..+.+.....+.+...",
];

/// The star, one character per quadrant.
///
/// A burst of four long rays and four short ones. It pulses where the mascot
/// hops: it stands for something being worked on rather than for PushOS
/// itself, and a status light that jumped about would be harder to read at a
/// glance than one that breathes.
const STAR: &[&str] = &[
    "....+....",
    "....#....",
    "..+.#.+..",
    "...###...",
    "+#######+",
    "...###...",
    "..+.#.+..",
    "....#....",
    "....+....",
];

/// The mascot's colour.
///
/// Warm orange, in the family Claude is drawn in, pushed a little brighter
/// because a 960 by 160 panel is read from further away than a screen is.
pub const MASCOT: Rgb = Rgb::new(226, 128, 84);

/// How far a quadrant may be dimmed at the bottom of its cycle.
const FLOOR: f32 = 0.45;

/// How many animation steps one full sweep takes.
const SWEEP_STEPS: u32 = 96;

/// How many steps one hop and the pause after it take.
///
/// A whole number of hops to one sweep, so the animation has a single period
/// and returns to exactly where it started. Two hops per sweep: often enough
/// to be alive, seldom enough to read past.
const HOP_STEPS: u32 = SWEEP_STEPS / 2;

/// How much of that is spent in the air.
///
/// Well under half, because a mascot that never rested would be a distraction
/// on a panel somebody is trying to read.
const HOP_AIRBORNE: u32 = 18;

/// How high it goes, in quadrants.
const HOP_HEIGHT: f32 = 2.4;

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
    /// Whether it leaves the ground.
    hops: bool,
    /// How much taller than wide each quadrant is drawn.
    tallness: f32,
}

impl Mascot {
    /// Builds the mascot from its artwork.
    pub fn new() -> Self {
        Self::from_art(ART)
    }

    /// Builds the star.
    ///
    /// The same geometry, a different shape and a different movement: this one
    /// breathes rather than hops, because it stands for something being worked
    /// on and a status light that jumped about would be harder to read.
    pub fn star() -> Self {
        let mut star = Self::from_art(STAR);
        star.hops = false;
        // Square, unlike the mascot: this one was drawn here rather than in a
        // terminal, and a burst has to be round to read as one.
        star.tallness = 1.0;
        star
    }

    fn from_art(art: &[&str]) -> Self {
        let mut quadrants = Vec::new();
        let mut width = 0;

        for (line, row) in art.iter().enumerate() {
            for (cell, character) in row.chars().enumerate() {
                let sparkle = match character {
                    '#' => false,
                    '+' => true,
                    // Anything else is space. A grid is easier to read with
                    // dots than with blanks, and easier to keep aligned.
                    _ => continue,
                };

                let x = u16::try_from(cell).unwrap_or(u16::MAX);
                let y = u16::try_from(line).unwrap_or(u16::MAX);
                width = width.max(x + 1);
                quadrants.push(Quadrant { x, y, sparkle });
            }
        }

        let height = u16::try_from(art.len()).unwrap_or(0);
        Self {
            quadrants,
            hops: true,
            tallness: TALLNESS,
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
        let across = f32::from(self.width) * (quadrant + gap) - gap;
        let down = f32::from(self.height) * (quadrant * self.tallness + gap) - gap;
        (across, down)
    }

    /// How far off the ground the mascot is, in quadrants.
    ///
    /// A hop and then a rest, rather than a constant bounce: something that
    /// never stopped moving would be a distraction on a panel somebody is
    /// trying to read past, and the point of the movement is that it is
    /// noticed.
    pub fn lift(phase: u32) -> f32 {
        let step = phase % HOP_STEPS;
        if step >= HOP_AIRBORNE {
            return 0.0;
        }

        // A parabola: fastest leaving the ground, slowest at the top.
        #[allow(clippy::cast_precision_loss)]
        let through = step as f32 / HOP_AIRBORNE as f32;
        HOP_HEIGHT * 4.0 * through * (1.0 - through)
    }

    /// Draws the mascot with its top-left corner at `origin`.
    ///
    /// `phase` advances the animation. It is taken as a plain counter so the
    /// caller decides the frame rate and the mascot has no clock of its own.
    /// A phase of zero is the mascot at rest, so a caller that does not want it
    /// moving simply never advances it.
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
        let tall = quadrant * self.tallness;
        let down = tall + gap;
        // All of him together. Feet that stayed on the ground while the body
        // jumped read as him coming apart, which is what a mascot drawn in
        // separate blocks must never be allowed to look like. The star does
        // not hop at all.
        let hop = if self.hops {
            Self::lift(phase) * down
        } else {
            0.0
        };

        for cell in &self.quadrants {
            let area = Area::new(
                left + f32::from(cell.x) * step,
                top + f32::from(cell.y) * down - hop,
                quadrant,
                tall,
            );
            canvas.fill_rounded(
                area,
                quadrant * 0.22,
                color.dimmed(self.brightness(*cell, phase)),
            );
        }
    }

    /// How bright one quadrant is at a given point in the animation.
    ///
    /// The mascot has a highlight travelling across it. The star breathes
    /// instead: the whole body rises and falls together, which is what a thing
    /// that is working looks like and what a thing that is not does not.
    fn brightness(&self, cell: Quadrant, phase: u32) -> f32 {
        if !self.hops && !cell.sparkle {
            #[allow(clippy::cast_precision_loss)]
            let through = (phase % SWEEP_STEPS) as f32 / SWEEP_STEPS as f32;
            let breath = (through * std::f32::consts::TAU).cos();
            return FLOOR + (1.0 - FLOOR) * (0.5 + breath / 2.0);
        }

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
        assert_eq!(width, 15, "as wide as the artwork he actually fills");
        assert_eq!(height, 5, "and as tall");
        assert!(mascot.quadrant_count() > 20, "the body should be solid");
    }

    #[test]
    fn the_artwork_is_a_rectangle() {
        // A row that drifted would tilt the burst, and it would be a while
        // before anybody worked out why.
        let widest = ART.iter().map(|row| row.chars().count()).max().unwrap_or(0);
        for (index, row) in ART.iter().enumerate() {
            assert_eq!(
                row.chars().count(),
                widest,
                "row {index} is a different length"
            );
        }
    }

    #[test]
    fn the_artwork_uses_only_marks_the_mascot_knows() {
        for art in [ART, STAR] {
            for line in art {
                for character in line.chars() {
                    assert!(
                        matches!(character, '#' | '+' | '.'),
                        "`{character}` is in the artwork and draws nothing"
                    );
                }
            }
        }
    }

    #[test]
    fn the_star_is_symmetrical() {
        // It is a mark rather than a picture, and a lopsided one would look
        // like a mistake at every size.
        for row in STAR {
            let forwards: Vec<char> = row.chars().collect();
            let backwards: Vec<char> = row.chars().rev().collect();
            assert_eq!(forwards, backwards, "`{row}` is not the same both ways");
        }

        let flipped: Vec<&&str> = STAR.iter().rev().collect();
        let upright: Vec<&&str> = STAR.iter().collect();
        assert_eq!(upright, flipped, "the star is not the same way up");
    }

    #[test]
    fn the_mascot_is_drawn_at_the_proportions_it_was_written_for() {
        // Written in terminal block characters, and a terminal cell is about
        // twice as tall as it is wide. Drawn as squares it comes out seven
        // times wider than it is tall and reads as a strip cut out of
        // something rather than as a shape.
        let mascot = Mascot::new();
        let (width, height) = mascot.measure(4.0, 1.0);
        assert!(width / height < 4.0, "{width} by {height} is still a slab");
    }

    #[test]
    fn clawd_stays_in_one_piece_when_he_jumps() {
        // Feet that stayed on the ground while the body left it read as him
        // coming apart, which at the size a header allows is indistinguishable
        // from a rendering fault.
        let mascot = Mascot::new();
        let mut first = DisplayFrame::blank();
        let mut mid_hop = DisplayFrame::blank();

        for (phase, frame) in [(0, &mut first), (HOP_AIRBORNE / 2, &mut mid_hop)] {
            let mut canvas = Canvas::new().expect("the display size is valid");
            canvas.clear(Rgb::BLACK);
            mascot.draw(&mut canvas, (40.0, 60.0), 6.0, 2.0, phase, MASCOT);
            canvas.present_into(frame);
        }

        // Every lit row moves up together, so the gap between his body and his
        // feet is the same on the ground as in the air.
        let rows = |frame: &DisplayFrame| -> Vec<usize> {
            let width = pushos_domain::ports::DISPLAY_WIDTH;
            (0..pushos_domain::ports::DISPLAY_HEIGHT)
                .filter(|y| (0..width).any(|x| frame.pixel(x, *y).is_some_and(|pixel| pixel != 0)))
                .collect()
        };

        let (grounded, airborne) = (rows(&first), rows(&mid_hop));
        let span = |rows: &[usize]| rows[rows.len() - 1] - rows[0];

        // Within a pixel, because a hop of a fractional height lands the
        // blocks on slightly different rows and softens their edges.
        assert!(
            span(&grounded).abs_diff(span(&airborne)) <= 1,
            "he should be the same height in the air: {} then {}",
            span(&grounded),
            span(&airborne)
        );
        assert!(
            airborne[0] < grounded[0],
            "and further off the ground: {} then {}",
            grounded[0],
            airborne[0]
        );
    }

    #[test]
    fn clawd_has_his_feet_under_him_and_stands_square() {
        // They were once on the same line as his body, trailing off to the
        // right, which made him a strip with debris beside it. Bunched under
        // one end they made him look like he was falling over.
        let mascot = Mascot::new();
        let feet: Vec<&Quadrant> = mascot
            .quadrants
            .iter()
            .filter(|cell| cell.sparkle)
            .collect();
        let body: Vec<&Quadrant> = mascot
            .quadrants
            .iter()
            .filter(|cell| !cell.sparkle)
            .collect();

        assert!(!feet.is_empty(), "he has feet");
        let lowest_body = body.iter().map(|cell| cell.y).max().expect("a body");
        assert!(
            feet.iter().all(|cell| cell.y > lowest_body),
            "and every one of them is under him"
        );

        // Balanced about his middle, within a quadrant.
        let span = |cells: &[&Quadrant]| {
            let (least, most) = (
                cells.iter().map(|c| c.x).min().unwrap_or(0),
                cells.iter().map(|c| c.x).max().unwrap_or(0),
            );
            f32::from(least + most) / 2.0
        };
        assert!(
            (span(&feet) - span(&body)).abs() <= 1.0,
            "he is standing off to one side: feet at {}, body at {}",
            span(&feet),
            span(&body)
        );
    }

    #[test]
    fn the_star_is_drawn_square() {
        // It was drawn here rather than in a terminal, and a burst has to be
        // round to read as one.
        let star = Mascot::star();
        let (width, height) = star.measure(4.0, 1.0);
        assert!((width - height).abs() < f32::EPSILON, "{width} by {height}");
    }

    #[test]
    fn the_mascot_hops_and_the_star_does_not() {
        // The star stands for the work rather than for PushOS, and a status
        // light that jumped about would be harder to read at a glance than one
        // that breathes.
        assert!(Mascot::new().hops);
        assert!(!Mascot::star().hops);
    }

    #[test]
    fn the_star_breathes_rather_than_being_swept_across() {
        // A highlight travelling across it would say "something is moving
        // through here"; rising and falling together says "this is alive".
        let star = Mascot::star();
        let body = star
            .quadrants
            .iter()
            .find(|cell| !cell.sparkle)
            .copied()
            .expect("the star has a body");

        let brightest = star.brightness(body, 0);
        let dimmest = star.brightness(body, SWEEP_STEPS / 2);
        assert!(brightest > dimmest, "it should rise and fall");

        // Every part of the body together, rather than one part at a time.
        for cell in star.quadrants.iter().filter(|cell| !cell.sparkle) {
            assert!(
                (star.brightness(*cell, 0) - brightest).abs() < f32::EPSILON,
                "the whole body should breathe together"
            );
        }
    }

    #[test]
    fn measuring_accounts_for_the_gaps_between_quadrants() {
        let mascot = Mascot::new();
        let (width, height) = mascot.measure(10.0, 2.0);

        // Fifteen quadrants of ten with fourteen two-pixel gaps across, and
        // five of twenty down, because the artwork was written for terminal
        // cells and those are twice as tall as they are wide.
        assert!((width - (15.0 * 12.0 - 2.0)).abs() < f32::EPSILON);
        assert!((height - (5.0 * 22.0 - 2.0)).abs() < f32::EPSILON);
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

    #[test]
    fn the_mascot_hops_and_then_rests() {
        // Something that never stopped moving would be a distraction on a
        // panel somebody is trying to read past.
        assert!(grounded(Mascot::lift(0)), "it starts on the ground");
        assert!(Mascot::lift(HOP_AIRBORNE / 2) > 1.0, "and gets well off it");
        assert!(grounded(Mascot::lift(HOP_AIRBORNE)), "and lands");
        assert!(
            (HOP_AIRBORNE..HOP_STEPS).all(|step| grounded(Mascot::lift(step))),
            "and then stays there for a while"
        );
    }

    /// Whether the mascot is on the ground, without comparing floats exactly.
    fn grounded(lift: f32) -> bool {
        lift.abs() < f32::EPSILON
    }

    #[test]
    fn the_hop_repeats_a_whole_number_of_times_per_sweep() {
        // Otherwise the animation would have two periods and never return to
        // exactly where it started, which is a drift nobody would ever notice
        // and everybody would eventually see.
        assert_eq!(SWEEP_STEPS % HOP_STEPS, 0);
        for cycle in 0..4 {
            assert!(grounded(Mascot::lift(cycle * HOP_STEPS)));
            assert!(Mascot::lift(cycle * HOP_STEPS + HOP_AIRBORNE / 2) > 1.0);
        }
    }

    #[test]
    fn a_mascot_that_is_never_advanced_never_moves() {
        // Which is what a caller showing it as a static mark relies on.
        assert!(grounded(Mascot::lift(0)));
    }
}
