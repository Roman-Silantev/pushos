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
/// Width of the mark standing in for an unassigned column.
const EMPTY_MARK_WIDTH: f32 = 18.0;

/// Draws every column.
///
/// Agents take the columns when any are running. A surface with agents on it is
/// one where what they are doing matters more than what the buttons around the
/// display would do, and there are only eight columns.
pub(crate) fn draw_slots(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    layout: &Layout,
    snapshot: &UiSnapshot,
) {
    if !snapshot.agents.is_empty() {
        draw_agents(canvas, text, theme, layout, snapshot);
        return;
    }

    for (index, slot) in snapshot.slots.iter().enumerate() {
        let area = layout.slot(index);
        if let Some(slot) = slot {
            draw_slot(canvas, text, theme, area, slot);
        } else {
            // An unassigned column keeps a short mark rather than going blank,
            // so the operator can still see the grid is eight wide. It sits on
            // the same line as its neighbours' labels rather than at the foot
            // of the column, where it would read as a stray rule.
            let inner = area.inset(SLOT_PADDING);
            canvas.fill(
                Area::new(
                    inner.x,
                    label_baseline(theme, inner) - 5.0,
                    EMPTY_MARK_WIDTH,
                    2.0,
                ),
                theme.divider,
            );
        }
    }
}

/// The baseline of a column's label.
///
/// The label and value are centred as a block in the column rather than pinned
/// to the top, so a row of columns reads as one line of text rather than as
/// content that has slipped upwards.
fn label_baseline(theme: &Theme, inner: Area) -> f32 {
    let block = theme.sizes.body + VALUE_GAP + theme.sizes.caption;
    inner.y + ((inner.height - block) / 2.0).max(0.0) + theme.sizes.body
}

/// Draws the running agents across the columns.
fn draw_agents(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    layout: &Layout,
    snapshot: &UiSnapshot,
) {
    for index in 0..crate::snapshot::SLOT_COUNT {
        let area = layout.slot(index);
        let Some(agent) = snapshot.agents.get(index) else {
            let inner = area.inset(SLOT_PADDING);
            canvas.fill(
                Area::new(
                    inner.x,
                    label_baseline(theme, inner) - 5.0,
                    EMPTY_MARK_WIDTH,
                    2.0,
                ),
                theme.divider,
            );
            continue;
        };

        let mut slot = Slot::new(agent.role.clone()).with_tone(agent.tone);
        // The state is always shown, and the agent's own words only when there
        // is room for both to be read.
        slot = slot.with_value(match &agent.detail {
            Some(detail) => format!("{} \u{2014} {detail}", agent.state),
            None => agent.state.clone(),
        });
        if agent.selected {
            slot = slot.selected();
        }

        draw_slot(canvas, text, theme, area, &slot);
    }
}

fn draw_slot(canvas: &mut Canvas, text: &mut TextRenderer, theme: &Theme, area: Area, slot: &Slot) {
    if slot.selected {
        canvas.fill_rounded(area, CORNER, theme.divider);
    }

    let inner = area.inset(SLOT_PADDING);
    let baseline = label_baseline(theme, inner);
    text.draw(
        canvas,
        &slot.label,
        (inner.x, baseline),
        TextStyle::left(theme.sizes.body, tone_color(theme, slot.tone), inner.width),
    );

    if let Some(value) = &slot.value {
        text.draw(
            canvas,
            value,
            (inner.x, baseline + theme.sizes.caption + VALUE_GAP),
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
