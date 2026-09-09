//! The renderer.
//!
//! Consumes a snapshot and produces a frame. It performs no input and holds no
//! reference to live state, so it cannot be blocked behind a model, the
//! network, the database or a subprocess. A screen that has not changed is not
//! redrawn, which is what keeps an idle PushOS near zero processor use.

use pushos_domain::ports::DisplayFrame;

use crate::canvas::Canvas;
use crate::mascot::Mascot;
use crate::snapshot::UiSnapshot;
use crate::text::{FontUnavailable, TextRenderer};
use crate::theme::Theme;
use crate::widgets::{
    Layout, draw_focus, draw_footer, draw_overlay, draw_slots, draw_splash, draw_status,
};

/// Draws the Push 2 display.
#[derive(Debug)]
pub struct PushRenderer {
    canvas: Canvas,
    text: TextRenderer,
    theme: Theme,
    layout: Layout,
    mascot: Mascot,
    last: Option<UiSnapshot>,
}

impl PushRenderer {
    /// Builds a renderer with the default theme.
    pub fn new() -> Result<Self, RendererUnavailable> {
        Self::with_theme(Theme::default())
    }

    /// Builds a renderer with a given theme.
    pub fn with_theme(theme: Theme) -> Result<Self, RendererUnavailable> {
        Ok(Self {
            canvas: Canvas::new().ok_or(RendererUnavailable::Canvas)?,
            text: TextRenderer::embedded()?,
            theme,
            layout: Layout::standard(),
            mascot: Mascot::new(),
            last: None,
        })
    }

    /// The theme in use.
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Draws `snapshot` into `frame` if anything has changed.
    ///
    /// Returns whether the frame was rewritten. A caller must still present the
    /// frame either way, because the panel blanks itself if it goes two seconds
    /// without one.
    pub fn render(&mut self, snapshot: &UiSnapshot, frame: &mut DisplayFrame) -> bool {
        if self
            .last
            .as_ref()
            .is_some_and(|last| !snapshot.differs_from(last))
        {
            return false;
        }

        self.draw(snapshot);
        self.canvas.present_into(frame);
        self.last = Some(snapshot.clone());
        true
    }

    /// Forces the next [`Self::render`] to redraw.
    ///
    /// Called when the surface reconnects, since the panel comes back blank and
    /// the renderer's idea of what is on it is no longer true.
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    fn draw(&mut self, snapshot: &UiSnapshot) {
        self.canvas.clear(self.theme.background);

        if let Some(splash) = &snapshot.splash {
            draw_splash(
                &mut self.canvas,
                &mut self.text,
                &self.theme,
                &self.mascot,
                splash,
            );
            return;
        }

        if let Some(overlay) = &snapshot.overlay {
            draw_overlay(&mut self.canvas, &mut self.text, &self.theme, overlay);
            return;
        }

        // After the overlay, because something the operator is being asked
        // right now outranks something they chose to look at a moment ago.
        if let Some(focus) = &snapshot.focus {
            draw_focus(
                &mut self.canvas,
                &mut self.text,
                &self.theme,
                &self.mascot,
                focus,
                snapshot.frame,
            );
            return;
        }

        draw_status(
            &mut self.canvas,
            &mut self.text,
            &self.theme,
            self.layout.status,
            snapshot,
        );
        draw_slots(
            &mut self.canvas,
            &mut self.text,
            &self.theme,
            &self.layout,
            snapshot,
        );
        draw_footer(
            &mut self.canvas,
            &mut self.text,
            &self.theme,
            self.layout.footer,
            snapshot,
        );
    }
}

/// The renderer could not be built.
#[derive(Debug, thiserror::Error)]
pub enum RendererUnavailable {
    /// The drawing surface could not be allocated.
    #[error("could not allocate the display canvas")]
    Canvas,
    /// The compiled-in typeface could not be read.
    #[error(transparent)]
    Font(#[from] FontUnavailable),
}

#[cfg(test)]
mod tests {
    use crate::snapshot::{Notice, Overlay, PageView, Slot, SurfacePresence, Tone};

