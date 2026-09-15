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

/// How often an animated screen is redrawn once it has dimmed.
///
/// Ten frames a second. Nobody has touched the surface for a while, so the
/// motion only has to say that something is working, not look smooth, and each
/// frame is a render and a USB transfer the Mac does not need to do.
const DIMMED_FRAME_INTERVAL: Duration = Duration::from_millis(100);

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
    /// The brightness last sent to the hardware.
    ///
    /// `None` until the first draw, because a Push 2 comes up at whatever its
    /// firmware chose, which is not what PushOS chose.
    brightness: Option<pushos_domain::rest::Levels>,
    /// Whether the screen is showing the dark frame of a sleeping surface.
    dark: bool,
    /// How long until the next animation frame, which depends on whether the
    /// surface has dimmed.
    frame_interval: Duration,
    /// Which failures to reach the hardware have been reported.
    ///
    /// A link that fails fails on every frame, thirty times a second. Said
    /// once, and again only after it has worked in between, so a surface left
    /// broken overnight writes one line rather than a million.
    reported: Reported,
}

/// Which hardware failures are already in the log.
#[derive(Debug, Default)]
struct Reported {
    frame: bool,
    lights: bool,
    brightness: bool,
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
            brightness: None,
            dark: false,
            frame_interval: FRAME_INTERVAL,
            reported: Reported::default(),
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

                () = next_frame(self.animating, self.frame_interval) => {
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
        if !snapshot.is_animated() {
            return Arc::clone(snapshot);
        }

        let mut animated = snapshot.as_ref().clone();
        animated.frame = self.animation;
        if let Some(splash) = &snapshot.splash {
            animated.splash = Some(pushos_ui::Splash {
                frame: self.animation,
                ..splash.clone()
            });
        }
        Arc::new(animated)
    }

    async fn draw(&mut self) {
        let view = self.view.borrow_and_update().clone();
        self.apply_brightness(view.levels).await;
        self.frame_interval = if view.rest == pushos_domain::rest::Rest::Dimmed {
            DIMMED_FRAME_INTERVAL
        } else {
            FRAME_INTERVAL
        };

        if view.rest.shows_screen() {
            if !self.draw_screen(&view).await {
                return;
            }
        } else if !self.darken_screen().await {
            return;
        }

        // A sleeping surface keeps only the lights asking for a person, and one
        // whose computer is asleep keeps none. Built here rather than published,
        // so waking is one publish with nothing to rebuild.
        let wanted = if view.rest.shows_screen() {
            view.leds.as_ref().clone()
        } else if view.rest.keeps_asking_lights() {
            view.leds.only_wanting_a_person()
        } else {
            pushos_ui::LedPlan::new()
        };

        // Only what changed is sent, so moving between pages that differ by one
        // pad costs one message rather than sixty-four.
        let changes = wanted.changes_from(&self.lit);
        if changes.is_empty() {
            return;
        }

        match self.output.set_leds(&changes).await {
            Ok(()) => {
                self.lit = wanted;
                self.reported.lights = false;
            }
            Err(error) => {
                if !std::mem::replace(&mut self.reported.lights, true) {
                    warn!(%error, "could not update the lights");
                }
            }
        }
    }

    /// Sends a brightness the hardware does not already have.
    ///
    /// Kept as unsent when it fails, so the next draw says it again rather than
    /// leaving the surface at whatever it was, possibly full, all night.
    async fn apply_brightness(&mut self, levels: pushos_domain::rest::Levels) {
        if self.brightness == Some(levels) {
            return;
        }
        match self.output.set_brightness(levels).await {
            Ok(()) => {
                self.brightness = Some(levels);
                self.reported.brightness = false;
            }
            Err(error) => {
                if !std::mem::replace(&mut self.reported.brightness, true) {
                    warn!(%error, "could not set the brightness");
                }
            }
        }
    }

    /// Draws the current view. Returns whether the lights should be updated.
    async fn draw_screen(&mut self, view: &SurfaceView) -> bool {
        let snapshot = self.with_animation(&view.snapshot);
        self.animating = snapshot.is_animated();
        self.dark = false;

        if self.renderer.render(&snapshot, &mut self.frame) {
            match self.output.present(&self.frame).await {
                Ok(()) => self.reported.frame = false,
                Err(error) => {
                    if !std::mem::replace(&mut self.reported.frame, true) {
                        warn!(%error, "could not present a frame; saying so again only once it recovers");
                    }
                    return false;
                }
            }
        }
        true
    }

    /// Shows a sleeping surface's screen: black, and not redrawn.
    ///
    /// Black rather than the last picture with the backlight off. An LCD that
    /// holds one image for hours is what leaves a trace of it, and a dark panel
    /// holding a uniform frame holds no image at all. Nothing animates while
    /// asleep, so the render loop stops ticking too.
    async fn darken_screen(&mut self) -> bool {
        self.animating = false;
        if self.dark {
            return true;
        }

        self.frame.fill(pushos_domain::color::Rgb::BLACK);
        if let Err(error) = self.output.present(&self.frame).await {
            warn!(%error, "could not darken the screen");
            return false;
        }
        self.dark = true;
        // Whatever was drawn before is gone, so waking must draw it all again.
        self.renderer.invalidate();
        true
    }
}

/// Waits for the next animation frame, or forever when nothing is moving.
async fn next_frame(animating: bool, interval: Duration) {
    if animating {
        tokio::time::sleep(interval).await;
    } else {
        std::future::pending::<()>().await;
    }
}
