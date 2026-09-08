//! Answering the control socket.
//!
//! Studio's edits land here. Every one of them goes through the same validated,
//! atomic path a hand edit does: the configuration files stay the source of
//! truth, and an edit that would produce an unusable surface is refused before
//! anything is written.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_actions::{ActionDispatcher, ProviderRegistry};
use pushos_api::protocol::{
    BindingList, ControlInfo, EditReport, Failure, FailureKind, GridPosition, PROTOCOL_VERSION,
    PageInfo, ProviderInfo, SessionList, StatusReport, SurfaceReport, TestReport, Vocabulary,
    WorkspaceInfo, WorkspaceList,
};
use pushos_api::{ControlPlane, SessionSource};
use pushos_config::{
    BindingAddress, BindingSpec, ConfigDocuments, ConfigError, ConfigStore, RuntimeConfig,
};
use pushos_domain::binding::{Binding, BindingScope};
use pushos_domain::context::SurfaceContext;
use pushos_domain::controls::ControlId;
use pushos_domain::gesture::Gesture;
use pushos_domain::ids::CorrelationId;
use pushos_ui::SurfacePresence;
use tokio::sync::watch;
use tracing::info;

use crate::actors::SurfaceView;

/// The version reported to clients.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Serves control requests against a running PushOS.
#[derive(Debug)]
pub struct RuntimeControl {
    config: Arc<ConfigStore>,
    providers: Arc<ProviderRegistry>,
    dispatcher: Arc<ActionDispatcher>,
    view: watch::Receiver<SurfaceView>,
    /// Where live sessions are read from, one source per kind of work.
    sessions: Vec<Arc<dyn SessionSource>>,
    /// The projects, when any are configured.
    workspaces: Option<Arc<pushos_workspaces::WorkspaceManager>>,
}

impl RuntimeControl {
    /// Builds a control plane over the runtime's parts.
    pub fn new(
        config: Arc<ConfigStore>,
        providers: Arc<ProviderRegistry>,
        dispatcher: Arc<ActionDispatcher>,
        view: watch::Receiver<SurfaceView>,
    ) -> Self {
        Self {
            config,
            providers,
            dispatcher,
            view,
            sessions: Vec::new(),
            workspaces: None,
        }
    }

    /// Reports the projects this manager knows about.
    #[must_use]
    pub fn with_workspaces(mut self, manager: Arc<pushos_workspaces::WorkspaceManager>) -> Self {
        self.workspaces = Some(manager);
        self
    }

    /// Reports the sessions these sources are running.
    #[must_use]
    pub fn with_sessions(mut self, sources: Vec<Arc<dyn SessionSource>>) -> Self {
        self.sessions = sources;
        self
    }

    fn report(&self, config: &RuntimeConfig) -> StatusReport {
        let view = self.view.borrow();
        StatusReport {
            protocol: PROTOCOL_VERSION,
            version: VERSION.to_owned(),
            surface: match view.snapshot.surface {
                SurfacePresence::Hardware => SurfaceReport::Push2,
                SurfacePresence::Simulated => SurfaceReport::Simulated,
                SurfacePresence::Absent => SurfaceReport::Absent,
            },
            page: view.snapshot.page.id.as_ref().map(ToString::to_string),
            workspace: view.snapshot.workspace.clone(),
            config_root: self.config.root().display().to_string(),
            binding_count: config.bindings.len(),
        }
    }

    /// Opens the configuration for editing.
    fn documents(&self) -> Result<ConfigDocuments, Failure> {
        ConfigDocuments::load(self.config.root()).map_err(|error| rejected(&error))
    }

    /// Saves an edit and makes it take effect.
    ///
    /// The reload is what puts the change on the surface; without it Studio
    /// would report success while the operator's pads carried on unchanged.
    fn commit(&self, mut documents: ConfigDocuments) -> Result<usize, Failure> {
        documents.save().map_err(|error| rejected(&error))?;
        let config = self.config.reload().map_err(|error| rejected(&error))?;
        Ok(config.bindings.len())
    }
}

#[async_trait]
impl ControlPlane for RuntimeControl {
    async fn status(&self) -> StatusReport {
        self.report(&self.config.current())
    }

    async fn describe(&self) -> Vocabulary {
        let config = self.config.current();

        Vocabulary {
            controls: ControlId::all().map(describe_control).collect(),
            gestures: Gesture::ALL
                .iter()
                .map(|gesture| gesture.slug().to_owned())
                .collect(),
            pages: config
                .pages
                .iter()
                .map(|page| PageInfo {
                    id: page.id.to_string(),
                    name: page.name.clone(),
                    description: page.description.clone(),
                })
                .collect(),
            providers: self
                .providers
                .namespaces()
                .into_iter()
                .filter_map(|name| {
                    let capabilities = self.providers.capabilities(&name)?;
                    let requires: Vec<_> = capabilities
                        .required_permissions()
                        .iter()
                        .map(|permission| permission.slug().to_owned())
                        .collect();
                    let permitted = capabilities
                        .required_permissions()
                        .iter()
                        .all(|permission| config.permissions.allows(*permission));

                    Some(ProviderInfo {
                        name: name.to_string(),
                        verbs: capabilities
                            .verbs()
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                        requires,
                        permitted,
                    })
                })
                .collect(),
        }
    }

    async fn bindings(&self) -> Result<BindingList, Failure> {
        let bindings: Vec<_> = self
            .config
            .current()
            .bindings
            .iter()
            .map(describe_binding)
            .collect();
        Ok(BindingList { bindings })
    }