    use super::*;

    fn renderer() -> PushRenderer {
        PushRenderer::new().expect("the compiled-in font and display size are valid")
    }

    fn populated() -> UiSnapshot {
        UiSnapshot {
            page: PageView::new("development", "Development").at(2, 6),
            workspace: Some("sydclaw".to_owned()),
            surface: SurfacePresence::Hardware,
            slots: [
                Some(
                    Slot::new("Builder")
                        .with_value("working")
                        .with_tone(Tone::Active),
                ),
                Some(Slot::new("Reviewer").with_value("idle")),
                Some(Slot::new("Tests").with_value("passing").selected()),
                None,
                None,
                None,
                None,
                Some(Slot::new("Ship").with_tone(Tone::Attention)),
            ],
            footer: Some("ready".to_owned()),
            notice: None,
            overlay: None,
            focus: None,
            splash: None,
            sessions: Vec::new(),
            listening: pushos_domain::voice::Listening::Idle,
            frame: 0,
        }
    }

    /// Pixels that differ from the page background, which is what "drawn" means
    /// on a themed display where the background is not black.
    fn ink(frame: &DisplayFrame) -> usize {
        let background = Theme::DARK.background.to_bgr565();
        frame
            .pixels()
            .iter()
            .filter(|&&pixel| pixel != background)
            .count()
    }

    #[test]
    fn a_first_render_always_draws() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        assert!(renderer.render(&populated(), &mut frame));
        assert!(
            ink(&frame) > 1_000,
            "the display should have visible content"
        );
    }

    #[test]
    fn an_unchanged_snapshot_is_not_redrawn() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        let snapshot = populated();

