//! Text rendering.
//!
//! Glyphs are rasterised once per size and kept, because the render loop must
//! not allocate. Text that will not fit is truncated with an ellipsis rather
//! than overflowing into the next slot.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};
use pushos_domain::color::Rgb;

use crate::canvas::Canvas;

/// The typeface PushOS draws with, compiled into the binary so the display
/// works before any file system is available.
const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter.ttf");

/// What is appended when text has to be cut short.
const ELLIPSIS: char = '\u{2026}';

/// What a character the typeface has no glyph for is drawn as instead.
///
/// PushOS shows text it did not write: a terminal's screen is full of box
/// rules, spinners, arrows and whatever else the program inside chose to draw
/// with. A typeface small enough to compile into the binary will not have all
/// of it, and the alternative to substituting is a row of empty rectangles,
/// which tells an operator nothing and looks broken.
///
/// Anything not listed and not in the typeface is dropped rather than
/// guessed at.
const SUBSTITUTES: &[(char, char)] = &[
    // Box drawing and rules, of which a terminal screen is mostly made.
    ('\u{2500}', '-'),
    ('\u{2501}', '-'),
    ('\u{2550}', '='),
    ('\u{2502}', '|'),
    ('\u{2503}', '|'),
    ('\u{2551}', '|'),
    // Arrows, which agents use to point at what they are waiting for.
    ('\u{2190}', '<'),
    ('\u{2192}', '>'),
    ('\u{2191}', '^'),
    ('\u{2193}', 'v'),
    ('\u{25b6}', '>'),
    ('\u{25c0}', '<'),
    ('\u{23f5}', '>'),
    // Bullets and marks.
    ('\u{2022}', '.'),
    ('\u{00b7}', '.'),
    ('\u{2713}', '+'),
    ('\u{2714}', '+'),
    ('\u{2717}', 'x'),
    ('\u{2718}', 'x'),
    ('\u{2733}', '*'),
    ('\u{2736}', '*'),
    ('\u{273b}', '*'),
    // The spinner frames a coding agent turns in its own output.
    ('\u{25d0}', 'o'),
    ('\u{25d1}', 'o'),
    ('\u{25d2}', 'o'),
    ('\u{25d3}', 'o'),
    ('\u{2588}', '#'),
    ('\u{2591}', '.'),
    ('\u{2592}', ':'),
    ('\u{2593}', '#'),
    // What a coding agent draws around its own tool calls.
    ('\u{23fa}', '*'),
    ('\u{23f8}', '='),
    ('\u{23fb}', 'o'),
    ('\u{29c9}', '#'),
    ('\u{23bf}', 'L'),
    // The prompt a coding agent draws, and the marks around what it is doing.
    // Dropping these lost the one character that says where a line begins, and
    // the lines beneath it stopped lining up with anything.
    ('\u{276f}', '>'),
    ('\u{276e}', '<'),
    ('\u{25b8}', '>'),
    ('\u{25b9}', '>'),
    ('\u{2039}', '<'),
    ('\u{203a}', '>'),
    ('\u{2026}', '.'),
];

/// How a run of text should be drawn.
///
/// Grouping these keeps [`TextRenderer::draw`] to three arguments and makes a
/// caller name what it means rather than remember an argument order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Type size in pixels.
    pub size: f32,
    /// Ink colour.
    pub color: Rgb,
    /// Where the anchor sits relative to the text.
    pub align: Align,
    /// Width the text must fit inside; longer text is truncated.
    pub max_width: f32,
}

impl TextStyle {
    /// Left-aligned text of a given size and colour.
    pub const fn left(size: f32, color: Rgb, max_width: f32) -> Self {
        Self {
            size,
            color,
            align: Align::Left,
            max_width,
        }
    }

    /// Returns a copy with a different alignment.
    #[must_use]
    pub const fn aligned(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Returns a copy in a different colour.
    #[must_use]
    pub const fn colored(mut self, color: Rgb) -> Self {
        self.color = color;
        self
    }
}

/// Where text sits relative to its anchor point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The anchor is the left edge.
    Left,
    /// The anchor is the horizontal centre.
    Centre,
    /// The anchor is the right edge.
    Right,
}

