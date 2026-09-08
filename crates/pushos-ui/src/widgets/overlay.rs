//! The panel that temporarily takes over the whole display.

use crate::canvas::{Area, Canvas};
use crate::snapshot::Overlay;
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

/// Margin between the panel and the edge of the display.
const PANEL_MARGIN: f32 = 6.0;
/// Padding inside the panel.
const PANEL_PADDING: f32 = 18.0;
/// Corner radius of the overlay panel.
const CORNER: f32 = 8.0;
/// Height of the progress bar.
const BAR_HEIGHT: f32 = 6.0;
/// Space between choice chips.
const CHIP_GAP: f32 = 10.0;
/// Padding inside a choice chip.
const CHIP_PADDING: f32 = 12.0;

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
    text.draw(
        canvas,
        &overlay.kind.to_uppercase(),
        (inner.x, inner.y + theme.sizes.caption),
        TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
    );

    let title_baseline = inner.y + theme.sizes.caption + theme.sizes.headline + 6.0;
    text.draw(
        canvas,
        &overlay.title,
        (inner.x, title_baseline),
        TextStyle::left(theme.sizes.headline, theme.text, inner.width),
    );

    let mut next_baseline = title_baseline + theme.sizes.body + 8.0;
    if let Some(detail) = &overlay.detail {
        text.draw(
            canvas,
            detail,
            (inner.x, next_baseline),
            TextStyle::left(theme.sizes.body, theme.muted, inner.width),
        );
        next_baseline += theme.sizes.body + 6.0;
    }

    if let Some(fraction) = overlay.progress {
        let track = Area::new(inner.x, next_baseline, inner.width, BAR_HEIGHT);
        canvas.fill_rounded(track, BAR_HEIGHT / 2.0, theme.divider);
        canvas.fill_rounded(
            Area::new(track.x, track.y, track.width * fraction, BAR_HEIGHT),
            BAR_HEIGHT / 2.0,
            theme.accent,
        );
    }

    draw_choices(canvas, text, theme, inner, overlay);
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
            (x + width / 2.0, y + height - 10.0),
            TextStyle::left(theme.sizes.body, theme.text, width).aligned(Align::Centre),
        );
        x += width + CHIP_GAP;
    }
}
