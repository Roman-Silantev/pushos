//! The eight labelled columns, aligned with the physical controls around them.

use crate::canvas::{Area, Canvas};
use crate::snapshot::{Slot, UiSnapshot};
use crate::text::{TextRenderer, TextStyle};
use crate::theme::Theme;

use super::layout::{Layout, SLOT_PADDING};
use super::status::tone_color;

/// Corner radius of a slot panel.
const CORNER: f32 = 4.0;
/// Width of the marker that shows a slot is selected.
const MARKER_WIDTH: f32 = 3.0;
/// Vertical gap between a slot's label and its value.
const VALUE_GAP: f32 = 6.0;

/// Draws every column.
pub(crate) fn draw_slots(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    layout: &Layout,
    snapshot: &UiSnapshot,
) {
    for (index, slot) in snapshot.slots.iter().enumerate() {
        let area = layout.slot(index);
        match slot {
            Some(slot) => draw_slot(canvas, text, theme, area, slot),
            // An unassigned column keeps a baseline rule rather than going
            // blank, so the operator can still see the grid is eight wide.
            None => canvas.fill(
                Area::new(area.x, area.bottom() - 1.0, area.width, 1.0),
                theme.divider,
            ),
        }
    }
}

fn draw_slot(canvas: &mut Canvas, text: &mut TextRenderer, theme: &Theme, area: Area, slot: &Slot) {
    if slot.selected {
        canvas.fill_rounded(area, CORNER, theme.divider);
    }

    let inner = area.inset(SLOT_PADDING);
    let label_baseline = inner.y + theme.sizes.body;
    text.draw(
        canvas,
        &slot.label,
        (inner.x, label_baseline),
        TextStyle::left(theme.sizes.body, tone_color(theme, slot.tone), inner.width),
    );

    if let Some(value) = &slot.value {
        text.draw(
            canvas,
            value,
            (inner.x, label_baseline + theme.sizes.caption + VALUE_GAP),
            TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
        );
    }

    if slot.selected {
        // A marker as well as a fill, so selection never depends on colour alone.
        canvas.fill_rounded(
            Area::new(area.x, area.y, MARKER_WIDTH, area.height),
            1.0,
            theme.accent,
        );
    }
}