/// A measurement as a whole number, so a cache cannot be defeated by floating
/// point noise.
///
/// Anything wider than a panel could ever be reads as one figure of its own
/// rather than as the widest real one: the answers differ, and a key they
/// shared would hand one of them the other's line.
fn quantised(measure: f32) -> u32 {
    /// Past this there is nothing to tell apart: it is wider than any display
    /// PushOS draws on, and every line fits.
    const BEYOND: f32 = 4_000.0;

    if measure.is_nan() {
        return 0;
    }
    if measure > BEYOND {
        return u32::MAX;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = (measure.max(0.0) * 10.0).round() as u32;
    whole
}

/// A rasterised glyph, kept so it is only produced once.
struct Glyph {
    metrics: Metrics,
    coverage: Vec<u8>,
}

/// Hashing for keys PushOS makes itself.
///
/// The standard hasher is built to be unguessable, which costs more than the
/// lookup saves when the key is a character and a size and the answer is
/// wanted for every character of every line, thirty times a second. These keys
/// come from the typeface and the theme, never from anything outside.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Quick(u64);

impl std::hash::Hasher for Quick {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            // The usual multiply-and-mix: enough spread for a few hundred
            // glyphs and a handful of lines.
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl std::hash::BuildHasher for Quick {
    type Hasher = Self;

    fn build_hasher(&self) -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

/// Lines as they were last fitted, under the size and width they were fitted
/// to.
///
/// Nested so that looking one up costs nothing: the outer key is a pair of
/// numbers, and the map inside is found by the line itself without copying it
/// first. Every line of every frame goes through here.
type Fitted = HashMap<(u32, u32), HashMap<String, String, Quick>, Quick>;

/// The most lines whose fitting is remembered.
///
/// A screenful is a dozen or so; this is room for the pages an operator moves
/// between without ever being a store worth thinking about.
const REMEMBERED_LINES: usize = 128;

/// The most rasterised glyphs kept at once.
///
/// A display of English text uses a hundred or so across its sizes. This is
/// room for the languages a session name might be written in, and a bound on
/// what a pad named in every alphabet at once could cost.
const REMEMBERED_GLYPHS: usize = 1_024;

/// Draws text onto a canvas.
pub struct TextRenderer {
    font: Font,
    cache: HashMap<(char, u32), Glyph, Quick>,
    /// What each line came to when it was last fitted.
    ///
    /// Fitting walks the string, asks the typeface about every character and
    /// measures each one. The lines on the display change seldom; the frame
    /// they are drawn in changes many times a second.
    fitted: Fitted,
    /// How many lines are remembered, across every size and width.
    remembered: usize,
}

impl TextRenderer {
    /// Builds a renderer over the compiled-in typeface.
    pub fn embedded() -> Result<Self, FontUnavailable> {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default()).map_err(|reason| {
            FontUnavailable {
                reason: reason.to_owned(),
            }
        })?;
        Ok(Self {
            font,
            cache: HashMap::default(),
            fitted: Fitted::default(),
            remembered: 0,
        })
    }

    /// How wide `text` would be at `size`.
    pub fn width(&mut self, text: &str, size: f32) -> f32 {
        // Collected before measuring, because deciding what is drawable reads
        // the typeface and measuring writes to the glyph cache.
        let drawable: Vec<char> = text.chars().filter_map(|c| self.readable(c)).collect();
        drawable
            .into_iter()
            .map(|character| self.advance(character, size))
            .sum()
    }

    /// Whether the typeface can draw a character at all.
    ///
    /// Public so a development tool can report what a terminal draws that this
    /// typeface does not have, which is how the substitution list was written.
    pub fn can_draw(&self, character: char) -> bool {
        self.font.has_glyph(character)
    }

    /// The character to draw in place of one the typeface cannot.
    ///
    /// `None` when there is nothing sensible: a character nobody can read is
    /// worse than a gap, because a row of empty rectangles reads as a fault in
    /// PushOS rather than as a glyph somebody else chose.
    fn readable(&self, character: char) -> Option<char> {
        // A space is the one blank the typeface actually has. Every other
        // blank — a tab, and the several exotic ones that turn up in what an
        // agent prints — has no glyph, and asking for one draws the empty box
        // the typeface keeps for characters it does not know. A blank is a
        // blank on a panel this size.
        if character.is_whitespace() {
            return Some(' ');
        }
        if self.font.has_glyph(character) {
            return Some(character);
        }

        SUBSTITUTES
            .iter()
            .find(|(from, _)| *from == character)
            .map(|(_, to)| *to)
            .filter(|to| self.font.has_glyph(*to))
    }

    /// The distance from one baseline to the next at `size`.
    pub fn line_height(&self, size: f32) -> f32 {
        self.font
            .horizontal_line_metrics(size)
            .map_or(size * 1.25, |metrics| metrics.new_line_size)
    }

    /// Draws text with its baseline at `baseline`, anchored horizontally at `x`.
    ///
    /// Returns the width actually drawn.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        at: (f32, f32),
        style: TextStyle,
    ) -> f32 {
        let (x, baseline) = at;
        let fitted = self.truncate(text, style.size, style.max_width);
        let width = self.width(&fitted, style.size);
        let mut pen = match style.align {
            Align::Left => x,
            Align::Centre => x - width / 2.0,
            Align::Right => x - width,
        };

        for character in fitted.chars() {
            self.ensure(character, style.size);
            if let Some(glyph) = self.cache.get(&Self::key(character, style.size)) {
                blit(canvas, glyph, pen, baseline, style.color);
                pen += glyph.metrics.advance_width;
            }
        }
        width
    }

    /// Shortens `text` so it fits within `max_width`, appending an ellipsis.
    ///
    /// The answer is remembered: the same line is fitted again on every frame
    /// of an animation, and nothing about it has changed.
    pub fn truncate(&mut self, text: &str, size: f32, max_width: f32) -> String {
        let shape = (quantised(size), quantised(max_width));
        if let Some(fitted) = self.fitted.get(&shape).and_then(|lines| lines.get(text)) {
            return fitted.clone();
        }
        let fitted = self.fit(text, size, max_width);
        // Cleared rather than evicted one by one: it is rebuilt in a frame,
        // and a page that changed its lines wants none of the old ones.
        if self.remembered >= REMEMBERED_LINES {
            self.fitted.clear();
            self.remembered = 0;
        }
        if self
            .fitted
            .entry(shape)
            .or_default()
            .insert(text.to_owned(), fitted.clone())
            .is_none()
        {
            self.remembered += 1;
        }
        fitted
    }

    /// Works out what a line comes to at a size and width.
    fn fit(&mut self, text: &str, size: f32, max_width: f32) -> String {
        // Substituted here as well as measured, so what is drawn is exactly
        // what was measured and nothing tofu reaches the panel.
        if self.width(text, size) <= max_width {
            return text.chars().filter_map(|c| self.readable(c)).collect();
        }

        let ellipsis = self.advance(ELLIPSIS, size);
        let budget = max_width - ellipsis;
        if budget <= 0.0 {
            return String::new();
        }

        let drawable: Vec<char> = text.chars().filter_map(|c| self.readable(c)).collect();
        let mut kept = String::new();
        let mut used = 0.0;
        for character in drawable {
            let advance = self.advance(character, size);
            if used + advance > budget {
                break;
            }
            used += advance;
            kept.push(character);
        }

        if kept.is_empty() {
            return String::new();
        }
        kept.push(ELLIPSIS);
        kept
    }

    fn advance(&mut self, character: char, size: f32) -> f32 {
        self.ensure(character, size);
        self.cache
            .get(&Self::key(character, size))
            .map_or(0.0, |glyph| glyph.metrics.advance_width)
    }

    fn ensure(&mut self, character: char, size: f32) {
        let key = Self::key(character, size);
        if self.cache.contains_key(&key) {
            return;
        }
        // Cleared rather than evicted one by one, like the fitted lines: what
        // the display is showing is rasterised again within a frame, and
        // deciding which glyph to keep would cost more than drawing it does.
        if self.cache.len() >= REMEMBERED_GLYPHS {
            self.cache.clear();
        }
        let (metrics, coverage) = self.font.rasterize(character, size);
        self.cache.insert(key, Glyph { metrics, coverage });
    }

    /// Sizes are quantised to a tenth of a pixel so the cache cannot be
    /// defeated by floating point noise.
    fn key(character: char, size: f32) -> (char, u32) {
        (character, quantised(size))
    }

    /// How many glyphs are currently cached.
    pub fn cached_glyphs(&self) -> usize {
        self.cache.len()
    }
}

impl std::fmt::Debug for TextRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextRenderer")
            .field("cached_glyphs", &self.cache.len())
            .finish_non_exhaustive()
    }
}

