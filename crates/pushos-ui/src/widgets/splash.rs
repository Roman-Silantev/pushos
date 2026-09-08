//! The screen PushOS shows when there is nothing else to say.
//!
//! Used while waiting for the surface and for a moment at startup. It is the
//! one animated screen in PushOS, which is why the renderer has a frame counter
//! at all.

use crate::canvas::{Area, Canvas};
use crate::mascot::{MASCOT, Mascot};
use crate::snapshot::Splash;
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

/// Size of one quadrant of the artwork.
///
/// Chosen so the mascot occupies about half the panel's width: large enough to
/// read from across a room, with room beneath it for a line of text.
const QUADRANT: f32 = 13.0;
/// Gap between quadrants, so the blocks read as blocks.
const GAP: f32 = 3.0;
/// Space between the artwork and the title beneath it.
const TITLE_GAP: f32 = 26.0;

/// Draws the splash across the whole display.
pub(crate) fn draw_splash(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    mascot: &Mascot,
    splash: &Splash,
) {
    canvas.clear(theme.background);

    let (art_width, art_height) = mascot.measure(QUADRANT, GAP);
    let lines = 1.0 + if splash.detail.is_some() { 1.0 } else { 0.0 };
    let block = art_height + TITLE_GAP + theme.sizes.body * lines + 6.0 * (lines - 1.0);

    let centre = Area::FULL.width / 2.0;
    let top = ((Area::FULL.height - block) / 2.0).max(4.0);

    mascot.draw(
        canvas,
        (centre - art_width / 2.0, top),
        QUADRANT,
        GAP,
        splash.frame,
        MASCOT,
    );

    let mut baseline = top + art_height + TITLE_GAP;
    text.draw(
        canvas,
        &splash.title,
        (centre, baseline),
        TextStyle::left(theme.sizes.body, theme.text, Area::FULL.width - 40.0)
            .aligned(Align::Centre),
    );

    if let Some(detail) = &splash.detail {
        baseline += theme.sizes.body + 6.0;
        text.draw(
            canvas,
            detail,
            (centre, baseline),
            TextStyle::left(theme.sizes.caption, theme.muted, Area::FULL.width - 40.0)
                .aligned(Align::Centre),
        );
    }
}
