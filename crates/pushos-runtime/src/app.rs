//! Assembling the runtime.
//!
//! One place decides which providers exist, which surface is attached and how
//! the tasks are wired. Nothing else needs to know the shape of the whole
//! system.

use std::sync::Arc;

use pushos_actions::{ActionDispatcher, ProviderRegistry};
use pushos_api::{ControlPlane, ControlServer};
use pushos_config::ConfigStore;
use pushos_domain::ports::{PushInput, PushOutput};
use pushos_storage::{EventRecord, Storage};
use pushos_ui::PushRenderer;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::actors::{
    AgentTask, InputTask, RenderTask, SessionPublisher, SurfaceView, TerminalTask, WorkflowTask,
};
use crate::bus::EventBus;
use crate::control::RuntimeControl;
use crate::sessions::{AgentSessions, RunSessions, TerminalSessions};
use crate::shutdown::Shutdown;

/// A configured but not yet running PushOS.
pub struct Runtime {
    config: Arc<ConfigStore>,
    providers: ProviderRegistry,
    storage: Option<Storage>,
    control: Option<ControlServer>,
    agents: Option<AgentWiring>,
    terminals: Option<TerminalWiring>,
    workspaces: Option<Arc<pushos_workspaces::WorkspaceManager>>,
    workflows: Option<WorkflowWiring>,
    voice: Option<Arc<pushos_actions::providers::voice::VoiceProvider>>,
    memory: Option<Arc<pushos_actions::providers::memory::MemoryProvider>>,
    attached: Option<Arc<pushos_actions::providers::session::SessionProvider>>,
    bus: EventBus,
}

/// The workflow engine and the stream it reports runs on.
struct WorkflowWiring {
    engine: Arc<pushos_workflows::WorkflowEngine>,
    updates: tokio::sync::mpsc::UnboundedReceiver<pushos_domain::run::Run>,
}

/// The agent supervisor and the stream its backends report on.
struct AgentWiring {
    supervisor: Arc<pushos_agents::AgentSupervisor>,
    updates: tokio::sync::mpsc::UnboundedReceiver<(
        pushos_domain::ids::SessionId,
        pushos_domain::ports::AgentEvent,
    )>,
}

/// The terminal supervisor and the stream its host reports on.
struct TerminalWiring {
    supervisor: Arc<pushos_terminal::TerminalSupervisor>,
    updates: tokio::sync::mpsc::UnboundedReceiver<(
        pushos_domain::ids::SessionId,
        pushos_domain::ports::TerminalEvent,
    )>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("providers", &self.providers.namespaces())
            .field("persisting", &self.storage.is_some())
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Begins assembling a runtime over a configuration store.
    pub fn new(config: Arc<ConfigStore>) -> Self {
        Self {
            config,
            providers: ProviderRegistry::new(),
            storage: None,
            control: None,
            agents: None,
            terminals: None,
            workspaces: None,
            workflows: None,
            voice: None,
            memory: None,
            attached: None,
            bus: EventBus::new(),
        }
    }

    /// Runs agent sessions, and puts what they are doing on the surface.
    ///
    /// Optional: PushOS runs without agents, it simply has none to drive.
    #[must_use]
    pub fn with_agents(
        mut self,
        supervisor: Arc<pushos_agents::AgentSupervisor>,
        updates: tokio::sync::mpsc::UnboundedReceiver<(
            pushos_domain::ids::SessionId,
            pushos_domain::ports::AgentEvent,
        )>,
    ) -> Self {
        self.agents = Some(AgentWiring {
            supervisor,
            updates,
        });
        self
    }

    /// Runs terminals, and puts what they are doing on the surface.
    ///
    /// Optional: PushOS runs without terminals, it simply has none to drive.
    #[must_use]
    pub fn with_terminals(
        mut self,
        supervisor: Arc<pushos_terminal::TerminalSupervisor>,
        updates: tokio::sync::mpsc::UnboundedReceiver<(
            pushos_domain::ids::SessionId,
            pushos_domain::ports::TerminalEvent,
        )>,
    ) -> Self {
        self.terminals = Some(TerminalWiring {
            supervisor,
            updates,
        });
        self
    }

