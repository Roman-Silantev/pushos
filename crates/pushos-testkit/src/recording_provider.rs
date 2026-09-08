//! An action provider that records rather than acts.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionDefinition, ActionResult, ActionStatus};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ExecutionId, ProviderName};
use pushos_domain::ports::{ActionProvider, ProviderCapabilities};

/// What a recording provider should do when asked to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptedOutcome {
    /// Report success.
    Complete,
    /// Report that work continues in the background.
    Start,
    /// Report that the operator must decide something.
    Wait,
    /// Fail with the given classification.
    Fail(ErrorClass),
}

/// A provider that answers to any verb it was declared with, and remembers
/// every call.
///
/// The recorded execution ids make it possible to assert that a retried action
/// was not carried out twice.
#[derive(Debug, Clone)]
pub struct RecordingProvider {
    name: ProviderName,
    verbs: Vec<ActionVerb>,
    outcome: Arc<Mutex<ScriptedOutcome>>,
    calls: Arc<Mutex<Vec<(ActionDefinition, ExecutionId)>>>,
}

impl RecordingProvider {
    /// Builds a provider claiming a namespace and a set of verbs.
    pub fn new(
        name: impl Into<ProviderName>,
        verbs: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            verbs: verbs.into_iter().map(ActionVerb::new).collect(),
            outcome: Arc::new(Mutex::new(ScriptedOutcome::Complete)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Sets what every subsequent execution will do.
    pub fn will(&self, outcome: ScriptedOutcome) {
        if let Ok(mut current) = self.outcome.lock() {
            *current = outcome;
        }
    }

    /// Every action the provider was asked to run, in order.
    pub fn calls(&self) -> Vec<ActionDefinition> {
        self.calls
            .lock()
            .map(|calls| {
                calls
                    .iter()
                    .map(|(definition, _)| definition.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many times the provider was executed.
    pub fn call_count(&self) -> usize {
        self.calls
            .lock()
            .map(|calls| calls.len())
            .unwrap_or_default()
    }

    /// The execution ids the provider saw, which must differ per execution.
    pub fn execution_ids(&self) -> Vec<ExecutionId> {
        self.calls
            .lock()
            .map(|calls| calls.iter().map(|(_, id)| *id).collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl ActionProvider for RecordingProvider {
    fn name(&self) -> ProviderName {
        self.name.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(self.verbs.clone())
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((context.definition.clone(), context.execution_id));
        }

        let outcome = self
            .outcome
            .lock()
            .map_or(ScriptedOutcome::Complete, |outcome| outcome.clone());

        match outcome {
            ScriptedOutcome::Complete => Ok(ActionResult::completed()),
            ScriptedOutcome::Start => Ok(ActionResult::started()),
            ScriptedOutcome::Wait => Ok(ActionResult {
                status: ActionStatus::Waiting,
                message: Some("waiting on the operator".to_owned()),
                display: None,
            }),
            ScriptedOutcome::Fail(class) => Err(ActionError::backend(
                "the recording provider was told to fail",
                class,
                std::io::Error::other("injected failure"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::ActionSelector;
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;

    use super::*;

    fn context(provider: &RecordingProvider, verb: &str) -> ActionContext {
        ActionContext::new(
            ActionDefinition::bare(ActionSelector::new(provider.name(), verb)),
            CorrelationId::generate(),
            SurfaceContext::empty(),
        )
    }

    #[tokio::test]
    async fn calls_are_recorded_with_distinct_execution_ids() {
        let provider = RecordingProvider::new("test", ["noop"]);
        provider
            .execute(context(&provider, "noop"))
            .await
            .expect("succeeds by default");
        provider
            .execute(context(&provider, "noop"))
            .await
            .expect("succeeds by default");

        assert_eq!(provider.call_count(), 2);
        let ids = provider.execution_ids();
        assert_ne!(
            ids[0], ids[1],
            "each execution gets its own idempotency key"
        );
    }

    #[tokio::test]
    async fn the_scripted_outcome_is_what_comes_back() {
        let provider = RecordingProvider::new("test", ["noop"]);

        provider.will(ScriptedOutcome::Start);
        let result = provider
            .execute(context(&provider, "noop"))
            .await
            .expect("started");
        assert_eq!(result.status, ActionStatus::Started);

        provider.will(ScriptedOutcome::Fail(ErrorClass::Permission));
        let error = provider
            .execute(context(&provider, "noop"))
            .await
            .expect_err("told to fail");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[test]
    fn a_provider_only_declares_the_verbs_it_was_given() {
        let provider = RecordingProvider::new("test", ["one", "two"]);
        let capabilities = provider.capabilities();
        assert!(capabilities.accepts(&ActionVerb::new("one")));
        assert!(!capabilities.accepts(&ActionVerb::new("three")));
    }
}