fn blit(canvas: &mut Canvas, glyph: &Glyph, pen: f32, baseline: f32, color: Rgb) {
    if glyph.metrics.width == 0 || glyph.metrics.height == 0 {
        return;
    }

    // Glyph metrics for the sizes PushOS draws at are small integers, well
    // inside the range a `f32` represents exactly and an `i32` holds.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let left = (pen + glyph.metrics.xmin as f32).round() as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let top = (baseline - (glyph.metrics.height as f32 + glyph.metrics.ymin as f32)).round() as i32;

    for row in 0..glyph.metrics.height {
        for column in 0..glyph.metrics.width {
            let coverage = glyph.coverage[row * glyph.metrics.width + column];
            let (Ok(dx), Ok(dy)) = (i32::try_from(column), i32::try_from(row)) else {
                continue;
            };
            // Saturating, so a pen position out at the edge of what an `i32`
            // holds cannot panic; a pixel that far off the canvas is dropped
            // by `blend` either way.
            canvas.blend(
                left.saturating_add(dx),
                top.saturating_add(dy),
                coverage,
                color,
            );
        }
    }
}

/// The compiled-in typeface could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the display font could not be loaded: {reason}")]
pub struct FontUnavailable {
    /// What the font parser reported.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_tab_is_drawn_as_a_blank_rather_than_an_empty_box() {
        // Terminal output keeps tabs on purpose — it is how output lines up —
        // and the typeface has no glyph for one, so asking for it draws the
        // box kept for characters nothing knows.
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        assert!(!text.font.has_glyph('\t'), "the premise of this test");

