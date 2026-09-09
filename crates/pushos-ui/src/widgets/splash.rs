//! The screen PushOS shows when there is nothing else to say.
//!
//! Used while waiting for the surface and for a moment at startup. It is the
//! one animated screen in PushOS, which is why the renderer has a frame counter
//! at all.

use crate::canvas::{Area, Canvas};
use crate::mascot::{MASCOT, Mascot};
use crate::snapshot::Splash;
use crate::text::{TextRenderer, TextStyle};
use crate::theme::Theme;

/// Size of one quadrant of the artwork.
///
/// The burst is square and the panel is 160 pixels tall, so this is what
/// decides whether it fits. Chosen to leave a margin above and below rather
/// than to fill the height, because a mark pressed against both edges reads as
/// a mistake.
const QUADRANT: f32 = 9.0;
/// Gap between quadrants, so the blocks read as blocks.
const GAP: f32 = 2.0;
/// Space between the artwork and the words beside it.
const TITLE_GAP: f32 = 28.0;
/// Space between the two lines of words.
const LINE_GAP: f32 = 7.0;

/// Draws the splash across the whole display.
pub(crate) fn draw_splash(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    mascot: &Mascot,
    splash: &Splash,
) {
    canvas.clear(theme.background);

    // The burst beside the words rather than above them: the panel is six
    // times wider than it is tall, and a design stacked vertically has to
    // shrink everything to fit a height it has plenty of width to spare on.
    let (art_width, art_height) = mascot.measure(QUADRANT, GAP);
    let widest = [Some(&splash.title), splash.detail.as_ref()]
        .into_iter()
        .flatten()
        .map(|line| text.width(line, theme.sizes.headline))
        .fold(0.0_f32, f32::max);

    let block = art_width + TITLE_GAP + widest;
    let left = ((Area::FULL.width - block) / 2.0).max(TITLE_GAP);
    let middle = Area::FULL.height / 2.0;

    mascot.draw(
        canvas,
        (left, middle - art_height / 2.0),
        QUADRANT,
        GAP,
        splash.frame,
        MASCOT,
    );

    let words = left + art_width + TITLE_GAP;
    let lines = if splash.detail.is_some() {
        theme.sizes.headline + LINE_GAP + theme.sizes.caption
    } else {
        theme.sizes.headline
    };
    let mut baseline = middle - lines / 2.0 + theme.sizes.headline;

    text.draw(
        canvas,
        &splash.title,
        (words, baseline),
        TextStyle::left(theme.sizes.headline, theme.text, Area::FULL.width - words),
    );

    if let Some(detail) = &splash.detail {
        baseline += LINE_GAP + theme.sizes.caption;
        text.draw(
            canvas,
            detail,
            (words, baseline),
            TextStyle::left(theme.sizes.caption, theme.muted, Area::FULL.width - words),
        );
    }
}
