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
use crate::actors::rest::{Heard, RestClock};
use crate::bus::EventBus;
use crate::shutdown::Shutdown;

/// The published view of the surface, for the renderer.
#[derive(Clone, Debug)]
pub struct SurfaceView {
    /// What the display should show.
    pub snapshot: Arc<UiSnapshot>,
    /// What the lights should say.
    pub leds: Arc<pushos_ui::LedPlan>,
    /// Where the operator is, for anything that needs to record it.
    pub context: pushos_domain::context::SurfaceContext,
    /// Whether anyone is using the surface.
    pub rest: pushos_domain::rest::Rest,
    /// How bright the lights and the screen should be because of that.
    pub levels: pushos_domain::rest::Levels,
}

impl SurfaceView {
    /// What there is to show before any surface is attached.
    ///
    /// PushOS runs with nothing plugged in, so this is a real state rather than
    /// a placeholder: it is what a client asking now would be told.
    pub fn waiting() -> Self {
        Self {
            snapshot: Arc::new(UiSnapshot::disconnected()),
            leds: Arc::new(pushos_ui::LedPlan::new()),
            context: pushos_domain::context::SurfaceContext::empty(),
            rest: pushos_domain::rest::Rest::Awake,
            levels: pushos_domain::rest::RestPolicy::DEFAULT
                .levels(pushos_domain::rest::Rest::Awake),
        }
    }
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
    /// Which project is in effect, published by the one thing that decides.
    ///
    /// Followed rather than tracked: the surface keeping its own answer is how
    /// two parts of PushOS come to disagree about where the operator is.
    projects: Option<watch::Receiver<Option<pushos_domain::ids::WorkspaceId>>>,
    /// Whether the microphone is on, published by the listener.
    ///
    /// Followed here rather than asked for when drawing, because the answer has
    /// to reach the display within the press that changed it.
    listening: Option<watch::Receiver<pushos_domain::voice::Listening>>,
    /// Whether the computer is awake, from whatever hears it sleep.
    power: Option<watch::Receiver<pushos_domain::rest::MachinePower>>,
    /// The sessions PushOS did not start, as last seen.
    ///
    /// Followed so the lights can show what each one is doing. A pad standing
    /// for a session that is stuck on a question has to look different from
    /// seven that are not.
    attached: Option<watch::Receiver<Vec<pushos_domain::attached::Attached>>>,
    /// Which bank of eight is being shown, so a pad naming a slot lights for
    /// the session actually under it.
    bank: Option<watch::Receiver<usize>>,
    /// What the sessions are doing, published by whoever is watching them.
    ///
    /// The latest list is the only one that matters, and a pipeline built for
    /// a surface that has just been plugged in needs the current one rather
    /// than a queue of everything it missed.
    sessions: watch::Receiver<super::Progress>,
    /// What each role's agent is doing, as last published.
    roles: Vec<super::RoleActivity>,
    /// The questions sessions are waiting on the operator to answer.
    questions: Option<watch::Receiver<Vec<pushos_domain::attached::SessionQuestion>>>,
    /// How many were waiting when last looked, so only a new one is announced.
    questions_seen: usize,
    gestures: Vec<GestureEvent>,
    /// Whether anyone is using the surface, for a board left on all day.
    rest: RestClock,
}

/// Tells the input pipeline what the sessions are doing.
pub(crate) type SessionRefresh = watch::Sender<super::Progress>;

