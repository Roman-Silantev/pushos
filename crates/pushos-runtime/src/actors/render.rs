//! The render task.
//!
//! Waits on the published surface view and draws when it changes. It never
//! reaches for live state, so it cannot be blocked behind anything the rest of
//! the runtime is doing.

use std::sync::Arc;
use std::time::Duration;

use pushos_domain::ports::{DisplayFrame, PushOutput};
use pushos_ui::{LedPlan, PushRenderer, UiSnapshot};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::actors::SurfaceView;
use crate::shutdown::Shutdown;

/// How often an animated screen is redrawn.
///
/// The specification's ceiling. Nothing else in PushOS redraws on a timer at
/// all: a still screen is drawn once and left alone.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Draws the display and drives the lights.
#[derive(Debug)]
pub struct RenderTask {
    output: Arc<dyn PushOutput>,
    renderer: PushRenderer,
    frame: DisplayFrame,
    lit: LedPlan,
    view: watch::Receiver<SurfaceView>,
    /// Where an animated screen has got to.
    ///
    /// Kept here rather than in the snapshot so that time passing never
    /// republishes state, and the input pipeline is not woken thirty times a
    /// second to say that nothing happened.
    animation: u32,
    animating: bool,
}

impl RenderTask {
    /// Builds the task.
    pub fn new(
        output: Arc<dyn PushOutput>,
        renderer: PushRenderer,
        view: watch::Receiver<SurfaceView>,
    ) -> Self {
        Self {
            output,
            renderer,
            frame: DisplayFrame::blank(),
            lit: LedPlan::new(),
            view,
            animation: 0,
            animating: false,
        }
    }

    /// Draws until the pipeline stops or shutdown begins.
    pub async fn run(mut self, shutdown: Shutdown) {
        // The surface comes up blank, so the first pass always draws.
        self.draw().await;

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => break,

                changed = self.view.changed() => {
                    if changed.is_err() {
                        debug!("the input pipeline stopped; ending the render task");
                        break;
                    }
                    self.draw().await;
                }

                () = next_frame(self.animating) => {
                    self.animation = self.animation.wrapping_add(1);
                    self.draw().await;
                }
            }
        }

        if let Err(error) = self.output.clear().await {
            debug!(%error, "could not clear the surface on the way out");
        }
    }

    /// Tells the renderer the panel has gone blank and must be redrawn.
    pub fn invalidate(&mut self) {
        self.renderer.invalidate();
        self.lit = LedPlan::new();
    }

    /// Stamps the current animation frame onto a snapshot.
    ///
    /// Cloning only when something actually animates keeps the still case, which
    /// is almost always the case, free of copying.
    fn with_animation(&self, snapshot: &Arc<UiSnapshot>) -> Arc<UiSnapshot> {
        let Some(splash) = &snapshot.splash else {
            return Arc::clone(snapshot);
        };

        let mut animated = snapshot.as_ref().clone();
        animated.splash = Some(pushos_ui::Splash {
            frame: self.animation,
            ..splash.clone()
        });
        Arc::new(animated)
    }

    async fn draw(&mut self) {
        let view = self.view.borrow_and_update().clone();
        let snapshot = self.with_animation(&view.snapshot);
        self.animating = snapshot.is_animated();

        if self.renderer.render(&snapshot, &mut self.frame)
            && let Err(error) = self.output.present(&self.frame).await
        {
            warn!(%error, "could not present a frame");
            return;
        }

        // Only what changed is sent, so moving between pages that differ by one
        // pad costs one message rather than sixty-four.
        let changes = view.leds.changes_from(&self.lit);
        if changes.is_empty() {
            return;
        }

        match self.output.set_leds(&changes).await {
            Ok(()) => self.lit = view.leds.as_ref().clone(),
            Err(error) => warn!(%error, "could not update the lights"),
        }
    }
}

/// Waits for the next animation frame, or forever when nothing is moving.
async fn next_frame(animating: bool) {
    if animating {
        tokio::time::sleep(FRAME_INTERVAL).await;
    } else {
        std::future::pending::<()>().await;
    }
}
