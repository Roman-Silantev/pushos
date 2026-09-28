//! The panel for finding what is not on a pad.
//!
//! A column of rows with one of them under the cursor, and a trail across the
//! top saying where in the tree it is. There are only 160 pixels of height, so
//! the rows shown are a window onto the level rather than all of it: the
//! cursor is kept inside that window and the level scrolls under it, which is
//! what stops a long list from either overflowing or shrinking its own text
//! until it cannot be read across a desk.

use pushos_domain::browse::Twig;

use crate::canvas::{Area, Canvas};
use crate::snapshot::Browser;
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

/// Margin between the panel and the edge of the display.
const PANEL_MARGIN: f32 = 5.0;
/// Padding inside the panel.
const PANEL_PADDING: f32 = 10.0;
/// Corner radius of the panel.
const CORNER: f32 = 8.0;
/// Height of one row.
const ROW_HEIGHT: f32 = 22.0;
/// Space between the trail and the rows.
const TRAIL_GAP: f32 = 6.0;
/// How far the marker on a row that leads further in sits from the right edge.
const CHEVRON_INSET: f32 = 4.0;

/// The band along the foot kept for the line saying how to work the panel.
///
/// Rows stop before it rather than being trusted to fit, so a taller row or a
/// larger type scale loses a row instead of drawing over the one line that
/// tells an operator how to get out.
const KEYS_HEIGHT: f32 = 14.0;

/// How many rows fit between the trail and the line of keys.
///
/// Worked out once rather than from the height each frame: the display does
/// not change size, and a constant is easier to reason about than arithmetic
/// that could round to zero. One fewer than before, because the foot now says
/// how to work the panel, and rows drawn over that line would be worse than a
/// row fewer.
pub(crate) const ROWS_SHOWN: usize = 4;

/// Draws the browser across the whole display.
pub(crate) fn draw_browser(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    browser: &Browser,
) {
    let panel = Area::FULL.inset(PANEL_MARGIN);
    canvas.fill(Area::FULL, theme.background);
    canvas.fill_rounded(panel, CORNER, theme.chrome);

    let inner = panel.inset(PANEL_PADDING);
    let mut cursor = inner.y + theme.sizes.caption;

    text.draw(
        canvas,
        &browser.trail.to_uppercase(),
        (inner.x, cursor),
        TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
    );
    cursor += TRAIL_GAP;

    draw_keys(canvas, text, theme, inner, browser);

    if browser.rows.is_empty() {
        cursor += theme.sizes.body;
        text.draw(
            canvas,
            &browser.empty,
            (inner.x, cursor),
            TextStyle::left(theme.sizes.body, theme.muted, inner.width),
        );
        return;
    }

    let from = window_start(browser.at, browser.rows.len());
    let rows_bottom = inner.bottom() - KEYS_HEIGHT;
    for (offset, twig) in browser
        .rows
        .iter()
        .enumerate()
        .skip(from)
        .take(ROWS_SHOWN)
        .enumerate()
        .map(|(offset, (_, twig))| (offset, twig))
    {
        let top = cursor + ROW_HEIGHT * as_f32(offset);
        // Never into the band the keys line sits in.
        if top + ROW_HEIGHT > rows_bottom {
            break;
        }
        let row = Area {
            x: inner.x,
            y: top,
            width: inner.width,
            height: ROW_HEIGHT,
        };
        draw_row(canvas, text, theme, row, twig, from + offset == browser.at);
    }
}

/// Says how to work the panel, along the foot.
///
/// Named after the controls the operator actually has bound, so rebinding the
/// browser changes what the panel says. Nothing is drawn when nothing is bound:
/// a hint pointing at a control that does not exist is worse than none.
fn draw_keys(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    inner: Area,
    browser: &Browser,
) {
    let said = match (&browser.enter, &browser.leave) {
        (Some(enter), Some(leave)) => format!("{enter} · in     {leave} · back"),
        (Some(enter), None) => format!("{enter} · in"),
        (None, Some(leave)) => format!("{leave} · back"),
        (None, None) => return,
    };

    text.draw(
        canvas,
        &said,
        (inner.x, inner.bottom()),
        TextStyle::left(theme.sizes.caption, theme.muted, inner.width),
    );
}