impl std::fmt::Debug for InputTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputTask")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl InputTask {
    /// Builds the pipeline for one surface.
    ///
    /// The view and the session lines are given rather than created, because
    /// they outlive any one surface: a Push 2 unplugged and plugged back in
    /// gets a new pipeline, and everything it is meant to show is still there.
    pub(crate) fn new(
        input: Box<dyn PushInput>,
        surface_kind: SurfacePresence,
        dispatcher: Arc<ActionDispatcher>,
        config: Arc<ConfigStore>,
        bus: EventBus,
        view: watch::Sender<SurfaceView>,
        sessions: watch::Receiver<super::Progress>,
    ) -> Self {
        let current = config.current();
        let surface = SurfaceState::new(Arc::clone(&current));
        let recognizer =
            GestureRecognizer::new(current.timing, current.bindings.gesture_interest().clone());
        let rest = RestClock::new(current.rest, Instant::now());

        Self {
            input,
            surface_kind,
            recognizer,
            surface,
            dispatcher,
            reloads: config.subscribe(),
            projects: None,
            listening: None,
            power: None,
            attached: None,
            bank: None,
            rest,
            config,
            bus,
            view,
            sessions,
            roles: Vec::new(),
            questions: None,
            questions_seen: 0,
            gestures: Vec::with_capacity(4),
        }
    }

    /// Follows the project in effect.
    #[must_use]
    pub fn following(
        mut self,
        projects: watch::Receiver<Option<pushos_domain::ids::WorkspaceId>>,
    ) -> Self {
        self.projects = Some(projects);
        self
    }

    /// Follows whether PushOS is listening.
    #[must_use]
    pub fn hearing(mut self, listening: watch::Receiver<pushos_domain::voice::Listening>) -> Self {
        self.listening = Some(listening);
        self
    }

    /// Follows whether the computer is awake.
    #[must_use]
    pub fn minding(mut self, power: watch::Receiver<pushos_domain::rest::MachinePower>) -> Self {
        self.power = Some(power);
        self
    }

    /// Follows the sessions PushOS did not start.
    #[must_use]
    pub fn watching(
        mut self,
        attached: watch::Receiver<Vec<pushos_domain::attached::Attached>>,
        bank: watch::Receiver<usize>,
    ) -> Self {
        self.attached = Some(attached);
        self.bank = Some(bank);
        self
    }

    /// Follows the questions sessions put to the operator.
    #[must_use]
    pub(crate) fn asked(
        mut self,
        questions: watch::Receiver<Vec<pushos_domain::attached::SessionQuestion>>,
    ) -> Self {
        self.questions = Some(questions);
        self
    }

    /// Puts a newly asked question on the display.
    ///
    /// Only a new one: the display is not interrupted again because one of
    /// several was answered. The session's pad is already blinking for it, so
    /// this is what it wants, in words.
    fn follow_questions(&mut self) {
        let Some(questions) = self.questions.as_mut() else {
            return;
        };
        let waiting = questions.borrow_and_update().clone();
        if waiting.len() > self.questions_seen
            && let Some(newest) = waiting.last()
        {
            self.surface.apply(
                pushos_domain::action::DisplayIntent::Prompt {
                    question: newest.describe(),
                    choices: vec!["Allow".to_owned(), "Deny".to_owned()],
                },
                Instant::now(),
            );
            self.publish();
        }
        self.questions_seen = waiting.len();
    }

    /// Runs until the surface goes away or shutdown begins.
    pub async fn run(mut self, shutdown: Shutdown) {
        self.surface.set_surface(self.surface_kind, Instant::now());

        // Taken up front rather than waited for: a surface plugged in while a
        // project is selected and agents are working should show that, not an
        // empty display until the next thing happens.
        self.follow_project();
        self.follow_listening();
        let progress = self.sessions.borrow_and_update().clone();
        self.surface.set_sessions(progress.lines);
        self.roles = progress.roles;
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

                // For the same reason: a pad pressed after switching project
                // must be resolved in the project it was pressed in.
                moved = wait_for_project(self.projects.as_mut()) => {
                    if moved {
                        self.follow_project();
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

                () = wait_for_sessions(&mut self.sessions) => {
                    let progress = self.sessions.borrow_and_update().clone();
                    self.surface.set_sessions(progress.lines);
                    self.roles = progress.roles;
                    self.publish();
                }

                heard = wait_for_listening(self.listening.as_mut()) => {
                    if heard {
                        self.follow_listening();
                    }
                }

                // Ahead of nothing in particular, but acted on at once: the
                // computer is holding its sleep until the lights are out.
                powered = wait_for_power(self.power.as_mut()) => {
                    if powered {
                        self.follow_power();
                    }
                }

                moved = wait_for_attached(self.attached.as_mut()) => {
                    if moved {
                        self.publish();
                    }
                }

                banked = wait_for_bank(self.bank.as_mut()) => {
                    if banked {
                        self.publish();
                    }
                }

                () = wait_for_questions(self.questions.as_mut()) => {
                    self.follow_questions();
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
        self.rest.adopt(config.rest, Instant::now());
        self.surface.adopt(config);
        self.publish();
    }

    /// Takes whether the computer is awake.
    fn follow_power(&mut self) {
        let Some(power) = self.power.as_mut() else {
            return;
        };
        let state = *power.borrow_and_update();
        if self.rest.on_machine(state, Instant::now()) {
            self.publish();
        }
    }

    /// Takes whether the microphone is on.
    fn follow_listening(&mut self) {
        let Some(listening) = self.listening.as_mut() else {
            return;
        };
        let state = *listening.borrow_and_update();
        self.surface.set_listening(state);
        self.publish();
    }

    /// Takes the project the manager most recently announced.
    fn follow_project(&mut self) {
        let Some(projects) = self.projects.as_mut() else {
            return;
        };
        let workspace = projects.borrow_and_update().clone();
        if workspace == self.surface.workspace() {
            return;
        }

        self.surface.select_workspace(workspace.clone());
        self.bus.publish(EventEnvelope::root(
            EventSource::Actions,
            CorrelationId::generate(),
            DomainEvent::WorkspaceChanged { workspace },
        ));
        self.publish();
    }

    async fn on_input(&mut self, event: ControlEvent) {
        let (heard, changed) = self.rest.on_input(&event);
        if changed {
            self.publish();
        }
        if heard == Heard::Spent {
            debug!(control = %event.control, "spent waking the surface");
            return;
        }

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

        let expired = self.surface.expire(now);
        let rested = self.rest.on_time(now);
        if expired || rested {
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

        let selector = binding.action.selector.clone();
        let outcome = self
            .dispatcher
            .dispatch(binding.action.clone(), context, correlation)
            .await;

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

        // Before the display instruction, so that an action which moved
        // project and restored a page lands as one change rather than two.
        if self
            .projects
            .as_ref()
            .is_some_and(|projects| projects.has_changed().unwrap_or(false))
        {
            self.follow_project();
        }

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
        [
            self.recognizer.next_deadline(),
            self.surface.next_expiry(),
            self.rest.next_change(Instant::now()),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn publish(&mut self) {
        let sessions = self
            .attached
            .as_ref()
            .map(|held| held.borrow().clone())
            .unwrap_or_default();
        let from = self.bank.as_ref().map_or(0, |bank| *bank.borrow());
        let mut view = build_view(&self.surface, &sessions, from, &self.roles);

        // Looked at here because this is where every change to the lights
        // passes. Something newly asking for a person wakes the surface in the
        // same publish that shows it.
        self.rest.on_asking(view.leds.asking(), Instant::now());
        view.rest = self.rest.state();
        view.levels = self.rest.levels();

        // A dropped receiver means the renderer is gone, which shutdown handles.
        let _ = self.view.send(view);
    }
}

fn build_view(
    surface: &SurfaceState,
    sessions: &[pushos_domain::attached::Attached],
    from: usize,
    roles: &[super::RoleActivity],
) -> SurfaceView {
    let context = surface.context(false);
    let rest = pushos_domain::rest::Rest::Awake;
    SurfaceView {
        leds: Arc::new(leds::plan_showing(
            surface.config(),
            &context,
            &leds::Showing {
                sessions,
                from,
                roles,
            },
        )),
        snapshot: Arc::new(surface.snapshot()),
        context,
        // Overwritten by the caller, which owns the clock.
        rest,
        levels: surface.config().rest.levels(rest),
    }
}

/// Waits for the project in effect to change, or forever when nothing says.
async fn wait_for_project(
    projects: Option<&mut watch::Receiver<Option<pushos_domain::ids::WorkspaceId>>>,
) -> bool {
    match projects {
        Some(projects) => projects.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Waits for the sessions to change, and for ever once nothing can publish.
///
/// A closed channel is what a PushOS with no agents, terminals or workflows
/// has: the publisher was dropped because there was nothing for it to watch.
/// Returning from that immediately, as `changed` does, would spin this loop at
/// the speed of the processor and starve every arm below it.
async fn wait_for_sessions(sessions: &mut watch::Receiver<super::Progress>) {
    if sessions.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Waits for the open sessions to change, or forever when none are watched.
async fn wait_for_attached(
    attached: Option<&mut watch::Receiver<Vec<pushos_domain::attached::Attached>>>,
) -> bool {
    match attached {
        Some(attached) => attached.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Waits for the bank being shown to move, or forever when none is watched.
/// Waits for the questions waiting on the operator to change, or forever when
/// nothing puts questions to them or whatever did has gone.
async fn wait_for_questions(
    questions: Option<&mut watch::Receiver<Vec<pushos_domain::attached::SessionQuestion>>>,
) {
    if let Some(questions) = questions
        && questions.changed().await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

async fn wait_for_bank(bank: Option<&mut watch::Receiver<usize>>) -> bool {
    match bank {
        Some(bank) => bank.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Waits for the microphone to go on or off, or forever when voice is off.
async fn wait_for_listening(
    listening: Option<&mut watch::Receiver<pushos_domain::voice::Listening>>,
) -> bool {
    match listening {
        Some(listening) => listening.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Waits for the computer to sleep or wake, or forever when nothing says.
async fn wait_for_power(
    power: Option<&mut watch::Receiver<pushos_domain::rest::MachinePower>>,
) -> bool {
    match power {
        Some(power) => power.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Waits until `deadline`, or forever when there is nothing pending.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}
