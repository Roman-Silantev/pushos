//! The render task.
//!
//! Waits on the published surface view and draws when it changes. It never
//! reaches for live state, so it cannot be blocked behind anything the rest of
//! the runtime is doing.

use std::sync::Arc;

use pushos_domain::ports::{DisplayFrame, PushOutput};
use pushos_ui::{LedPlan, PushRenderer};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::actors::SurfaceView;
use crate::shutdown::Shutdown;

/// Draws the display and drives the lights.
#[derive(Debug)]
pub struct RenderTask {
    output: Arc<dyn PushOutput>,
    renderer: PushRenderer,
    frame: DisplayFrame,
    lit: LedPlan,
    view: watch::Receiver<SurfaceView>,
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

    async fn draw(&mut self) {
        let view = self.view.borrow_and_update().clone();

        if self.renderer.render(&view.snapshot, &mut self.frame)
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
