//! Launching, focusing and opening.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult};
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{
    ActionProvider, ApplicationLauncher, ApplicationTarget, ProviderCapabilities,
};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "app";

/// Opens applications, files and URLs.
#[derive(Debug)]
pub struct ApplicationProvider {
    launcher: Arc<dyn ApplicationLauncher>,
}

impl ApplicationProvider {
    /// Builds the provider over a launcher.
    pub fn new(launcher: Arc<dyn ApplicationLauncher>) -> Self {
        Self { launcher }
    }
}

#[async_trait]
impl ActionProvider for ApplicationProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(["launch", "focus", "open_path", "open_url"].map(ActionVerb::new))
            .requiring([Permission::ApplicationLaunch])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        let params = context.params();
        let target = match context.definition.selector.verb.as_str() {
            // Launching and focusing are the same request to the host: bring
            // the application forward, starting it if it is not running.
            "launch" | "focus" => ApplicationTarget::Application {
                name: params.require_text("target")?.to_owned(),
            },
            "open_path" => ApplicationTarget::Path {
                path: params.require_text("target")?.into(),
                with: params.text("with").map(ToOwned::to_owned),
            },
            "open_url" => ApplicationTarget::Url {
                url: params.require_text("target")?.to_owned(),
            },
            verb => {
                return Err(ActionError::UnknownVerb {
                    provider: self.name(),
                    verb: verb.to_owned(),
                });
            }
        };

        self.launcher.open(&target).await?;
        Ok(ActionResult::completed())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::error::ErrorClass;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::FakeApplications;

    use super::*;

    async fn run(
        apps: &FakeApplications,
        verb: &str,
        params: Params,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        ApplicationProvider::new(Arc::new(apps.clone()))
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    fn target(value: &str) -> Params {
        let mut params = Params::new();
        params.set("target", ParamValue::from(value));
        params
    }

    #[tokio::test]
    async fn launching_and_focusing_make_the_same_request() {
        let apps = FakeApplications::new();
        run(&apps, "launch", target("Cursor"))
            .await
            .expect("succeeds");
        run(&apps, "focus", target("Cursor"))
            .await
            .expect("succeeds");

        let expected = ApplicationTarget::Application {
            name: "Cursor".to_owned(),
        };
        assert_eq!(apps.opened(), [expected.clone(), expected]);
    }

    #[tokio::test]
    async fn a_path_may_name_the_application_that_opens_it() {
        let apps = FakeApplications::new();
        let mut params = target("~/Projects/pushos");
        params.set("with", ParamValue::from("Cursor"));
        run(&apps, "open_path", params).await.expect("succeeds");

        assert_eq!(
            apps.opened(),
            [ApplicationTarget::Path {
                path: PathBuf::from("~/Projects/pushos"),
                with: Some("Cursor".to_owned()),
            }]
        );
    }

    #[tokio::test]
    async fn a_url_is_opened_as_a_url_not_as_a_path() {
        let apps = FakeApplications::new();
        run(&apps, "open_url", target("https://example.com"))
            .await
            .expect("succeeds");
        assert_eq!(
            apps.opened(),
            [ApplicationTarget::Url {
                url: "https://example.com".to_owned()
            }]
        );
    }

    #[tokio::test]
    async fn a_missing_target_is_caught_before_the_host_is_touched() {
        let apps = FakeApplications::new();
        let error = run(&apps, "launch", Params::new())
            .await
            .expect_err("no target");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(apps.opened().is_empty());
    }

    #[test]
    fn launching_is_a_declared_capability() {
        let apps = FakeApplications::new();
        let provider = ApplicationProvider::new(Arc::new(apps));
        assert_eq!(
            provider.capabilities().required_permissions(),
            [Permission::ApplicationLaunch]
        );
    }
}
