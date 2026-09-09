//! The status bar: where you are, and whether the surface is there.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};
use crate::snapshot::{Tone, UiSnapshot};
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

use super::layout::SLOT_PADDING;

/// Distance from the bottom of the bar to the text baseline.
const BASELINE_INSET: f32 = 7.0;

/// The side of the square that marks the panel as PushOS's.
const MARK: f32 = 6.0;

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

    // A mark in PushOS's own orange, so the panel says whose it is without
    // spending a word on it.
    let mark = Area::new(left, area.y + (area.height - MARK) / 2.0, MARK, MARK);
    canvas.fill_rounded(mark, 1.0, theme.accent);
    left += MARK + SLOT_PADDING;

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

    // Ahead of everything else on this side. Whether the microphone is on
    // outranks which page you are on and what the surface is.
    if let Some(badge) = snapshot.listening.badge() {
        draw_listening(canvas, text, theme, area, right, badge);
        return;
    }

    if let Some(label) = snapshot.surface.caption() {
        text.draw(
            canvas,
            label,
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

/// How far the dot sits from the word beside it.
const DOT_GAP: f32 = 5.0;

/// How big the dot is.
const DOT_SIZE: f32 = 6.0;

/// Draws the one thing on this display an operator must never miss.
///
/// A word alone would read as another label. The block beside it is what makes
/// it register from across a desk.
fn draw_listening(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    area: Area,
    right: f32,
    badge: &str,
) {
    let baseline = area.bottom() - BASELINE_INSET;
    let style = TextStyle {
        max_width: area.width / 2.0,
        ..TextStyle::left(theme.sizes.caption, theme.failure, area.width / 2.0)
            .aligned(Align::Right)
    };
    let width = text.draw(canvas, badge, (right, baseline), style);

    let dot = Area::new(
        right - width - DOT_GAP - DOT_SIZE,
        area.y + (area.height - DOT_SIZE) / 2.0,
        DOT_SIZE,
        DOT_SIZE,
    );
    canvas.fill_rounded(dot, DOT_SIZE / 2.0, theme.failure);
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
