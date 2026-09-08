//! The input pipeline.
//!
//! One task carries a physical event the whole way:
//!
//! ```text
//! ControlEvent -> Gesture -> Binding -> Action
//! ```
//!
//! It owns the recogniser and the surface state outright, so there is no lock
//! on the input path. Everything downstream reads the immutable snapshot it
//! publishes.

use std::sync::Arc;
use std::time::Instant;

use pushos_actions::ActionDispatcher;
use pushos_bindings::{BindingResolver, GestureEvent, GestureRecognizer};
use pushos_config::{ConfigStore, RuntimeConfig};
use pushos_domain::action::{ActionResult, ActionStatus};
use pushos_domain::binding::BindingKey;
use pushos_domain::event::{DomainEvent, EventEnvelope, EventSource};
use pushos_domain::ids::CorrelationId;
use pushos_domain::input::ControlEvent;
use pushos_domain::ports::PushInput;
use pushos_ui::{SurfacePresence, Tone, UiSnapshot};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::actors::SurfaceState;
use crate::actors::leds;
use crate::bus::EventBus;
use crate::shutdown::Shutdown;

/// The published view of the surface, for the renderer.
#[derive(Clone, Debug)]
pub struct SurfaceView {
    /// What the display should show.
    pub snapshot: Arc<UiSnapshot>,
    /// What the lights should say.
    pub leds: Arc<pushos_ui::LedPlan>,
}

/// Runs the input pipeline.
pub struct InputTask {
    input: Box<dyn PushInput>,
    surface_kind: SurfacePresence,
    recognizer: GestureRecognizer,
    surface: SurfaceState,
    dispatcher: Arc<ActionDispatcher>,
    config: Arc<ConfigStore>,
    bus: EventBus,
    view: watch::Sender<SurfaceView>,
    reloads: watch::Receiver<u64>,
    gestures: Vec<GestureEvent>,
}

