//! Page navigation.
//!
//! The provider decides *where* to go and nothing else. Applying the move is
//! the job of whatever owns the current page, which is why this provider needs
//! no state and can be tested on its own.

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, DisplayIntent};
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::page::PageTarget;
use pushos_domain::ports::{ActionProvider, ProviderCapabilities};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "page";

/// Moves the surface between pages.
#[derive(Debug, Default, Clone, Copy)]
pub struct PageProvider;

impl PageProvider {
    /// Builds the provider.
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ActionProvider for PageProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(["show", "next", "previous", "home", "back"].map(ActionVerb::new))
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        let target = match context.definition.selector.verb.as_str() {
            "show" => PageTarget::Named(context.params().require_text("target")?.into()),
            "next" => PageTarget::Next,
            "previous" => PageTarget::Previous,
            "home" => PageTarget::Home,
            "back" => PageTarget::Back,
            verb => {
                return Err(ActionError::UnknownVerb {
                    provider: self.name(),
                    verb: verb.to_owned(),
                });
            }
        };

        Ok(ActionResult::completed().with_display(DisplayIntent::Page(target)))
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;

    use super::*;

    async fn run(verb: &str, params: Params) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        PageProvider::new()
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    #[tokio::test]
    async fn showing_a_page_asks_for_that_page_by_name() {
        let mut params = Params::new();
        params.set("target", ParamValue::from("music"));

        let result = run("show", params).await.expect("the target is present");
        assert_eq!(
            result.display,
            Some(DisplayIntent::Page(PageTarget::Named("music".into())))
        );
    }

    #[tokio::test]
    async fn showing_a_page_without_a_target_is_a_validation_error() {
        let error = run("show", Params::new())
            .await
            .expect_err("no target was given");
        assert_eq!(error.class(), pushos_domain::error::ErrorClass::Validation);
    }

    #[tokio::test]
    async fn relative_moves_need_no_parameters() {
        for (verb, expected) in [
            ("next", PageTarget::Next),
            ("previous", PageTarget::Previous),
            ("home", PageTarget::Home),
            ("back", PageTarget::Back),
        ] {
            let result = run(verb, Params::new())
                .await
                .expect("relative moves take no target");
            assert_eq!(result.display, Some(DisplayIntent::Page(expected)));
        }
    }

    #[test]
    fn the_provider_declares_exactly_the_verbs_it_handles() {
        let capabilities = PageProvider::new().capabilities();
        assert_eq!(capabilities.verbs().len(), 5);
        assert!(
            capabilities.required_permissions().is_empty(),
            "moving pages needs no capability"
        );
    }
}