    /// Follows the project in effect.
    ///
    /// Held so that what the current project was doing is kept when PushOS
    /// stops, and so the control socket can report which one it is.
    #[must_use]
    pub fn with_workspaces(mut self, manager: Arc<pushos_workspaces::WorkspaceManager>) -> Self {
        self.workspaces = Some(manager);
        self
    }

    /// Runs workflows, and puts what they are doing on the surface.
    ///
    /// Optional: PushOS runs without workflows, it simply has none to start.
    #[must_use]
    pub fn with_workflows(
        mut self,
        engine: Arc<pushos_workflows::WorkflowEngine>,
        updates: tokio::sync::mpsc::UnboundedReceiver<pushos_domain::run::Run>,
    ) -> Self {
        self.workflows = Some(WorkflowWiring { engine, updates });
        self
    }

    /// Listens when a control is held, and puts what it hears on the surface.
    ///
    /// Optional: PushOS runs without voice, it simply does not listen. Takes
    /// the provider rather than the listener, because what speech runs has to
    /// go through the same dispatcher a finger uses, and the runtime is what
    /// gives it that.
    #[must_use]
    pub fn with_voice(
        mut self,
        provider: Arc<pushos_actions::providers::voice::VoiceProvider>,
    ) -> Self {
        self.voice = Some(provider);
        self
    }

    /// Keeps notes, and hands them to agents when asked.
    ///
    /// Optional: PushOS runs without memory, it simply has nothing written
    /// down. Takes the provider rather than the library, because a briefing
    /// reaches an agent through the same dispatcher a finger uses.
    #[must_use]
    pub fn with_memory(
        mut self,
        provider: Arc<pushos_actions::providers::memory::MemoryProvider>,
    ) -> Self {
        self.memory = Some(provider);
        self
    }

    /// Watches the terminals the operator already had open.
    ///
    /// Optional: PushOS runs without this, it simply cannot see anything it did
    /// not start itself. Takes the provider rather than the watcher, because
    /// which session the operator is looking at is the provider's to know and
    /// the display follows it.
    #[must_use]
    pub fn with_attached(
        mut self,
        provider: Arc<pushos_actions::providers::session::SessionProvider>,
    ) -> Self {
        self.attached = Some(provider);
        self
    }

    /// Installs an action provider.
    pub fn with_provider(
        mut self,
        provider: Arc<dyn pushos_domain::ports::ActionProvider>,
    ) -> Result<Self, pushos_actions::DuplicateProvider> {
        self.providers.register(provider)?;
        Ok(self)
    }

    /// Records events to the audit trail.
    ///
    /// Optional: PushOS runs without persistence, it simply forgets.
    #[must_use]
    pub fn with_storage(mut self, storage: Storage) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Serves a control socket, so PushOS Studio can configure the surface
    /// while it is running.
    ///
    /// Optional: without one PushOS runs perfectly well, it simply cannot be
    /// configured from another process.
    #[must_use]
    pub fn with_control_socket(mut self, server: ControlServer) -> Self {
        self.control = Some(server);
        self
    }

    /// The event bus, for components that want to observe the runtime.
    pub fn bus(&self) -> EventBus {
        self.bus.clone()
    }

