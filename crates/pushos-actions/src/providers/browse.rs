//! Moving the browser.
//!
//! The provider decides *which way*, and nothing else. What is at that place
//! in the tree, and what choosing it does, belongs to whatever owns the live
//! state — the same division as page navigation, and for the same reason: a
//! provider that knew the tree would have to be given every registry in PushOS
//! and would go stale the moment one of them changed.

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, DisplayIntent};
use pushos_domain::browse::BrowseMove;
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::ports::{ActionProvider, ProviderCapabilities};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "browse";

/// How many rows one turn of a knob moves.
const ONE_ROW: i8 = 1;

/// Opens and moves the browser.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrowseProvider;

impl BrowseProvider {
    /// Builds the provider.
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ActionProvider for BrowseProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Nothing here needs permission. Moving a cursor does nothing on its
        // own; the action a chosen row runs is checked when it runs, the way
        // every other action is.
        ProviderCapabilities::new(
            ["open", "close", "next", "previous", "enter", "leave"].map(ActionVerb::new),
        )
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        let move_to = match context.definition.selector.verb.as_str() {
            "open" => BrowseMove::Open,
            "close" => BrowseMove::Close,
            "next" => BrowseMove::Step(ONE_ROW),
            "previous" => BrowseMove::Step(-ONE_ROW),
            "enter" => BrowseMove::Enter,
            "leave" => BrowseMove::Leave,
            verb => {
                return Err(ActionError::UnknownVerb {
                    provider: self.name(),
                    verb: verb.to_owned(),
                });
            }
        };

        Ok(ActionResult::completed().with_display(DisplayIntent::Browse(move_to)))
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;

    use super::*;

    async fn run(verb: &str) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), Params::new());
        BrowseProvider::new()
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    fn moved(result: &ActionResult) -> BrowseMove {
        match result.display.as_ref() {
            Some(DisplayIntent::Browse(move_to)) => *move_to,
            other => panic!("expected a browse move, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn each_verb_asks_for_its_own_move() {
        for (verb, wanted) in [
            ("open", BrowseMove::Open),
            ("close", BrowseMove::Close),
            ("next", BrowseMove::Step(1)),
            ("previous", BrowseMove::Step(-1)),
            ("enter", BrowseMove::Enter),
            ("leave", BrowseMove::Leave),
        ] {
            let result = run(verb).await.expect("the verb exists");
            assert_eq!(moved(&result), wanted, "`{verb}` asked for the wrong move");
        }
    }

    #[tokio::test]
    async fn a_turn_either_way_moves_one_row() {
        // A knob that jumped several rows a detent would be unusable on a
        // list of sixty-four.
        assert_eq!(
            moved(&run("next").await.expect("next")),
            BrowseMove::Step(1)
        );
        assert_eq!(
            moved(&run("previous").await.expect("previous")),
            BrowseMove::Step(-1)
        );
    }

    #[tokio::test]
    async fn an_unknown_verb_is_refused() {
        assert!(run("rummage").await.is_err());
    }

    #[tokio::test]
    async fn moving_the_browser_needs_no_permission() {
        // It moves a cursor. What a chosen row runs is checked when it runs.
        assert!(
            BrowseProvider::new()
                .capabilities()
                .required_for(&ActionVerb::new("enter"))
                .is_empty()
        );
    }
}
