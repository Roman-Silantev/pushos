//! The eight columns, aligned with the physical controls around them.
//!
//! Laid out the way Ableton lays out Push 2's own screens, because that layout
//! was designed for this panel and this distance: a small dim caption naming
//! what the column is, a larger line for what it currently says, and a bar
//! underneath carrying the state without being read at all. An operator
//! glancing down mid-thought gets the bar; one who looks gets the words.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};
use crate::snapshot::{SessionLine, Slot, Tone, UiSnapshot};
use crate::text::{TextRenderer, TextStyle};
use crate::theme::Theme;

use super::layout::{Layout, SLOT_PADDING};
use super::status::tone_color;

/// Corner radius of a slot panel.
const CORNER: f32 = 3.0;
/// Height of the band along the top of a selected column.
const HEADER_HEIGHT: f32 = 3.0;
/// Height of the bar under a column.
const BAR_HEIGHT: f32 = 4.0;
/// Vertical gap between the caption and the name.
const LINE_GAP: f32 = 3.0;
/// Width of the mark standing in for an unassigned column.
const EMPTY_MARK_WIDTH: f32 = 14.0;
/// How many frames one sweep of a working bar takes.
const SWEEP: u32 = 44;
/// How much of a working bar the moving part covers.
const SWEEP_WIDTH: f32 = 0.34;

/// Draws every column.
///
/// Sessions take the columns when any are running. A surface with agents,
/// terminals and windows working on it is one where what they are doing
/// matters more than what the buttons around the display would do, and there
/// are only eight columns.
pub(crate) fn draw_slots(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    layout: &Layout,
    snapshot: &UiSnapshot,
) {
    draw_dividers(canvas, theme, layout);

    for index in 0..crate::snapshot::SLOT_COUNT {
        let area = layout.slot(index);

        if !snapshot.sessions.is_empty() {
            match snapshot.sessions.get(index) {
                Some(session) => draw_session(canvas, text, theme, area, session, snapshot.frame),
                None => draw_empty(canvas, theme, area),
            }
            continue;
        }

        match snapshot.slots.get(index).and_then(Option::as_ref) {
            Some(slot) => draw_slot(canvas, text, theme, area, slot),
            None => draw_empty(canvas, theme, area),
        }
    }
}

/// Draws the thin rules that make eight columns read as a grid.
///
/// Ableton's own screens do this, and the reason is the hardware: the eight
/// encoders above and eight buttons below have to line up with something, and
/// spacing alone leaves an operator counting across.
fn draw_dividers(canvas: &mut Canvas, theme: &Theme, layout: &Layout) {
    for index in 1..crate::snapshot::SLOT_COUNT {
        let column = layout.slot(index);
        canvas.fill(
            Area::new(
                column.x - super::layout::SLOT_GAP,
                column.y + DIVIDER_INSET,
                1.0,
                column.height - DIVIDER_INSET * 2.0,
            ),
            theme.divider,
        );
    }
}

/// How far a divider stops short of the top and bottom of the band.
const DIVIDER_INSET: f32 = 6.0;

/// Draws a column that has nothing in it.
///
/// A short mark rather than a blank, so the operator can still see the grid is
/// eight wide. It sits where a caption would, not at the foot of the column
/// where it would read as a stray rule.
fn draw_empty(canvas: &mut Canvas, theme: &Theme, area: Area) {
    let inner = area.inset(SLOT_PADDING);
    canvas.fill(
        Area::new(inner.x, inner.y + inner.height / 2.0, EMPTY_MARK_WIDTH, 1.0),
        theme.divider,
    );
}