    /// Starts everything that outlives the hardware.
    ///
    /// Agents, terminals, projects and the control socket are not the surface's
    /// to own. A Push 2 unplugged and plugged back in must find its sessions
    /// still running and its project still selected, and PushOS with nothing
    /// attached must still be configurable, or Studio could not reach a machine
    /// whose device is in a bag.
    pub async fn start(self, shutdown: &Shutdown) -> RunningRuntime {
        let granted = self.config.current().permissions.clone();
        let providers = Arc::new(self.providers);
        let dispatcher = Arc::new(ActionDispatcher::new(Arc::clone(&providers), granted));

        // Given after construction, because the dispatcher this points at
        // contains the provider that points back at the engine.
        if self.workflows.is_some() || self.voice.is_some() || self.memory.is_some() {
            let runner: Arc<dyn pushos_domain::ports::ActionRunner> = Arc::clone(&dispatcher) as _;
            if let Some(wiring) = &self.workflows {
                wiring.engine.use_actions(&runner).await;
            }
            if let Some(provider) = &self.voice {
                provider.use_actions(&runner).await;
            }
            if let Some(provider) = &self.memory {
                provider.use_actions(&runner).await;
            }
        }

        let mut watched_sessions = None;

        // Created once, for the life of the process. A surface publishes into
        // the view; with none attached it holds the waiting screen.
        let (view, watching) = watch::channel(SurfaceView::waiting());
        let (refresh, lines) = watch::channel(Vec::new());

        let mut publisher = SessionPublisher::new(refresh);
        let mut sources: Vec<Arc<dyn pushos_api::SessionSource>> = Vec::new();
        if let Some(wiring) = &self.agents {
            publisher = publisher.with_agents(Arc::clone(&wiring.supervisor));
            sources.push(Arc::new(AgentSessions::new(Arc::clone(&wiring.supervisor))));
        }
        if let Some(wiring) = &self.terminals {
            publisher = publisher.with_terminals(Arc::clone(&wiring.supervisor));
            sources.push(Arc::new(TerminalSessions::new(Arc::clone(
                &wiring.supervisor,
            ))));
        }
        if let Some(wiring) = &self.workflows {
            publisher = publisher.with_workflows(Arc::clone(&wiring.engine));
            sources.push(Arc::new(RunSessions::new(Arc::clone(&wiring.engine))));
        }

        // Nothing tells PushOS when a terminal window opens, so it asks, and
        // the display follows the answer rather than asking for itself.
        if let Some(provider) = &self.attached {
            let (task, found) = crate::actors::AttachedTask::new(provider.watcher());
            let task = task.every(self.config.current().session_poll);
            publisher = publisher.with_attached(found.clone(), provider.watch());
            watched_sessions = Some(found);

            let watching = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watching, move || publishing.publish()).await });
        }

        if let Some(server) = self.control {
            let mut control = RuntimeControl::new(
                Arc::clone(&self.config),
                providers,
                Arc::clone(&dispatcher),
                watching.clone(),
            )
            .with_sessions(sources);
            if let Some(manager) = &self.workspaces {
                control = control.with_workspaces(Arc::clone(manager));
            }
            let plane: Arc<dyn ControlPlane> = Arc::new(control);
            let serving = shutdown.clone();
            shutdown.spawn(async move {
                server
                    .serve(plane, async move { serving.cancelled().await })
                    .await;
            });
        }

        if let Some(storage) = self.storage.clone() {
            let mut events = self.bus.subscribe();
            let audit = shutdown.clone();
            shutdown.spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        () = audit.cancelled() => break,
                        envelope = events.recv() => {
                            let Some(envelope) = envelope else { break };
                            if let Err(error) =
                                storage.record_event(EventRecord::from_envelope(&envelope))
                            {
                                warn!(%error, "could not record an event");
                            }
                        }
                    }
                }
            });
        }

        let engine = self
            .workflows
            .as_ref()
            .map(|wiring| Arc::clone(&wiring.engine));

        if let Some(wiring) = self.agents {
            let mut task = AgentTask::new(wiring.supervisor, wiring.updates, self.bus.clone());
            if let Some(engine) = &engine {
                task = task.feeding(Arc::clone(engine));
            }
            let watched = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watched, move || publishing.publish()).await });
        }

        if let Some(wiring) = self.terminals {
            let mut task = TerminalTask::new(wiring.supervisor, wiring.updates, self.bus.clone());
            if let Some(engine) = &engine {
                task = task.feeding(Arc::clone(engine));
            }
            let watched = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watched, move || publishing.publish()).await });
        }

        // Started before anything can search: a directory of notes written
        // while PushOS was not running is the normal case, not an exception.
        if let Some(provider) = self.memory {
            let indexing = Arc::clone(&provider);
            shutdown.spawn(async move {
                // The library says how many and from where; saying it again
                // here would be two lines about one thing.
                if let Err(error) = indexing.reindex().await {
                    warn!(%error, "notes could not be indexed");
                }
            });
        }

        if let Some(wiring) = self.workflows {
            let task = WorkflowTask::new(wiring.updates, self.bus.clone());
            let watched = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watched, move || publishing.publish()).await });

            // Picked up after everything that can advance them is listening, so
            // a run resumed here finds its agents and terminals ready.
            let resuming = wiring.engine;
            shutdown.spawn(async move { resuming.resume().await });
        }

        RunningRuntime {
            config: self.config,
            dispatcher,
            workspaces: self.workspaces,
            view,
            watching,
            lines,
            sessions: watched_sessions,
            listening: self.voice.map(|provider| provider.listener().watch()),
            bus: self.bus,
        }
    }

    /// Starts the runtime and serves one surface until it goes away.
    ///
    /// What the tests and the simulated surface use. Real hardware comes and
    /// goes, so the CLI starts once and serves repeatedly.
    pub async fn run(
        self,
        input: Box<dyn PushInput>,
        output: Arc<dyn PushOutput>,
        renderer: PushRenderer,
        shutdown: Shutdown,
    ) {
        let running = self.start(&shutdown).await;
        running.serve(input, output, renderer, &shutdown).await;
        running.stop().await;

        shutdown.stop().await;
        info!("PushOS stopped");
    }
}

