//! One thing, across the whole panel, until the operator looks away.
//!
//! An operator who has chosen one of eight things wants everything the panel
//! can tell them about it, so this uses all 960 pixels rather than a column's
//! worth. A header carrying what it is and what it is doing, and under that as
//! many lines of what it last said as will fit.
//!
//! The mark on the left is the mascot, at the size of the header and drawn in
//! PushOS's own orange. It moves while the thing being watched is working and
//! stands still when it is not, so the state is legible from across a desk
//! before a word has been read.

use pushos_domain::color::Rgb;

use crate::canvas::{Area, Canvas};
use crate::mascot::Mascot;
use crate::snapshot::{Focus, Tone};
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
/// Side of one quadrant of the mascot.
///
/// Big enough to be the mascot rather than a smudge. It has a column of the
/// panel to itself, which is what it is for: a glance at this screen should
/// say whose surface it is and whether the thing being watched is moving,
/// before a word has been read.
const QUADRANT: f32 = 4.0;
/// Gap between those quadrants.
const QUADRANT_GAP: f32 = 1.0;
/// Space between the mascot and the words beside it.
const MARK_GAP: f32 = 16.0;
/// Room left above the mascot for it to hop into.
const HOP_ROOM: f32 = 14.0;
/// Width of the bar that repeats the state at the right.
const BAR_WIDTH: f32 = 64.0;
/// Height of that bar.
const BAR_HEIGHT: f32 = 4.0;
/// How many frames one sweep of that bar takes.
const SWEEP: u32 = 44;
/// How much of the bar the moving part covers.
const SWEEP_WIDTH: f32 = 0.34;

/// Draws the focused view across the whole display.
pub(crate) fn draw_focus(
    canvas: &mut Canvas,
    text: &mut TextRenderer,
    theme: &Theme,
    mascot: &Mascot,
    focus: &Focus,
    frame: u32,
) {
    let panel = Area::FULL.inset(MARGIN);
    canvas.fill(Area::FULL, theme.background);
    canvas.fill_rounded(panel, CORNER, theme.chrome);

    let inner = panel.inset(PADDING);
    let colour = tone_color(theme, focus.tone);

    // The mascot takes a column down the left and the words take the rest, so
    // it is a part of the layout rather than something perched on top of it.
    let (mark_width, mark_height) = mascot.measure(QUADRANT, QUADRANT_GAP);
    let phase = if focus.is_animated() { frame } else { 0 };
    mascot.draw(
        canvas,
        // Sitting a little below centre, so the room it hops into is above it
        // rather than off the top of the panel.
        (
            inner.x,
            inner.y + (inner.height - mark_height) / 2.0 + HOP_ROOM / 2.0,
        ),
        QUADRANT,
        QUADRANT_GAP,
        phase,
        theme.accent,
    );

    let words = Area::new(
        inner.x + mark_width + MARK_GAP,
        inner.y,
        inner.width - mark_width - MARK_GAP,
        inner.height,
    );

    let after_header = draw_header(canvas, text, theme, words, focus, colour, frame);
    draw_said(canvas, text, theme, words, after_header, &focus.lines);
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