        assert!(renderer.render(&snapshot, &mut frame));
        assert!(!renderer.render(&snapshot, &mut frame), "nothing changed");
        assert!(
            !renderer.render(&snapshot.clone(), &mut frame),
            "still nothing changed"
        );
    }

    #[test]
    fn any_change_causes_a_redraw() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        let mut snapshot = populated();
        renderer.render(&snapshot, &mut frame);

        snapshot.notice = Some(Notice::new("deployment failed", Tone::Failure));
        assert!(renderer.render(&snapshot, &mut frame));
    }

    #[test]
    fn a_reconnect_forces_the_next_render_to_redraw() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        let snapshot = populated();

        renderer.render(&snapshot, &mut frame);
        assert!(!renderer.render(&snapshot, &mut frame));

        renderer.invalidate();
        assert!(
            renderer.render(&snapshot, &mut frame),
            "the panel came back blank"
        );
    }

    #[test]
    fn an_overlay_replaces_the_normal_layout() {
        let mut renderer = renderer();
        let mut with_overlay = DisplayFrame::blank();
        let mut without = DisplayFrame::blank();

        let mut snapshot = populated();
        renderer.render(&snapshot, &mut without);

        snapshot.overlay = Some(
            Overlay::new("Apple Music", "Windowlicker")
                .with_detail("Aphex Twin")
                .with_progress(0.4),
        );
        renderer.render(&snapshot, &mut with_overlay);

        assert_ne!(with_overlay.pixels(), without.pixels());
    }

    #[test]
    fn an_overlay_asking_a_question_draws_its_choices() {
        let mut renderer = renderer();
        let mut plain = DisplayFrame::blank();
        let mut asking = DisplayFrame::blank();

        let base = Overlay::new("Deployment", "Preview failed").with_detail("3 of 4 checks");
        let mut snapshot = populated();

        snapshot.overlay = Some(base.clone());
        renderer.render(&snapshot, &mut plain);

        snapshot.overlay = Some(base.asking(["OPEN INCIDENT".to_owned(), "IGNORE".to_owned()]));
        renderer.render(&snapshot, &mut asking);

        // The overlay panel already covers the display, so counting ink would
        // saturate. What matters is that the choices changed a substantial area.
        let changed = asking
            .pixels()
            .iter()
            .zip(plain.pixels())
            .filter(|(after, before)| after != before)
            .count();
        assert!(
            changed > 500,
            "the choices should be visibly drawn, changed {changed} pixels"
        );
    }

    /// Regression: the progress bar was positioned from the top and the choice
    /// row from the bottom, so on a 160-pixel panel an overlay carrying both
    /// drew one over the other.
    #[test]
    fn an_overlay_carrying_everything_does_not_draw_its_progress_over_its_choices() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();

        let mut snapshot = populated();
        snapshot.overlay = Some(
            Overlay::new("Deployment", "Preview build failed")
                .with_detail("3 of 4 checks passed, tests timed out after 12m")
                .with_progress(0.75)
                .asking([
                    "OPEN INCIDENT".to_owned(),
                    "RETRY".to_owned(),
                    "IGNORE".to_owned(),
                ]),
        );
        renderer.render(&snapshot, &mut frame);

        // The choice row occupies the bottom of the panel. The progress bar is
        // the only accent-coloured thing an overlay draws, so finding accent
        // down there means the two collided.
        let accent = Theme::DARK.accent.to_bgr565();
        let mut collisions = 0;
        for y in 115..155 {
            for x in 0..960 {
                if frame.pixel(x, y) == Some(accent) {
                    collisions += 1;
                }
            }
        }

        assert_eq!(collisions, 0, "the progress bar overlapped the choice row");
    }

    #[test]
    fn an_overlay_without_choices_still_shows_its_progress() {
        let mut with_bar = DisplayFrame::blank();
        let mut without = DisplayFrame::blank();
        let mut renderer = renderer();

        let base = Overlay::new("Deployment", "Deploying").with_detail("step 3 of 4");
        let mut snapshot = populated();

        snapshot.overlay = Some(base.clone());
        renderer.render(&snapshot, &mut without);

        renderer.invalidate();
        snapshot.overlay = Some(base.with_progress(0.5));
        renderer.render(&snapshot, &mut with_bar);

        let accent = Theme::DARK.accent.to_bgr565();
        let count = |frame: &DisplayFrame| {
            frame
                .pixels()
                .iter()
                .filter(|pixel| **pixel == accent)
                .count()
        };
        assert!(
            count(&with_bar) > count(&without),
            "the progress bar should be drawn"
        );
    }

    #[test]
    fn a_stand_in_is_never_presented_as_a_push_2() {
        let mut renderer = renderer();
        let mut hardware = DisplayFrame::blank();
        let mut simulated = DisplayFrame::blank();

        renderer.render(&populated(), &mut hardware);

        let mut snapshot = populated();
        snapshot.surface = SurfacePresence::Simulated;
        renderer.render(&snapshot, &mut simulated);

        assert_ne!(
            hardware.pixels(),
            simulated.pixels(),
            "a simulated surface must be marked on the panel"
        );
    }

    #[test]
    fn a_disconnected_surface_says_so() {
        let mut renderer = renderer();
        let mut connected = DisplayFrame::blank();
        let mut offline = DisplayFrame::blank();

        renderer.render(&populated(), &mut connected);
        renderer.render(&UiSnapshot::disconnected(), &mut offline);

        assert_ne!(connected.pixels(), offline.pixels());
        assert!(ink(&offline) > 500, "an offline surface still shows why");
    }

    #[test]
    fn running_agents_take_the_columns() {
        let mut renderer = renderer();
        let mut without = DisplayFrame::blank();
        let mut with_agents = DisplayFrame::blank();

        let snapshot = populated();
        renderer.render(&snapshot, &mut without);

        let mut busy = snapshot;
        busy.sessions = vec![
            crate::snapshot::SessionLine::new("Builder", "working", Tone::Active)
                .with_detail("rewriting the queue"),
            crate::snapshot::SessionLine::new("Reviewer", "waiting", Tone::Attention).selected(),
        ];
        renderer.render(&busy, &mut with_agents);

        assert_ne!(
            without.pixels(),
            with_agents.pixels(),
            "what the agents are doing should be on screen"
        );
    }

    #[test]
    fn an_agent_waiting_for_a_decision_is_not_drawn_like_one_working() {
        let mut renderer = renderer();
        let mut working = DisplayFrame::blank();
        let mut waiting = DisplayFrame::blank();

        let mut snapshot = populated();
        snapshot.sessions = vec![crate::snapshot::SessionLine::new(
            "Builder",
            "working",
            Tone::Active,
        )];
        renderer.render(&snapshot, &mut working);

        renderer.invalidate();
        snapshot.sessions = vec![crate::snapshot::SessionLine::new(
            "Builder",
            "waiting",
            Tone::Attention,
        )];
        renderer.render(&snapshot, &mut waiting);

        assert_ne!(
            working.pixels(),
            waiting.pixels(),
            "an agent that has stopped and is waiting must look different"
        );
    }

    #[test]
    fn a_splash_takes_over_the_whole_display() {
        let mut renderer = renderer();
        let mut normal = DisplayFrame::blank();
        let mut waiting = DisplayFrame::blank();

        renderer.render(&populated(), &mut normal);

        let mut snapshot = populated();
        snapshot.splash = Some(crate::snapshot::Splash::new("PushOS", 0));
        renderer.render(&snapshot, &mut waiting);

        // The status bar is the top of the normal layout; the splash replaces it.
        assert_ne!(normal.pixel(10, 10), waiting.pixel(10, 10));
    }

    #[test]
    fn advancing_the_animation_is_a_reason_to_redraw() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();

        let mut snapshot = UiSnapshot::disconnected();
        assert!(renderer.render(&snapshot, &mut frame));
        assert!(!renderer.render(&snapshot, &mut frame), "nothing moved yet");

        snapshot.splash = snapshot.splash.map(crate::snapshot::Splash::advanced);
        assert!(
            renderer.render(&snapshot, &mut frame),
            "the animation advanced"
        );
    }

    #[test]
    fn the_mascot_is_actually_drawn_in_its_own_colour() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        renderer.render(&UiSnapshot::disconnected(), &mut frame);

        // Every quadrant is a dimmed shade of one hue, so at least one pixel
        // should be more red than blue by a wide margin.
        let warm = frame
            .pixels()
            .iter()
            .filter(|pixel| {
                let red = *pixel & 0x1F;
                let blue = (*pixel >> 11) & 0x1F;
                red > blue + 4
            })
            .count();

        assert!(
            warm > 200,
            "the mascot should be visibly warm, found {warm} pixels"
        );
    }

    #[test]
    fn an_empty_page_renders_without_drawing_over_the_edges() {
        let mut renderer = renderer();
        let mut frame = DisplayFrame::blank();
        renderer.render(&UiSnapshot::default(), &mut frame);
        assert_eq!(frame.pixels().len(), 960 * 160);
    }

    #[test]
    fn very_long_labels_do_not_bleed_between_columns() {
        let mut renderer = renderer();
        let mut narrow = DisplayFrame::blank();
        let mut wide = DisplayFrame::blank();

        let mut snapshot = populated();
        snapshot.slots[0] = Some(Slot::new("Build"));
        renderer.render(&snapshot, &mut narrow);

        renderer.invalidate();
        snapshot.slots[0] = Some(Slot::new("Build".repeat(40)));
        renderer.render(&snapshot, &mut wide);

        // A column that overflowed would light pixels in its neighbour's area.
        let layout = Layout::standard();
        let boundary = layout.slot(1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let x = (boundary.x + boundary.width - 4.0) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let y = (boundary.y + 20.0) as usize;
        assert_eq!(
            wide.pixel(x, y),
            narrow.pixel(x, y),
            "text overflowed its column"
        );
    }
}
