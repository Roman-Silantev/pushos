//! Apple Shortcuts.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult};
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ProviderCapabilities, ShortcutRunner};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "shortcut";

/// Runs Shortcuts on the operator's behalf.
#[derive(Debug)]
pub struct ShortcutProvider {
    runner: Arc<dyn ShortcutRunner>,
}

impl ShortcutProvider {
    /// Builds the provider over a Shortcuts backend.
    pub fn new(runner: Arc<dyn ShortcutRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl ActionProvider for ShortcutProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(["run", "list"].map(ActionVerb::new))
            .requiring([Permission::ShortcutsExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "run" => {
                let name = context.params().require_text("target")?;
                self.runner
                    .run(name, context.params().text("input"))
                    .await?;
                Ok(ActionResult::completed().with_message(format!("ran `{name}`")))
            }
            "list" => {
                let names = self.runner.list().await?;
                Ok(ActionResult::completed()
                    .with_message(format!("{} shortcuts available", names.len())))
            }
            verb => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: verb.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::error::ErrorClass;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::FakeShortcuts;

    use super::*;

    async fn run(
        shortcuts: &FakeShortcuts,
        verb: &str,
        params: Params,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        ShortcutProvider::new(Arc::new(shortcuts.clone()))
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    #[tokio::test]
    async fn a_shortcut_runs_by_name_with_optional_input() {
        let shortcuts = FakeShortcuts::with_shortcuts(["Office Lights".to_owned()]);
        let mut params = Params::new();
        params.set("target", ParamValue::from("Office Lights"));
        params.set("input", ParamValue::from("on"));

        run(&shortcuts, "run", params).await.expect("succeeds");
        assert_eq!(
            shortcuts.runs(),
            [("Office Lights".to_owned(), Some("on".to_owned()))]
        );
    }

    #[tokio::test]
    async fn input_is_optional() {
        let shortcuts = FakeShortcuts::default();
        let mut params = Params::new();
        params.set("target", ParamValue::from("End Workday"));

        run(&shortcuts, "run", params).await.expect("succeeds");
        assert_eq!(shortcuts.runs(), [("End Workday".to_owned(), None)]);
    }

    #[tokio::test]
    async fn running_without_naming_a_shortcut_is_a_validation_error() {
        let shortcuts = FakeShortcuts::default();
        let error = run(&shortcuts, "run", Params::new())
            .await
            .expect_err("no name");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(shortcuts.runs().is_empty());
    }

    #[tokio::test]
    async fn listing_reports_how_many_are_available() {
        let shortcuts = FakeShortcuts::with_shortcuts(["One".to_owned(), "Two".to_owned()]);
        let result = run(&shortcuts, "list", Params::new())
            .await
            .expect("succeeds");
        assert_eq!(result.message.as_deref(), Some("2 shortcuts available"));
    }
}
