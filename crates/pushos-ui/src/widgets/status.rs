//! The status bar: where you are, and whether the surface is there.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};
use crate::snapshot::{Tone, UiSnapshot};
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

use super::layout::SLOT_PADDING;

/// Distance from the bottom of the bar to the text baseline.
const BASELINE_INSET: f32 = 7.0;

/// Draws the bar along the top of the display.
pub(crate) fn draw_status(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    area: Area,
    snapshot: &UiSnapshot,
) {
    canvas.fill(area, theme.chrome);

    let baseline = area.bottom() - BASELINE_INSET;
    let caption = TextStyle::left(theme.sizes.caption, theme.text, area.width / 2.0);

    let mut left = area.x + SLOT_PADDING;
    if let Some(workspace) = &snapshot.workspace {
        let style = TextStyle {
            max_width: area.width / 3.0,
            ..caption.colored(theme.accent)
        };
        left += text.draw(canvas, workspace, (left, baseline), style);
        left += text.draw(
            canvas,
            "  /  ",
            (left, baseline),
            TextStyle {
                max_width: 40.0,
                ..caption.colored(theme.muted)
            },
        );
    }

    text.draw(canvas, &snapshot.page.name, (left, baseline), caption);

    let right = area.x + area.width - SLOT_PADDING;
    if !snapshot.connected {
        text.draw(
            canvas,
            "PUSH OFFLINE",
            (right, baseline),
            TextStyle {
                max_width: 200.0,
                ..caption.colored(theme.attention).aligned(Align::Right)
            },
        );
        return;
    }

    if let Some((index, total)) = snapshot.page.position {
        text.draw(
            canvas,
            &format!("{index}/{total}"),
            (right, baseline),
            TextStyle {
                max_width: 80.0,
                ..caption.colored(theme.muted).aligned(Align::Right)
            },
        );
    }
}

/// The colour a tone resolves to.
pub(super) const fn tone_color(theme: &Theme, tone: Tone) -> Rgb {
    match tone {
        Tone::Normal => theme.text,
        Tone::Muted => theme.muted,
        Tone::Active => theme.accent,
        Tone::Attention => theme.attention,
        Tone::Failure => theme.failure,
    }
}