impl std::fmt::Debug for InputTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputTask")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl InputTask {
    /// Builds the pipeline.
    pub fn new(
        input: Box<dyn PushInput>,
        surface_kind: SurfacePresence,
        dispatcher: Arc<ActionDispatcher>,
        config: Arc<ConfigStore>,
        bus: EventBus,
    ) -> (Self, watch::Receiver<SurfaceView>) {
        let current = config.current();
        let surface = SurfaceState::new(Arc::clone(&current));
        let recognizer =
            GestureRecognizer::new(current.timing, current.bindings.gesture_interest().clone());

        let (view, receiver) = watch::channel(build_view(&surface));

        let task = Self {
            input,
            surface_kind,
            recognizer,
            surface,
            dispatcher,
            reloads: config.subscribe(),
            config,
            bus,
            view,
            gestures: Vec::with_capacity(4),
        };
        (task, receiver)
    }

    /// Runs until the surface goes away or shutdown begins.
    pub async fn run(mut self, shutdown: Shutdown) {
        self.surface.set_surface(self.surface_kind, Instant::now());
        self.publish();
        self.bus.publish(EventEnvelope::root(
            EventSource::Push,
            CorrelationId::generate(),
            DomainEvent::PushConnected,
        ));

        loop {
            let deadline = self.next_deadline();

            tokio::select! {
                biased;

                () = shutdown.cancelled() => break,

                // Ahead of input deliberately: a configuration change must take
                // effect before the next gesture is interpreted, or a reload
                // could be overtaken by a pad the operator has already pressed.
                reloaded = self.reloads.changed() => {
                    if reloaded.is_ok() {
                        let config = self.config.current();
                        self.reconfigure(config);
                    }
                }

                event = self.input.next_event() => {
                    let Some(event) = event else {
                        info!("the surface stopped producing input");
                        break;
                    };
                    self.on_input(event).await;
                }

                () = sleep_until(deadline) => {
                    self.on_deadline(Instant::now()).await;
                }
            }
        }

        self.surface
            .set_surface(SurfacePresence::Absent, Instant::now());
        self.recognizer.reset();
        self.publish();
    }

    /// Adopts a new configuration.
    ///
    /// A control already mid-gesture keeps the timers it was armed with, so a
    /// reload cannot strand a press that is already in flight.
    pub fn reconfigure(&mut self, config: Arc<RuntimeConfig>) {
        self.recognizer
            .set_interest(config.bindings.gesture_interest().clone());
        self.surface.adopt(config);
        self.publish();
    }

    async fn on_input(&mut self, event: ControlEvent) {
        self.gestures.clear();
        self.recognizer.observe(event, &mut self.gestures);

        for gesture in std::mem::take(&mut self.gestures) {
            self.on_gesture(gesture).await;
        }
    }

    async fn on_deadline(&mut self, now: Instant) {
        self.gestures.clear();
        self.recognizer.poll(now, &mut self.gestures);

        for gesture in std::mem::take(&mut self.gestures) {
            self.on_gesture(gesture).await;
        }

        if self.surface.expire(now) {
            self.publish();
        }
    }

    async fn on_gesture(&mut self, gesture: GestureEvent) {
        let correlation = CorrelationId::generate();
        let context = self.surface.context(self.recognizer.shift_held());
        let config = Arc::clone(self.surface.config());

        let Some(binding) = BindingResolver::new().resolve(&config.bindings, &gesture, &context)
        else {
            debug!(control = %gesture.control, %gesture.gesture, "no binding applies");
            self.bus.publish(EventEnvelope::root(
                EventSource::Bindings,
                correlation,
                DomainEvent::BindingUnmatched {
                    control: gesture.control,
                    gesture: gesture.gesture,
                },
            ));
            return;
        };

        self.bus.publish(EventEnvelope::root(
            EventSource::Bindings,
            correlation,
            DomainEvent::BindingResolved {
                binding: binding.id.clone(),
                selector: binding.action.selector.clone(),
            },
        ));

        let outcome = self
            .dispatcher
            .dispatch(binding.action.clone(), context, correlation)
            .await;
        let selector = binding.action.selector.clone();

        match outcome {
            Ok(result) => self.on_result(&selector, result, correlation),
            Err(error) => {
                warn!(%selector, %error, class = ?error.class(), "action failed");
                self.surface
                    .notify(error.to_string(), Tone::Failure, Instant::now());
                self.bus.publish(EventEnvelope::root(
                    EventSource::Actions,
                    correlation,
                    DomainEvent::ActionProgressed {
                        selector,
                        status: ActionStatus::Failed,
                        message: Some(error.to_string()),
                    },
                ));
                self.publish();
            }
        }

        // The key is reported after resolution so the audit trail reads in the
        // order things actually happened.
        self.bus.publish(EventEnvelope::root(
            EventSource::Gestures,
            correlation,
            DomainEvent::GestureRecognised {
                key: BindingKey::new(gesture.control, gesture.gesture),
            },
        ));
    }

    fn on_result(
        &mut self,
        selector: &pushos_domain::action::ActionSelector,
        result: ActionResult,
        correlation: CorrelationId,
    ) {
        let now = Instant::now();

        if let Some(display) = result.display
            && let Some(page) = self.surface.apply(display, now)
        {
            self.bus.publish(EventEnvelope::root(
                EventSource::Actions,
                correlation,
                DomainEvent::PageChanged { page },
            ));
        }

        if result.status == ActionStatus::Failed
            && let Some(message) = result.message.clone()
        {
            self.surface.notify(message, Tone::Failure, now);
        }

        self.bus.publish(EventEnvelope::root(
            EventSource::Actions,
            correlation,
            DomainEvent::ActionProgressed {
                selector: selector.clone(),
                status: result.status,
                message: result.message,
            },
        ));

        self.publish();
    }

    fn next_deadline(&self) -> Option<Instant> {
        match (self.recognizer.next_deadline(), self.surface.next_expiry()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (found, None) | (None, found) => found,
        }
    }

    fn publish(&self) {
        // A dropped receiver means the renderer is gone, which shutdown handles.
        let _ = self.view.send(build_view(&self.surface));
    }
}

fn build_view(surface: &SurfaceState) -> SurfaceView {
    let context = surface.context(false);
    SurfaceView {
        leds: Arc::new(leds::plan_for(surface.config(), &context)),
        snapshot: Arc::new(surface.snapshot()),
    }
}

/// Waits until `deadline`, or forever when there is nothing pending.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}
