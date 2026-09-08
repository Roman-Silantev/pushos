//! The renderer.
//!
//! Consumes a snapshot and produces a frame. It performs no input and holds no
//! reference to live state, so it cannot be blocked behind a model, the
//! network, the database or a subprocess. A screen that has not changed is not
//! redrawn, which is what keeps an idle PushOS near zero processor use.

use pushos_domain::ports::DisplayFrame;

use crate::canvas::Canvas;
use crate::snapshot::UiSnapshot;
use crate::text::{FontUnavailable, TextRenderer};
use crate::theme::Theme;
use crate::widgets::{Layout, draw_footer, draw_overlay, draw_slots, draw_status};

/// Draws the Push 2 display.
#[derive(Debug)]
pub struct PushRenderer {
    canvas: Canvas,
    text: TextRenderer,
    theme: Theme,
    layout: Layout,
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

        if let Some(overlay) = &snapshot.overlay {
            draw_overlay(&mut self.canvas, &mut self.text, &self.theme, overlay);
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
    use crate::snapshot::{Notice, Overlay, PageView, Slot, Tone};

    use super::*;

    fn renderer() -> PushRenderer {
        PushRenderer::new().expect("the compiled-in font and display size are valid")
    }

    fn populated() -> UiSnapshot {
        UiSnapshot {
            page: PageView::new("development", "Development").at(2, 6),
            workspace: Some("sydclaw".to_owned()),
            connected: true,
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
