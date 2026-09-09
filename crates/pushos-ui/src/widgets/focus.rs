//! One thing, across the whole panel, until the operator looks away.
//!
//! An operator who has chosen one of eight things wants everything the panel
//! can tell them about it, so this uses all 960 pixels rather than a column's
//! worth. A header carrying what it is and what it is doing, and under that as
//! many lines of what it last said as will fit.
//!
//! One mark down the left, and which mark it is says what is happening. A
//! session that is working or waiting on a person shows the star, breathing.
//! Anything else shows the mascot, hopping. Two marks would be two things to
//! read where the panel has room for one, and the change of shape carries more
//! at a glance than either of them moving would.
//!
//! Neither moves for the sake of moving. The star breathes only for something
//! actually going somewhere, and the mascot hops only when there is nothing
//! that needs watching, which is when a little life is welcome and no
//! information is being displaced.

use pushos_domain::action::Depth;
use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};
use crate::mascot::Mascot;
use crate::snapshot::{Focus, SessionLine, Tone};
use crate::text::{Align, TextRenderer, TextStyle};
use crate::theme::Theme;

use super::status::tone_color;

/// Margin around the panel.
const MARGIN: f32 = 6.0;
/// Padding inside it.
const PADDING: f32 = 9.0;
/// Corner radius.
const CORNER: f32 = 5.0;
/// Height of the rule under the header.
const RULE: f32 = 1.0;
/// Space above and below that rule.
const RULE_GAP: f32 = 6.0;
/// Space between lines of what was said.
const LINE_GAP: f32 = 3.0;
/// The column the mark sits in.
///
/// Fixed rather than measured, because the two marks are different shapes: the
/// mascot is wide and short, having been written for terminal cells, and the
/// star is square. A column that resized with the mark would shift every word
/// on the panel each time a session started working.
const MARK_COLUMN: f32 = 116.0;

/// Side of one quadrant of the star.
///
/// The star is nine quadrants square, so this is what decides how much of the
/// panel it takes. Big enough to be the mark rather than a smudge.
const QUADRANT: f32 = 7.0;
/// Gap between those quadrants.
const QUADRANT_GAP: f32 = 2.0;
/// Space between the marks and the words beside them.
const MARK_GAP: f32 = 16.0;
/// Side of one quadrant of the mascot.
///
/// It is twenty-eight quadrants across, so this is smaller than the star's:
/// the two are different shapes and what they share is the column, not the
/// size of a block.
const MASCOT_QUADRANT: f32 = 3.0;
/// Gap between those quadrants.
const MASCOT_GAP: f32 = 1.0;
/// Width of the bar that repeats the state at the right.
const BAR_WIDTH: f32 = 64.0;
/// Width of the column listing the other sessions.
const OTHERS_WIDTH: f32 = 210.0;
/// Side of the mark beside each of those.
const DOT: f32 = 5.0;
/// Space between that mark and the name beside it.
const DOT_GAP: f32 = 7.0;
/// Space between one of those lines and the next.
const OTHERS_GAP: f32 = 4.0;
/// Height of that bar.
const BAR_HEIGHT: f32 = 4.0;
/// How many frames one sweep of that bar takes.
const SWEEP: u32 = 44;
/// How much of the bar the moving part covers.
const SWEEP_WIDTH: f32 = 0.34;
/// Width of the bar saying where in a long history the panel is.
const DEPTH_WIDTH: f32 = 3.0;
/// Shortest that bar's moving part is allowed to be.
///
/// A window of eight lines in a history of a thousand would otherwise be a
/// thumb too small to see, which is the case where knowing where you are
/// matters most.
const DEPTH_LEAST: f32 = 8.0;

