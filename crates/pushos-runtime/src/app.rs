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
use tracing::{info, warn};

use crate::actors::{AgentTask, InputTask, RenderTask, SessionPublisher, TerminalTask};
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

    /// Starts the runtime against an attached surface.
    ///
    /// Returns once every task has stopped.
    pub async fn run(
        self,
        input: Box<dyn PushInput>,
        output: Arc<dyn PushOutput>,
        renderer: PushRenderer,
        shutdown: Shutdown,
    ) {
        let granted = self.config.current().permissions.clone();
        let providers = Arc::new(self.providers);
        let dispatcher = Arc::new(ActionDispatcher::new(Arc::clone(&providers), granted));

        let (pipeline, view, refresh) = InputTask::new(
            input,
            output.kind().into(),
            Arc::clone(&dispatcher),
            Arc::clone(&self.config),
            self.bus.clone(),
        );
        let render = RenderTask::new(output, renderer, view.clone());

        // Built before the tasks take ownership, so the control socket can
        // list sessions without reaching into either supervisor itself.
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
            let plane: Arc<dyn ControlPlane> = Arc::new(
                RuntimeControl::new(Arc::clone(&self.config), providers, dispatcher, view)
                    .with_sessions(sources),
            );
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
            let watching = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watching, move || publishing.publish()).await });
        }

        if let Some(wiring) = self.terminals {
            let task = TerminalTask::new(wiring.supervisor, wiring.updates, self.bus.clone());
            let watching = shutdown.clone();
            let publishing = publisher.clone();
            shutdown.spawn(async move { task.run(watching, move || publishing.publish()).await });
        }

        info!("PushOS running");
        let rendering = shutdown.spawn(render.run(shutdown.clone()));
        let reading = shutdown.spawn(pipeline.run(shutdown.clone()));

        // The pipeline ends when the surface goes away; that is what stops the
        // runtime, not an error.
        let _ = reading.await;
        shutdown.stop().await;
        let _ = rendering.await;
        info!("PushOS stopped");
    }
}