    async fn bind(&self, spec: BindingSpec) -> Result<EditReport, Failure> {
        let mut documents = self.documents()?;
        let edit = documents.upsert_binding(&spec);
        let binding_count = self.commit(documents)?;

        info!(control = %spec.address.control, action = %spec.action, "binding written by a client");
        Ok(EditReport {
            file: edit.file.display().to_string(),
            replaced: edit.replaced,
            binding_count,
        })
    }

    async fn unbind(&self, address: BindingAddress) -> Result<EditReport, Failure> {
        let mut documents = self.documents()?;
        let Some(file) = documents.remove_binding(&address) else {
            return Err(Failure::new(
                FailureKind::NotFound,
                format!(
                    "`{}` has no `{}` binding here",
                    address.control, address.gesture
                ),
            ));
        };

        let binding_count = self.commit(documents)?;
        Ok(EditReport {
            file: file.display().to_string(),
            replaced: true,
            binding_count,
        })
    }

    async fn test(&self, address: BindingAddress) -> Result<TestReport, Failure> {
        let config = self.config.current();
        let binding = find_binding(&config, &address).ok_or_else(|| {
            Failure::new(
                FailureKind::NotFound,
                format!(
                    "`{}` has no `{}` binding here",
                    address.control, address.gesture
                ),
            )
        })?;

        let action = binding.action.selector.to_string();
        let surface = surface_for(&binding.scope);

        match self
            .dispatcher
            .dispatch(binding.action.clone(), surface, CorrelationId::generate())
            .await
        {
            Ok(result) => Ok(TestReport {
                action,
                status: format!("{:?}", result.status).to_lowercase(),
                message: result.message,
            }),
            Err(error) => {
                let kind = match error.class() {
                    pushos_domain::error::ErrorClass::Permission => FailureKind::Denied,
                    pushos_domain::error::ErrorClass::Validation => FailureKind::Rejected,
                    _ => FailureKind::Internal,
                };
                Err(Failure::new(kind, error.to_string()))
            }
        }
    }

    async fn sessions(&self) -> Result<SessionList, Failure> {
        let mut sessions = Vec::new();
        for source in &self.sessions {
            sessions.extend(source.sessions().await);
        }
        Ok(SessionList { sessions })
    }

    async fn workspaces(&self) -> Result<WorkspaceList, Failure> {
        let Some(manager) = &self.workspaces else {
            return Ok(WorkspaceList::default());
        };

        let current = manager.current().await;
        let workspaces = manager
            .registry()
            .all()
            .iter()
            .map(|workspace| WorkspaceInfo {
                id: workspace.id.to_string(),
                name: workspace.name.clone(),
                root: workspace.root.display().to_string(),
                description: workspace.description.clone(),
                home_page: workspace.home_page.as_ref().map(ToString::to_string),
                current: current.as_ref() == Some(&workspace.id),
                isolate_agents: workspace.isolate_agents,
                apps: workspace.apps.keys().cloned().collect(),
                // Checked rather than assumed: a project pointing somewhere
                // that has been moved would otherwise fail only when a pad was
                // pressed.
                root_exists: workspace.root.is_dir(),
            })
            .collect();

        Ok(WorkspaceList { workspaces })
    }

    async fn reload(&self) -> Result<StatusReport, Failure> {
        let config = self.config.reload().map_err(|error| rejected(&error))?;
        Ok(self.report(&config))
    }
}

/// Builds the context a binding would resolve in, so testing it runs the same
/// action the operator would get by pressing the control.
fn surface_for(scope: &BindingScope) -> SurfaceContext {
    match scope {
        BindingScope::Global => SurfaceContext::empty(),
        BindingScope::Page(page) => SurfaceContext::empty().on_page(page.clone()),
        BindingScope::Workspace(workspace) => {
            SurfaceContext::empty().in_workspace(workspace.clone())
        }
        BindingScope::WorkspacePage { workspace, page } => SurfaceContext::empty()
            .in_workspace(workspace.clone())
            .on_page(page.clone()),
    }
}

fn find_binding<'config>(
    config: &'config RuntimeConfig,
    address: &BindingAddress,
) -> Option<&'config Binding> {
    config
        .bindings
        .iter()
        .find(|binding| describe_binding(binding).address.matches(address))
}

fn describe_control(control: ControlId) -> ControlInfo {
    ControlInfo {
        id: control.to_string(),
        kind: format!("{:?}", control.kind()).to_lowercase(),
        illuminated: control.is_illuminated(),
        grid: match control {
            ControlId::Pad(pad) => Some(GridPosition {
                row: pad.row(),
                column: pad.column(),
            }),
            _ => None,
        },
    }
}

fn describe_binding(binding: &Binding) -> BindingSpec {
    let (workspace, page) = match &binding.scope {
        BindingScope::Global => (None, None),
        BindingScope::Page(page) => (None, Some(page.to_string())),
        BindingScope::Workspace(workspace) => (Some(workspace.to_string()), None),
        BindingScope::WorkspacePage { workspace, page } => {
            (Some(workspace.to_string()), Some(page.to_string()))
        }
    };

    let mut params: std::collections::BTreeMap<_, _> = binding
        .action
        .params
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect();
    // `target` is written back as the shorthand it was read from.
    let target = params
        .remove("target")
        .and_then(|value| value.as_text().map(ToOwned::to_owned));

    BindingSpec {
        address: BindingAddress {
            control: binding.control.to_string(),
            gesture: binding.gesture.slug().to_owned(),
            page,
            workspace,
        },
        action: binding.action.selector.to_string(),
        target,
        params,
        label: binding.label.clone(),
        priority: binding.priority,
    }
}

/// Turns a configuration failure into one a client can act on, keeping every
/// individual problem rather than flattening them into one sentence.
fn rejected(error: &ConfigError) -> Failure {
    let problems = error
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Failure::new(FailureKind::Rejected, error.to_string()).with_problems(problems)
}