/// Draws the focused view across the whole display.
pub(crate) fn draw_focus(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    star: &Mascot,
    mascot: &Mascot,
    focus: &Focus,
    frame: u32,
) {
    let panel = Area::FULL.inset(MARGIN);
    canvas.fill(Area::FULL, theme.background);
    canvas.fill_rounded(panel, CORNER, theme.chrome);

    let inner = panel.inset(PADDING);
    let colour = tone_color(theme, focus.tone);

    // One mark, and which one says what is happening. The star for something
    // that needs watching, the mascot for everything else.
    let watching = matches!(focus.tone, Tone::Active | Tone::Attention | Tone::Failure);
    let (mark, quadrant, gap) = if watching {
        (star, QUADRANT, QUADRANT_GAP)
    } else {
        (mascot, MASCOT_QUADRANT, MASCOT_GAP)
    };

    let (mark_width, mark_height) = mark.measure(quadrant, gap);
    mark.draw(
        canvas,
        (
            // Centred in a column of its own so the words beside it never move.
            inner.x + (MARK_COLUMN - mark_width) / 2.0,
            inner.y + (inner.height - mark_height) / 2.0,
        ),
        quadrant,
        gap,
        pace(focus.tone, frame),
        theme.accent,
    );

    // The header spans everything to the right of the mark, so the state sits
    // in the far corner and the rule under it runs the whole way across. Only
    // what is below the rule is divided.
    let across = Area::new(
        inner.x + MARK_COLUMN + MARK_GAP,
        inner.y,
        inner.width - MARK_COLUMN - MARK_GAP,
        inner.height,
    );
    let after_header = draw_header(canvas, text, theme, across, focus, colour, frame);

    // Below it, the sessions the operator is not reading take a column under
    // the state they belong with. Being deep in one of eight is no reason to
    // lose sight of the other seven, and one of them waiting on a person is
    // exactly what they would want to be told.
    let keeps_others = !focus.others.is_empty();
    let reserved = if keeps_others { OTHERS_WIDTH } else { 0.0 };

    let words = Area::new(
        across.x,
        after_header,
        across.width - reserved,
        inner.bottom() - after_header,
    );
    draw_said(canvas, text, theme, words, words.y, &focus.lines);

    // Where these lines sit in everything the session has said, drawn in the
    // gap the mark left. A hand on the touch strip is moving through hundreds
    // of lines with nothing else on the panel saying how far it has got.
    if let Some(depth) = focus.depth {
        draw_depth(
            canvas,
            theme,
            Area::new(
                inner.x + MARK_COLUMN + (MARK_GAP - DEPTH_WIDTH) / 2.0,
                words.y,
                DEPTH_WIDTH,
                words.height,
            ),
            depth,
            focus.lines.len(),
        );
    }

    if keeps_others {
        let beside = Area::new(
            inner.x + inner.width - OTHERS_WIDTH + MARK_GAP,
            after_header,
            OTHERS_WIDTH - MARK_GAP,
            inner.bottom() - after_header - theme.sizes.label - OTHERS_GAP,
        );
        draw_others(canvas, text, theme, beside, &focus.others);
    }

    // A view that takes over the panel and stays has to say how to get out of
    // it, because nothing else on screen will.
    if let Some(back) = &focus.back {
        text.draw(
            canvas,
            &format!("{back} to go back"),
            (inner.x + inner.width, inner.bottom()),
            TextStyle::left(theme.sizes.label, theme.muted, OTHERS_WIDTH).aligned(Align::Right),
        );
    }
}

/// Draws where the shown lines sit in a longer history.
///
/// A track for everything that was said and a thumb for the part on the panel,
/// laid out the way the strip beside it is: the newest is at the bottom, and
/// sliding a finger up travels back through what happened, which is the
/// direction the words themselves already run.
fn draw_depth(canvas: &mut Canvas, theme: &Theme, track: Area, depth: Depth, shown: usize) {
    if track.height <= 0.0 || depth.total == 0 {
        return;
    }
    canvas.fill_rounded(track, DEPTH_WIDTH / 2.0, theme.divider);

    let total = as_length(depth.total);
    let window = as_length(shown.min(depth.total));
    let start = as_length(depth.total.saturating_sub(depth.back + shown));

    let height = (track.height * window / total).max(DEPTH_LEAST);
    // Clamped so a thumb widened to the minimum cannot run off the end.
    let top = (track.height * start / total).min(track.height - height);

    canvas.fill_rounded(
        Area::new(track.x, track.y + top, track.width, height),
        DEPTH_WIDTH / 2.0,
        theme.accent,
    );
}

/// A line count as a length to divide by.
#[allow(clippy::cast_precision_loss)]
fn as_length(lines: usize) -> f32 {
    lines as f32
}

/// How fast the mark moves, which says which kind of state this is.
///
/// Quick when a session wants a person, steady when one is working, and gently
/// otherwise, where the mascot hops and nothing is being displaced by it. A
/// mark that moved at one pace whatever was happening would be decoration.
const fn pace(tone: Tone, frame: u32) -> u32 {
    match tone {
        // Twice the pace, because this is the one worth walking over for.
        Tone::Attention | Tone::Failure => frame * 2,
        Tone::Active => frame,
        // Half, so the mascot is alive without asking to be looked at.
        Tone::Normal => frame / 2,
        // Nothing recognisable is running, so nothing moves.
        Tone::Muted => 0,
    }
}

