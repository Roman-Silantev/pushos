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
    AgentTask, InputTask, RenderTask, SessionPublisher, SurfaceView, TerminalTask,
};
use crate::bus::EventBus;
use crate::control::RuntimeControl;
use crate::sessions::{AgentSessions, TerminalSessions};
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
    bus: EventBus,
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
    pub fn start(self, shutdown: &Shutdown) -> RunningRuntime {
        let granted = self.config.current().permissions.clone();
        let providers = Arc::new(self.providers);
        let dispatcher = Arc::new(ActionDispatcher::new(Arc::clone(&providers), granted));

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

        if let Some(wiring) = self.agents {
            let task = AgentTask::new(wiring.supervisor, wiring.updates, self.bus.clone());
            let watched = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watched, move || publishing.publish()).await });
        }

        if let Some(wiring) = self.terminals {
            let task = TerminalTask::new(wiring.supervisor, wiring.updates, self.bus.clone());
            let watched = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watched, move || publishing.publish()).await });
        }

        RunningRuntime {
            config: self.config,
            dispatcher,
            workspaces: self.workspaces,
            view,
            watching,
            lines,
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
        let running = self.start(&shutdown);
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
