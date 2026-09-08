//! Moving between projects from the surface.
//!
//! One gesture, several consequences. Selecting a project moves the surface,
//! brings its bindings into force, restores the page it was last on, and points
//! everything started afterwards at its directory. The provider asks the
//! manager to do all of that and reports where the operator ended up.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, DisplayIntent};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::page::PageTarget;
use pushos_domain::permissions::Permission;
use pushos_domain::ports::ProviderCapabilities;
use pushos_domain::ports::{ActionProvider, ApplicationLauncher, ApplicationTarget};
use pushos_domain::workspace::{MalformedWorkspaceTarget, WorkspaceTarget};
use pushos_workspaces::{Arrival, WorkspaceManager, describe_missing_app};
use tracing::info;

/// The namespace this provider claims.
pub const NAMESPACE: &str = "workspace";

/// Turns gestures into project operations.
#[derive(Debug)]
pub struct WorkspaceProvider {
    manager: Arc<WorkspaceManager>,
    applications: Arc<dyn ApplicationLauncher>,
}

impl WorkspaceProvider {
    /// Builds the provider over the manager.
    pub fn new(manager: Arc<WorkspaceManager>, applications: Arc<dyn ApplicationLauncher>) -> Self {
        Self {
            manager,
            applications,
        }
    }

    /// Reads the target a binding names.
    fn target(context: &ActionContext) -> Result<WorkspaceTarget, ActionError> {
        let written = context.params().require_text("target")?;
        written.parse().map_err(|error: MalformedWorkspaceTarget| {
            ActionError::backend(error.to_string(), ErrorClass::Validation, error)
        })
    }

    /// Moves, and reports where that left the operator.
    async fn go(
        &self,
        target: &WorkspaceTarget,
        context: &ActionContext,
    ) -> Result<ActionResult, ActionError> {
        let arrival = self
            .manager
            .select(target, &context.surface)
            .await
            .map_err(|error| {
                ActionError::backend(error.to_string(), ErrorClass::Validation, error)
            })?;

        info!(%target, workspace = ?arrival.workspace, "workspace selected");
        Ok(report(arrival))
    }

    /// Opens the project in one of the applications it names.
    async fn open(&self, context: &ActionContext) -> Result<ActionResult, ActionError> {
        let Some(workspace) = self.manager.current_workspace().await else {
            return Err(ActionError::backend(
                "no project is selected, so there is nothing to open",
                ErrorClass::Validation,
                std::io::Error::other("no workspace"),
            ));
        };

        // Without a name, the project's own directory: "open this project" is
        // what the pad usually means.
        let Some(app) = context.params().text("app") else {
            self.applications
                .open(&ApplicationTarget::Path {
                    path: workspace.root.clone(),
                    with: None,
                })
                .await?;
            return Ok(opened(
                &workspace.name,
                workspace.root.display().to_string(),
            ));
        };

        let path = workspace.app(app).ok_or_else(|| {
            ActionError::backend(
                describe_missing_app(workspace, app),
                ErrorClass::Validation,
                std::io::Error::other("no such application"),
            )
        })?;

        self.applications
            .open(&ApplicationTarget::Path {
                path: path.into(),
                with: Some(app.to_owned()),
            })
            .await?;
        Ok(opened(&workspace.name, format!("{app} \u{2014} {path}")))
    }
}

#[async_trait]
impl ActionProvider for WorkspaceProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            ["select", "next", "previous", "leave", "open"].map(ActionVerb::new),
        )
        // Moving between projects changes what every other control means, but
        // it starts nothing, so it needs no capability. Opening one launches an
        // application, and that does.
        .verb_requiring(ActionVerb::new("open"), [Permission::ApplicationLaunch])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "select" => {
                let target = Self::target(&context)?;
                self.go(&target, &context).await
            }
            "next" => self.go(&WorkspaceTarget::Next, &context).await,
            "previous" => self.go(&WorkspaceTarget::Previous, &context).await,
            "leave" => self.go(&WorkspaceTarget::None, &context).await,
            "open" => self.open(&context).await,
            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Turns an arrival into what the surface should do.