        let drawn = text.truncate("one\ttwo\u{3000}three", 12.0, 1_000.0);

        assert_eq!(drawn, "one two three");
        for character in drawn.chars() {
            assert!(
                text.font.has_glyph(character),
                "`{character}` would be drawn as an empty rectangle"
            );
        }
    }

    #[test]
    fn nothing_the_typeface_cannot_draw_reaches_the_panel() {
        // PushOS shows text it did not write. A terminal screen is full of box
        // rules, spinners and arrows, and a row of empty rectangles reads as a
        // fault in PushOS rather than as a glyph somebody else chose.
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        let screen =
            "\u{2500}\u{2500}\u{2502} \u{25d0} working \u{2190} back \u{2713} done \u{2588}";

        let drawn = text.truncate(screen, 12.0, 1_000.0);
        for character in drawn.chars() {
            assert!(
                text.font.has_glyph(character),
                "`{character}` would be drawn as an empty rectangle"
            );
        }
    }

    #[test]
    fn a_substituted_line_still_says_what_it_said() {
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        // A rule and a spinner, both of which this typeface lacks and both of
        // which a coding session draws constantly.
        let drawn = text.truncate("\u{2500}\u{2500} \u{25d0} 3 failed", 12.0, 1_000.0);

        assert!(drawn.contains("3 failed"), "the words survive: {drawn}");
        assert!(
            drawn.contains("--"),
            "and the rule becomes something: {drawn}"
        );
        assert!(drawn.contains('o'), "and so does the spinner: {drawn}");
    }

    #[test]
    fn every_character_a_coding_session_draws_is_readable() {
        // Written from what one actually puts on screen, checked against the
        // typeface rather than assumed.
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        let seen = "\u{2500}\u{2502}\u{25d0}\u{2733}\u{23f5}\u{2714}\u{2718}\u{2588}";

        let drawn = text.truncate(seen, 12.0, 1_000.0);
        assert_eq!(
            drawn.chars().count(),
            seen.chars().count(),
            "every one of these has a stand-in: {drawn}"
        );
    }

    #[test]
    fn a_character_with_no_sensible_stand_in_is_dropped_rather_than_guessed() {
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        // A private-use glyph from somebody's patched terminal font.
        let odd = '\u{1f680}';
        assert!(
            !text.can_draw(odd),
            "this test needs a character it cannot draw"
        );

        let drawn = text.truncate(&format!("before {odd} after"), 12.0, 1_000.0);
        assert!(!drawn.contains(odd), "{drawn}");
        assert!(drawn.contains("before"), "{drawn}");
        assert!(drawn.contains("after"), "{drawn}");
    }

    #[test]
    fn what_is_measured_is_what_is_drawn() {
        // Measuring the original and drawing the substitute would put text in
        // the wrong place, and centred text somewhere else entirely.
        let mut text = super::TextRenderer::embedded().expect("the font is compiled in");
        let original = "\u{2500}\u{2500}\u{2500} done";

        let measured = text.width(original, 12.0);
        let drawn = text.truncate(original, 12.0, 1_000.0);
        let redrawn = text.width(&drawn, 12.0);

        assert!(
            (measured - redrawn).abs() < 0.01,
            "measured {measured} but would draw {redrawn}"
        );
    }

    use pushos_domain::ports::DisplayFrame;

    use super::*;

    fn renderer() -> TextRenderer {
        TextRenderer::embedded().expect("the compiled-in font is valid")
    }

    #[test]
    fn the_embedded_font_loads() {
        let mut text = renderer();
        assert!(text.width("PushOS", 18.0) > 0.0);
    }

    #[test]
    fn width_grows_with_the_length_of_the_text() {
        let mut text = renderer();
        assert!(text.width("Development", 18.0) > text.width("Dev", 18.0));
    }

    #[test]
    fn width_grows_with_the_type_size() {
        let mut text = renderer();
        assert!(text.width("PushOS", 28.0) > text.width("PushOS", 13.0));
    }

    #[test]
    fn text_that_fits_is_left_alone() {
        let mut text = renderer();
        assert_eq!(text.truncate("Home", 18.0, 500.0), "Home");
    }

    #[test]
    fn text_that_does_not_fit_is_cut_short_with_an_ellipsis() {
        let mut text = renderer();
        let truncated = text.truncate("Security Review Workflow", 18.0, 60.0);

        assert!(truncated.ends_with('\u{2026}'));
        assert!(truncated.chars().count() < "Security Review Workflow".chars().count());
        assert!(text.width(&truncated, 18.0) <= 60.0);
    }

    #[test]
    fn an_impossible_budget_produces_nothing_rather_than_overflowing() {
        let mut text = renderer();
        assert_eq!(text.truncate("Anything", 18.0, 1.0), "");
        assert_eq!(text.truncate("Anything", 18.0, 0.0), "");
    }

    #[test]
    fn neither_cache_can_grow_without_end() {
        // A name in an alphabet of its own is drawn like any other, and a
        // surface left running for a week must not end up holding every
        // glyph and line it has ever been shown.
        // Drawn at every size a theme could ask for rather than in every
        // alphabet: the typeface substitutes what it cannot draw, so a
        // character it has no glyph for never reaches the cache at all.
        let mut text = renderer();
        let many = REMEMBERED_GLYPHS + 200;
        for step in 0..many {
            #[allow(clippy::cast_precision_loss)]
            let size = 8.0 + step as f32 / 10.0;
            text.width("a", size);
            text.truncate(&format!("line {step}"), size, 100.0);
        }
        assert!(text.cached_glyphs() > 0, "something was actually drawn");

        assert!(text.cached_glyphs() <= REMEMBERED_GLYPHS);
        assert!(text.remembered <= REMEMBERED_LINES);
    }

    #[test]
    fn a_width_past_anything_drawable_is_not_confused_with_the_widest_real_one() {
        assert_eq!(quantised(4_000.0), 40_000);
        assert_eq!(quantised(9_000.0), u32::MAX);
        assert_eq!(quantised(f32::INFINITY), u32::MAX);
        assert_eq!(quantised(f32::NAN), 0, "no width at all");
        assert_eq!(quantised(-5.0), 0);
    }

    #[test]
    fn glyphs_are_rasterised_once_and_reused() {
        let mut text = renderer();
        text.width("aaaa", 18.0);
        let after_first = text.cached_glyphs();

        text.width("aaaaaaaa", 18.0);
        assert_eq!(
            text.cached_glyphs(),
            after_first,
            "the same glyph was rasterised twice"
        );
    }

    #[test]
    fn the_same_size_written_differently_hits_one_cache_entry() {
        let mut text = renderer();
        text.width("a", 18.0);
        text.width("a", 18.000_001);
        assert_eq!(text.cached_glyphs(), 1);
    }

    #[test]
    fn drawing_puts_ink_on_the_canvas() {
        let mut text = renderer();
        let mut canvas = Canvas::new().expect("the display size is valid");
        canvas.clear(Rgb::BLACK);
        text.draw(
            &mut canvas,
            "PushOS",
            (20.0, 40.0),
            TextStyle::left(24.0, Rgb::WHITE, 900.0),
        );

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);
        assert!(
            frame.pixels().iter().any(|&pixel| pixel != 0),
            "nothing was drawn"
        );
    }

    #[test]
    fn alignment_moves_the_text_without_changing_its_width() {
        let mut text = renderer();
        let mut canvas = Canvas::new().expect("the display size is valid");
        canvas.clear(Rgb::BLACK);

        let style = TextStyle::left(18.0, Rgb::WHITE, 900.0);
        let left = text.draw(&mut canvas, "Home", (10.0, 40.0), style);
        let right = text.draw(
            &mut canvas,
            "Home",
            (900.0, 80.0),
            style.aligned(Align::Right),
        );
        assert!((left - right).abs() < 0.001);
    }

    #[test]
    fn drawing_off_the_edge_does_not_panic() {
        let mut text = renderer();
        let mut canvas = Canvas::new().expect("the display size is valid");
        let style = TextStyle::left(24.0, Rgb::WHITE, 900.0);
        text.draw(&mut canvas, "PushOS", (-500.0, -500.0), style);
        text.draw(&mut canvas, "PushOS", (5_000.0, 5_000.0), style);
    }

    #[test]
    fn line_height_exceeds_the_type_size() {
        let text = renderer();
        assert!(text.line_height(18.0) > 18.0);
    }
}