/// The parts of PushOS that keep running whether or not a Push 2 is attached.
pub struct RunningRuntime {
    config: Arc<ConfigStore>,
    dispatcher: Arc<ActionDispatcher>,
    workspaces: Option<Arc<pushos_workspaces::WorkspaceManager>>,
    view: watch::Sender<SurfaceView>,
    watching: watch::Receiver<SurfaceView>,
    lines: watch::Receiver<Vec<pushos_ui::SessionLine>>,
    /// Whether the microphone is on, when voice is configured.
    listening: Option<watch::Receiver<pushos_domain::voice::Listening>>,
    /// The sessions PushOS did not start, when any are watched.
    sessions: Option<watch::Receiver<Vec<pushos_domain::attached::Attached>>>,
    bus: EventBus,
}

impl std::fmt::Debug for RunningRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningRuntime")
            .field("projects", &self.workspaces.is_some())
            .finish_non_exhaustive()
    }
}

impl RunningRuntime {
    /// Drives one surface until it goes away.
    ///
    /// Returns when the Push 2 is unplugged or shutdown begins. Everything
    /// above stays exactly as it was, so plugging it back in resumes rather
    /// than starts again.
    pub async fn serve(
        &self,
        input: Box<dyn PushInput>,
        output: Arc<dyn PushOutput>,
        renderer: PushRenderer,
        shutdown: &Shutdown,
    ) {
        let pipeline = InputTask::new(
            input,
            output.kind().into(),
            Arc::clone(&self.dispatcher),
            Arc::clone(&self.config),
            self.bus.clone(),
            self.view.clone(),
            self.lines.clone(),
        );

        // The surface follows the project rather than keeping its own answer,
        // so the two can never disagree about where the operator is.
        let pipeline = match &self.workspaces {
            Some(manager) => pipeline.following(manager.subscribe()),
            None => pipeline,
        };

        // A surface plugged in mid-phrase shows that PushOS is listening,
        // rather than waiting for the operator to let go and press again.
        let pipeline = match &self.listening {
            Some(listening) => pipeline.hearing(listening.clone()),
            None => pipeline,
        };

        // So the lights show what each session is doing, not merely that a pad
        // is bound to one.
        let pipeline = match &self.sessions {
            Some(sessions) => pipeline.watching(sessions.clone()),
            None => pipeline,
        };
        let render = RenderTask::new(output, renderer, self.watching.clone());

        // The surface's own coordinator. Its tasks end when the Push 2 goes
        // away, or when the whole runtime does, and nothing above it is
        // disturbed either way.
        let attached = shutdown.child();

        info!("PushOS running");
        let rendering = attached.spawn(render.run(attached.clone()));
        let reading = attached.spawn(pipeline.run(attached.clone()));

        // The pipeline ends when the surface goes away; that is what ends this,
        // not an error.
        let _ = reading.await;

        // Told to stop rather than dropped, so it finishes the frame it is on
        // and leaves the display in a state someone chose.
        attached.stop().await;
        let _ = rendering.await;
    }

    /// The event bus, for components that want to observe the runtime.
    pub fn bus(&self) -> EventBus {
        self.bus.clone()
    }

    /// What the surface is showing, for anything that needs to record it.
    pub fn view(&self) -> watch::Receiver<SurfaceView> {
        self.watching.clone()
    }

    /// Keeps what the project in effect was doing, before PushOS stops.
    pub async fn stop(&self) {
        if let Some(manager) = &self.workspaces {
            let context = self.watching.borrow().context.clone();
            manager.remember_current(&context).await;
        }
    }
}