/// Which row the window onto a long level starts at.
///
/// Keeps the cursor inside the window, scrolling the level under it rather
/// than moving the cursor to the edge and leaving it there.
pub(crate) fn window_start(at: usize, rows: usize) -> usize {
    if rows <= ROWS_SHOWN {
        return 0;
    }
    // Centre the cursor where there is room either side, and stop at the ends.
    at.saturating_sub(ROWS_SHOWN / 2).min(rows - ROWS_SHOWN)
}

fn draw_row(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    row: Area,
    twig: &Twig,
    under_cursor: bool,
) {
    let (label, detail) = if under_cursor {
        canvas.fill_rounded(row, CORNER / 2.0, theme.accent);
        (theme.background, theme.background)
    } else {
        (theme.text, theme.muted)
    };

    let baseline = row.y + ROW_HEIGHT - theme.sizes.label / 2.0;
    text.draw(
        canvas,
        &twig.label,
        (row.x + PANEL_PADDING / 2.0, baseline),
        TextStyle::left(theme.sizes.body, label, row.width * 0.6),
    );

    if let Some(said) = &twig.detail {
        text.draw(
            canvas,
            said,
            (row.x + row.width - CHEVRON_INSET, baseline),
            TextStyle {
                align: Align::Right,
                ..TextStyle::left(theme.sizes.caption, detail, row.width * 0.35)
            },
        );
    } else if twig.has_children {
        // A row that leads further in says so, the way a folder does.
        text.draw(
            canvas,
            "›",
            (row.x + row.width - CHEVRON_INSET, baseline),
            TextStyle {
                align: Align::Right,
                ..TextStyle::left(theme.sizes.body, detail, row.width * 0.1)
            },
        );
    }
}

/// A row count as a float, for laying rows out.
fn as_f32(rows: usize) -> f32 {
    f32::from(u16::try_from(rows).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_that_fits_is_shown_from_its_top() {
        for rows in 0..=ROWS_SHOWN {
            assert_eq!(window_start(0, rows), 0);
            assert_eq!(window_start(rows.saturating_sub(1), rows), 0);
        }
    }

    #[test]
    fn a_long_level_scrolls_under_the_cursor_rather_than_pinning_it() {
        let rows = 20;
        // Near the top the window stays at the top.
        assert_eq!(window_start(0, rows), 0);
        assert_eq!(window_start(1, rows), 0);

        // In the middle the cursor sits inside the window.
        let from = window_start(10, rows);
        assert!(from <= 10 && 10 < from + ROWS_SHOWN, "cursor is off screen");

        // At the end the window stops rather than running past it.
        assert_eq!(window_start(19, rows), rows - ROWS_SHOWN);
    }

    #[test]
    fn the_cursor_is_always_somewhere_on_the_display() {
        // The one property that matters: whatever the level's length and
        // wherever the cursor is, it is drawn.
        for rows in 1..40 {
            for at in 0..rows {
                let from = window_start(at, rows);
                assert!(
                    at >= from && at < from + ROWS_SHOWN,
                    "{at} of {rows} fell outside the window starting at {from}"
                );
                assert!(from + ROWS_SHOWN <= rows.max(ROWS_SHOWN));
            }
        }
    }

    #[test]
    fn the_rows_leave_room_for_the_line_that_says_how_to_work_them() {
        // The panel is 160 pixels tall and the keys sit on the last line of it.
        // Rows drawn over that line would be worse than a row fewer.
        let inner = Area::FULL.inset(PANEL_MARGIN).inset(PANEL_PADDING);
        let trail = inner.y + theme_caption() + TRAIL_GAP;
        let rows_end = trail + ROW_HEIGHT * as_f32(ROWS_SHOWN);

        assert!(
            rows_end <= inner.bottom() - KEYS_HEIGHT + ROW_HEIGHT,
            "four rows at {rows_end} run into the keys line at {}",
            inner.bottom() - KEYS_HEIGHT
        );
    }

    /// The caption size the default theme uses, for the layout arithmetic.
    fn theme_caption() -> f32 {
        crate::theme::Theme::DARK.sizes.caption
    }
}
