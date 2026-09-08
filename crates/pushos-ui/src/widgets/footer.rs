//! The message line along the bottom of the display.

use crate::canvas::{Area, Canvas};
use crate::snapshot::UiSnapshot;
use crate::text::{TextRenderer, TextStyle};
use crate::theme::Theme;

use super::layout::SLOT_PADDING;
use super::status::tone_color;

/// Width of the marker that flags a notice.
const MARKER_WIDTH: f32 = 3.0;

/// Draws the footer, preferring a transient notice over the standing message.
pub(crate) fn draw_footer(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    area: Area,
    snapshot: &UiSnapshot,
) {
    canvas.fill(area, theme.chrome);

    let (message, color) = match (&snapshot.notice, &snapshot.footer) {
        (Some(notice), _) => (notice.text.as_str(), tone_color(theme, notice.tone)),
        (None, Some(footer)) => (footer.as_str(), theme.muted),
        (None, None) => return,
    };

    // A notice is marked as well as coloured, because colour alone is not a
    // signal an operator can rely on.
    if snapshot.notice.is_some() {
        canvas.fill(Area::new(area.x, area.y, MARKER_WIDTH, area.height), color);
    }

    text.draw(
        canvas,
        message,
        (
            area.x + SLOT_PADDING,
            area.y + area.height / 2.0 + theme.sizes.body / 3.0,
        ),
        TextStyle::left(theme.sizes.body, color, area.width - SLOT_PADDING * 2.0),
    );
}