/// Draws the sessions the operator is not reading, down the right.
///
/// A mark and a name each, in the colour of what that session is doing. No
/// state in words: this is the corner of the eye, and anything that had to be
/// read would be competing with the thing that was actually asked for.
fn draw_others(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    beside: Area,
    others: &[SessionLine],
) {
    let step = theme.sizes.caption + OTHERS_GAP;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fits = (beside.height / step).floor().max(0.0) as usize;
    if fits == 0 {
        return;
    }

    // Whatever wants a person first, because that is the whole reason for
    // showing them at all.
    let mut order: Vec<&SessionLine> = others.iter().collect();
    order.sort_by_key(|line| match line.tone {
        Tone::Attention => 0,
        Tone::Failure => 1,
        Tone::Active => 2,
        Tone::Normal => 3,
        Tone::Muted => 4,
    });

    let mut cursor = beside.y + theme.sizes.caption;
    for line in order.into_iter().take(fits) {
        let colour = tone_color(theme, line.tone);
        canvas.fill_rounded(
            Area::new(beside.x, cursor - DOT, DOT, DOT),
            DOT / 2.0,
            colour,
        );
        text.draw(
            canvas,
            &line.name,
            (beside.x + DOT + DOT_GAP, cursor),
            TextStyle::left(
                theme.sizes.caption,
                if line.tone == Tone::Attention {
                    colour
                } else {
                    theme.muted
                },
                beside.width - DOT - DOT_GAP,
            ),
        );
        cursor += step;
    }
}

/// Draws the header, returning the top of the room left under it.
fn draw_header(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    inner: Area,
    focus: &Focus,
    colour: Rgb,
    frame: u32,
) -> f32 {
    let left = inner.x;
    let right = inner.x + inner.width;

    // What it is, small and dim, because it does not change.
    let mut cursor = inner.y + theme.sizes.label;
    if !focus.kind.is_empty() {
        text.draw(
            canvas,
            &focus.kind.to_uppercase(),
            (left, cursor),
            TextStyle::left(theme.sizes.label, theme.muted, inner.width / 2.0),
        );
    }

    // What it is called, the largest thing on the panel.
    cursor += LINE_GAP + theme.sizes.headline;
    text.draw(
        canvas,
        &focus.title,
        (left, cursor),
        TextStyle::left(
            theme.sizes.headline,
            theme.text,
            right - left - BAR_WIDTH - MARK_GAP,
        ),
    );

    // What it is doing, against the right edge, with the bar under it saying
    // the same thing without being read.
    if !focus.state.is_empty() {
        text.draw(
            canvas,
            &focus.state.to_uppercase(),
            (right, inner.y + theme.sizes.label),
            TextStyle::left(theme.sizes.label, colour, BAR_WIDTH * 2.0).aligned(Align::Right),
        );
        draw_bar(
            canvas,
            theme,
            Area::new(
                right - BAR_WIDTH,
                inner.y + theme.sizes.label + LINE_GAP + 2.0,
                BAR_WIDTH,
                BAR_HEIGHT,
            ),
            focus.tone,
            colour,
            frame,
        );
    }

    let under = cursor + RULE_GAP;
    canvas.fill(Area::new(inner.x, under, inner.width, RULE), theme.divider);
    under + RULE + RULE_GAP
}

/// Draws what the thing last said, newest at the bottom.
///
/// Only as many lines as fit, and the newest are the ones kept: a panel that
/// showed the oldest and ran out of room would be showing the wrong end of the
/// conversation.
fn draw_said(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    inner: Area,
    top: f32,
    lines: &[String],
) {
    let step = theme.sizes.caption + LINE_GAP;
    let room = (inner.bottom() - top).max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fits = (room / step).floor().max(0.0) as usize;
    if fits == 0 {
        return;
    }

    let shown = &lines[lines.len().saturating_sub(fits)..];
    let mut cursor = top + theme.sizes.caption;

    for (index, line) in shown.iter().enumerate() {
        // The newest line in full colour and the ones above it dimmer, so the
        // eye lands on what just happened.
        let colour = if index + 1 == shown.len() {
            theme.text
        } else {
            theme.muted
        };
        text.draw(
            canvas,
            line,
            (inner.x, cursor),
            TextStyle::left(theme.sizes.caption, colour, inner.width),
        );
        cursor += step;
    }
}

/// Draws the bar that repeats the state without being read.
fn draw_bar(canvas: &mut Canvas, theme: &Theme, track: Area, tone: Tone, colour: Rgb, frame: u32) {
    canvas.fill_rounded(track, BAR_HEIGHT / 2.0, theme.divider);

    match tone {
        Tone::Active => {
            #[allow(clippy::cast_precision_loss)]
            let phase = (frame % SWEEP) as f32 / SWEEP as f32;
            let width = track.width * SWEEP_WIDTH;
            let x = track.x - width + (track.width + width) * phase;
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
        Tone::Attention | Tone::Failure => {
            canvas.fill_rounded(track, BAR_HEIGHT / 2.0, colour);
        }
        Tone::Normal | Tone::Muted => {
            canvas.fill_rounded(
                Area::new(track.x, track.y, track.width * 0.22, track.height),
                BAR_HEIGHT / 2.0,
                colour,
            );
        }
    }
}