/// Draws one running session.
fn draw_session(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    area: Area,
    session: &SessionLine,
    frame: u32,
) {
    let colour = tone_color(theme, session.tone);
    let inner = frame_of(canvas, theme, area, session.selected, colour);

    // The caption: what this column is, in the operator's own words where the
    // session gave any, dim and small because it does not change.
    let mut cursor = inner.y + theme.sizes.label;
    if let Some(detail) = &session.detail {
        text.draw(
            canvas,
            &detail.to_uppercase(),
            (inner.x, cursor),
            TextStyle::left(theme.sizes.label, theme.muted, inner.width),
        );
    }

    // The name: the largest thing in the column, because it is what an
    // operator is looking for when they look at all.
    cursor += LINE_GAP + theme.sizes.body;
    text.draw(
        canvas,
        &session.name,
        (inner.x, cursor),
        TextStyle::left(
            theme.sizes.body,
            if session.selected { theme.text } else { colour },
            inner.width,
        ),
    );

    // The state, and the bar that says it again without being read.
    cursor += LINE_GAP + theme.sizes.caption;
    text.draw(
        canvas,
        &session.state,
        (inner.x, cursor),
        TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
    );

    draw_bar(
        canvas,
        theme,
        Area::new(
            inner.x,
            inner.bottom() - BAR_HEIGHT,
            inner.width,
            BAR_HEIGHT,
        ),
        session.tone,
        colour,
        frame,
    );
}

/// Draws the panel a column sits in, returning the room left inside it.
fn frame_of(canvas: &mut Canvas, theme: &Theme, area: Area, selected: bool, colour: Rgb) -> Area {
    if selected {
        canvas.fill_rounded(area, CORNER, theme.chrome);
        // A band along the top as well as a fill, so selection never depends
        // on colour alone.
        canvas.fill(Area::new(area.x, area.y, area.width, HEADER_HEIGHT), colour);
    }
    area.inset(SLOT_PADDING)
}

/// Draws the bar that carries a column's state at a glance.
///
/// Full for something waiting on a person, sweeping for something working,
/// a quiet line for everything else. The movement is the point: a still panel
/// with one thing moving on it tells an operator where to look before they
/// have read a word.
fn draw_bar(canvas: &mut Canvas, theme: &Theme, track: Area, tone: Tone, colour: Rgb, frame: u32) {
    canvas.fill_rounded(track, BAR_HEIGHT / 2.0, theme.divider);

    match tone {
        // Working: a band that sweeps across and starts again.
        Tone::Active => {
            #[allow(clippy::cast_precision_loss)]
            let phase = (frame % SWEEP) as f32 / SWEEP as f32;
            let width = track.width * SWEEP_WIDTH;
            let travel = track.width + width;
            let x = track.x - width + travel * phase;

            // Clipped to the track so the band appears and leaves rather than
            // spilling into the column beside it.
            let left = x.max(track.x);
            let right = (x + width).min(track.x + track.width);
            if right > left {
                canvas.fill_rounded(
                    Area::new(left, track.y, right - left, track.height),
                    BAR_HEIGHT / 2.0,
                    colour,
                );
            }
        }
        // Waiting on a person: the whole bar, so it cannot be missed.
        Tone::Attention | Tone::Failure => {
            canvas.fill_rounded(track, BAR_HEIGHT / 2.0, colour);
        }
        // Everything else: a short mark at the left, present and quiet.
        Tone::Normal | Tone::Muted => {
            canvas.fill_rounded(
                Area::new(track.x, track.y, track.width * 0.22, track.height),
                BAR_HEIGHT / 2.0,
                colour,
            );
        }
    }
}

/// Draws one configured column.
fn draw_slot(canvas: &mut Canvas, text: &mut TextRenderer, theme: &Theme, area: Area, slot: &Slot) {
    let colour = tone_color(theme, slot.tone);
    let inner = frame_of(canvas, theme, area, slot.selected, colour);

    // Centred as a block rather than pinned to the top, so a row of columns
    // reads as one line of text rather than as content that has slipped up.
    let block = theme.sizes.body + LINE_GAP + theme.sizes.caption;
    let mut cursor = inner.y + ((inner.height - block) / 2.0).max(0.0) + theme.sizes.body;

    text.draw(
        canvas,
        &slot.label,
        (inner.x, cursor),
        TextStyle::left(
            theme.sizes.body,
            if slot.selected { theme.text } else { colour },
            inner.width,
        ),
    );

    if let Some(value) = &slot.value {
        cursor += LINE_GAP + theme.sizes.caption;
        text.draw(
            canvas,
            value,
            (inner.x, cursor),
            TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
        );
    }
}
