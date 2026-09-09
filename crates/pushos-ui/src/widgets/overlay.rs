//! The panel that temporarily takes over the whole display.
//!
//! There are only 160 pixels of height, so the layout is computed from what the
//! overlay actually carries rather than stacked blindly. A choice row is
//! reserved first, because a question the operator cannot read the answers to
//! is worse than one missing its progress bar. Everything else is drawn while
//! there is room and dropped when there is not, in the order it matters.

use crate::canvas::{Area, Canvas};
use crate::snapshot::Overlay;
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

/// Margin between the panel and the edge of the display.
const PANEL_MARGIN: f32 = 5.0;
/// Padding inside the panel.
const PANEL_PADDING: f32 = 14.0;
/// Corner radius of the overlay panel.
const CORNER: f32 = 8.0;
/// Height of the progress bar.
const BAR_HEIGHT: f32 = 6.0;
/// Space between choice chips.
const CHIP_GAP: f32 = 10.0;
/// Padding inside a choice chip.
const CHIP_PADDING: f32 = 12.0;
/// Space between stacked lines.
const LINE_GAP: f32 = 6.0;

/// Draws an overlay across the whole display.
pub(crate) fn draw_overlay(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    overlay: &Overlay,
) {
    let panel = Area::FULL.inset(PANEL_MARGIN);
    canvas.fill(Area::FULL, theme.background);
    canvas.fill_rounded(panel, CORNER, theme.chrome);

    let inner = panel.inset(PANEL_PADDING);
    let chips = chip_height(theme, overlay);
    // Everything above the choice row has to fit in what is left.
    let available = inner.bottom() - chips;

    let mut cursor = inner.y;

    if !overlay.kind.is_empty() {
        cursor += theme.sizes.caption;
        text.draw(
            canvas,
            &overlay.kind.to_uppercase(),
            (inner.x, cursor),
            TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
        );
        cursor += LINE_GAP;
    }

    cursor += theme.sizes.headline;
    text.draw(
        canvas,
        &overlay.title,
        (inner.x, cursor),
        TextStyle::left(theme.sizes.headline, theme.text, inner.width),
    );

    if let Some(detail) = &overlay.detail
        && cursor + LINE_GAP + theme.sizes.body <= available
    {
        cursor += LINE_GAP + theme.sizes.body;
        text.draw(
            canvas,
            detail,
            (inner.x, cursor),
            TextStyle::left(theme.sizes.body, theme.muted, inner.width),
        );
    }

    // Drawn before the progress bar, because a list is what the operator asked
    // for and a bar is decoration next to it.
    for line in &overlay.lines {
        if cursor + LINE_GAP + theme.sizes.body > available {
            break;
        }
        cursor += LINE_GAP + theme.sizes.body;
        text.draw(
            canvas,
            line,
            (inner.x, cursor),
            TextStyle::left(theme.sizes.body, theme.text, inner.width),
        );
    }

    if let Some(fraction) = overlay.progress
        && cursor + LINE_GAP + BAR_HEIGHT <= available
    {
        cursor += LINE_GAP;
        let track = Area::new(inner.x, cursor, inner.width, BAR_HEIGHT);
        canvas.fill_rounded(track, BAR_HEIGHT / 2.0, theme.divider);
        canvas.fill_rounded(
            Area::new(track.x, track.y, track.width * fraction, BAR_HEIGHT),
            BAR_HEIGHT / 2.0,
            theme.accent,
        );
    }

    draw_choices(canvas, text, theme, inner, overlay);
}

/// How much vertical room the choice row needs, including its gap.
fn chip_height(theme: &Theme, overlay: &Overlay) -> f32 {
    if overlay.choices.is_empty() {
        0.0
    } else {
        theme.sizes.body + CHIP_PADDING + LINE_GAP
    }
}

fn draw_choices(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    inner: Area,
    overlay: &Overlay,
) {
    if overlay.choices.is_empty() {
        return;
    }

    let height = theme.sizes.body + CHIP_PADDING;
    let y = inner.bottom() - height;
    let mut x = inner.x;

    for choice in &overlay.choices {
        let width = text.width(choice, theme.sizes.body) + CHIP_PADDING * 2.0;
        if x + width > inner.x + inner.width {
            break;
        }
        canvas.fill_rounded(Area::new(x, y, width, height), height / 2.0, theme.divider);
        text.draw(
            canvas,
            choice,
            (x + width / 2.0, y + height - CHIP_PADDING),
            TextStyle::left(theme.sizes.body, theme.text, width).aligned(Align::Centre),
        );
        x += width + CHIP_GAP;
    }
}
