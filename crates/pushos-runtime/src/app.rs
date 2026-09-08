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

use crate::actors::{InputTask, RenderTask};
use crate::bus::EventBus;
use crate::control::RuntimeControl;
use crate::shutdown::Shutdown;

/// A configured but not yet running PushOS.
pub struct Runtime {
    config: Arc<ConfigStore>,
    providers: ProviderRegistry,
    storage: Option<Storage>,
    control: Option<ControlServer>,
    bus: EventBus,
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
            bus: EventBus::new(),
        }
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

        let (pipeline, view) = InputTask::new(
            input,
            Arc::clone(&dispatcher),
            Arc::clone(&self.config),
            self.bus.clone(),
        );
        let render = RenderTask::new(output, renderer, view.clone());

        if let Some(server) = self.control {
            let plane: Arc<dyn ControlPlane> = Arc::new(RuntimeControl::new(
                Arc::clone(&self.config),
                providers,
                dispatcher,
                view,
            ));
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