fn report(arrival: Arrival) -> ActionResult {
    let message = match &arrival.name {
        Some(name) => name.clone(),
        None => "no project".to_owned(),
    };

    ActionResult {
        status: ActionStatus::Completed,
        message: Some(message),
        // Only the page. Which project is in effect is the manager's to say,
        // and the surface follows it rather than being told twice.
        display: arrival
            .page
            .map(|page| DisplayIntent::Page(PageTarget::Named(page))),
    }
}

fn opened(name: &str, detail: String) -> ActionResult {
    ActionResult {
        status: ActionStatus::Completed,
        message: Some(format!("opened {name}")),
        display: Some(DisplayIntent::Toast {
            title: name.to_owned(),
            detail: Some(detail),
        }),
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::{CorrelationId, PageId, WorkspaceId};
    use pushos_domain::workspace::Workspace;
    use pushos_testkit::{FakeApplications, FakeRepository, FakeWorkspaceMemory};
    use pushos_workspaces::WorkspaceRegistry;

    use super::*;

    struct Fixture {
        provider: WorkspaceProvider,
        manager: Arc<WorkspaceManager>,
        applications: FakeApplications,
    }

    impl Fixture {
        fn new() -> Self {
            let mut sydclaw = Workspace::new("sydclaw", "Sydclaw", "/tmp/sydclaw");
            sydclaw.home_page = Some(PageId::new("development"));
            sydclaw
                .apps
                .insert("Cursor".to_owned(), "/tmp/sydclaw".to_owned());

            let manager = Arc::new(WorkspaceManager::new(
                WorkspaceRegistry::new([
                    sydclaw,
                    Workspace::new("pushos", "PushOS", "/tmp/pushos"),
                ]),
                Arc::new(FakeWorkspaceMemory::new()),
                Arc::new(FakeRepository::new("/tmp/pushos")),
                "/tmp/nowhere",
                "/tmp/trees",
            ));
            let applications = FakeApplications::new();

            Self {
                provider: WorkspaceProvider::new(
                    Arc::clone(&manager),
                    Arc::new(applications.clone()),
                ),
                manager,
                applications,
            }
        }

        async fn run(&self, verb: &str, params: Params) -> Result<ActionResult, ActionError> {
            self.run_from(verb, params, SurfaceContext::empty()).await
        }

        async fn run_from(
            &self,
            verb: &str,
            params: Params,
            surface: SurfaceContext,
        ) -> Result<ActionResult, ActionError> {
            let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
            self.provider
                .execute(ActionContext::new(
                    definition,
                    CorrelationId::generate(),
                    surface,
                ))
                .await
        }
    }

    fn target(name: &str) -> Params {
        let mut params = Params::new();
        params.set("target", ParamValue::Text(name.into()));
        params
    }

    #[tokio::test]
    async fn selecting_a_project_moves_the_surface_to_it_and_to_its_page() {
        // Both at once, because arriving in a project and then arriving on its
        // page are not two things the operator did.
        let fixture = Fixture::new();
        let result = fixture
            .run("select", target("sydclaw"))
            .await
            .expect("the project is configured");

        assert_eq!(result.status, ActionStatus::Completed);
        assert_eq!(
            result.display,
            Some(DisplayIntent::Page(PageTarget::Named(PageId::new(
                "development"
            )))),
            "the page it restored"
        );
        assert_eq!(result.message.as_deref(), Some("Sydclaw"));
        assert_eq!(
            fixture.manager.current().await,
            Some(WorkspaceId::new("sydclaw")),
            "and the project itself, which the surface follows"
        );
    }

    #[tokio::test]
    async fn stepping_moves_through_the_configured_order() {
        let fixture = Fixture::new();
        fixture
            .run("next", Params::new())
            .await
            .expect("something is configured");

        assert_eq!(
            fixture.manager.current().await,
            Some(WorkspaceId::new("sydclaw"))
        );

        fixture
            .run_from(
                "next",
                Params::new(),
                SurfaceContext::empty().in_workspace("sydclaw"),
            )
            .await
            .expect("something is configured");
        assert_eq!(
            fixture.manager.current().await,
            Some(WorkspaceId::new("pushos"))
        );
    }

    #[tokio::test]
    async fn leaving_puts_the_operator_in_no_project_at_all() {
        let fixture = Fixture::new();
        fixture
            .run("select", target("sydclaw"))
            .await
            .expect("configured");

        let result = fixture
            .run_from(
                "leave",
                Params::new(),
                SurfaceContext::empty().in_workspace("sydclaw"),
            )
            .await
            .expect("leaving is always possible");

        assert_eq!(result.display, None, "there is no page to restore");
        assert_eq!(fixture.manager.current().await, None);
    }

    #[tokio::test]
    async fn naming_a_project_nothing_configures_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("select", target("nowhere"))
            .await
            .expect_err("nothing configures it");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(error.to_string().contains("nowhere"), "{error}");
    }

    #[tokio::test]
    async fn selecting_without_naming_anything_is_refused() {
        // Guessing would silently move the operator out of the project they
        // were in.
        let fixture = Fixture::new();
        let error = fixture
            .run("select", Params::new())
            .await
            .expect_err("a target is needed");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn opening_a_project_without_naming_an_application_opens_its_directory() {
        let fixture = Fixture::new();
        fixture
            .run("select", target("sydclaw"))
            .await
            .expect("configured");
        fixture
            .run("open", Params::new())
            .await
            .expect("the fake succeeds by default");

        assert_eq!(
            fixture.applications.opened(),
            [ApplicationTarget::Path {
                path: "/tmp/sydclaw".into(),
                with: None
            }]
        );
    }

    #[tokio::test]
    async fn opening_with_a_named_application_uses_what_the_project_configured() {
        let fixture = Fixture::new();
        fixture
            .run("select", target("sydclaw"))
            .await
            .expect("configured");

        let mut params = Params::new();
        params.set("app", ParamValue::Text("Cursor".into()));
        fixture
            .run("open", params)
            .await
            .expect("the fake succeeds by default");

        assert_eq!(
            fixture.applications.opened(),
            [ApplicationTarget::Path {
                path: "/tmp/sydclaw".into(),
                with: Some("Cursor".to_owned())
            }]
        );
    }

    #[tokio::test]
    async fn opening_an_application_the_project_does_not_name_says_which_it_does() {
        let fixture = Fixture::new();
        fixture
            .run("select", target("sydclaw"))
            .await
            .expect("configured");

        let mut params = Params::new();
        params.set("app", ParamValue::Text("Xcode".into()));
        let error = fixture.run("open", params).await.expect_err("no such app");

        let message = error.to_string();
        assert!(message.contains("Xcode"), "{message}");
        assert!(message.contains("Cursor"), "{message}");
    }

    #[tokio::test]
    async fn opening_with_no_project_selected_says_so() {
        let fixture = Fixture::new();
        let error = fixture
            .run("open", Params::new())
            .await
            .expect_err("nothing is selected");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_verb_the_provider_does_not_have_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("teleport", Params::new())
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }

    #[test]
    fn moving_between_projects_needs_no_capability_but_opening_one_does() {
        // Gating navigation behind a launch permission would teach the operator
        // to grant more than they meant to.
        let fixture = Fixture::new();
        let capabilities = fixture.provider.capabilities();

        assert!(capabilities.required_permissions().is_empty());
        assert!(
            capabilities
                .required_for(&ActionVerb::new("select"))
                .is_empty()
        );
        assert_eq!(
            capabilities.required_for(&ActionVerb::new("open")),
            [Permission::ApplicationLaunch]
        );
        assert_eq!(
            capabilities.every_permission(),
            [Permission::ApplicationLaunch]
        );
    }

    #[test]
    fn every_verb_the_provider_answers_is_one_it_declares() {
        let fixture = Fixture::new();
        let capabilities = fixture.provider.capabilities();
        for verb in ["select", "next", "previous", "leave", "open"] {
            assert!(
                capabilities.accepts(&ActionVerb::new(verb)),
                "`{verb}` is answered but not declared"
            );
        }
    }
}
