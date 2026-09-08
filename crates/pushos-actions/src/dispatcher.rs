//! The action dispatcher.
//!
//! One place turns a resolved binding into a running action. Nothing in the
//! dispatcher knows what any particular action does, so a Push control never
//! needs custom execution logic.

use std::sync::Arc;
use std::time::Duration;

use pushos_domain::action::{ActionContext, ActionDefinition, ActionResult};
use pushos_domain::context::SurfaceContext;
use pushos_domain::error::ActionError;
use pushos_domain::ids::CorrelationId;
use pushos_domain::permissions::PermissionSet;
use tracing::{debug, warn};

use crate::registry::ProviderRegistry;

/// How long an action may run before the dispatcher gives up on it.
///
/// A provider that expects to take longer must return `Started` promptly and
/// report completion through the event bus, rather than holding the dispatcher.
pub const DEFAULT_ACTION_BUDGET: Duration = Duration::from_secs(30);

/// Runs actions on behalf of resolved bindings.
#[derive(Debug)]
pub struct ActionDispatcher {
    registry: Arc<ProviderRegistry>,
    granted: PermissionSet,
    budget: Duration,
}

impl ActionDispatcher {
    /// Builds a dispatcher over a registry and the permissions in force.
    pub fn new(registry: Arc<ProviderRegistry>, granted: PermissionSet) -> Self {
        Self {
            registry,
            granted,
            budget: DEFAULT_ACTION_BUDGET,
        }
    }

    /// Sets how long an action may run before it is abandoned.
    #[must_use]
    pub const fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// The permissions actions run under.
    pub const fn granted(&self) -> &PermissionSet {
        &self.granted
    }

    /// Runs one action.
    ///
    /// Permission is checked before the provider is reached, so a provider can
    /// never be the only thing standing between a binding and a capability.
    pub async fn dispatch(
        &self,
        definition: ActionDefinition,
        surface: SurfaceContext,
        correlation_id: CorrelationId,
    ) -> Result<ActionResult, ActionError> {
        let provider = self.registry.resolve(&definition.selector)?;

        for permission in provider.capabilities().required_permissions() {
            self.granted.require(*permission)?;
        }

        let selector = definition.selector.to_string();
        let context = ActionContext::new(definition, correlation_id, surface);
        let execution_id = context.execution_id;

        debug!(%selector, %correlation_id, %execution_id, "dispatching action");

        let Ok(result) = tokio::time::timeout(self.budget, provider.execute(context)).await else {
            warn!(%selector, %correlation_id, "action exceeded its budget");
            return Err(ActionError::TimedOut {
                selector,
                timeout_ms: u64::try_from(self.budget.as_millis()).unwrap_or(u64::MAX),
            });
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use pushos_domain::action::{ActionSelector, ActionStatus};
    use pushos_domain::error::ErrorClass;
    use pushos_domain::ids::{ActionVerb, ProviderName};
    use pushos_domain::permissions::Permission;
    use pushos_domain::ports::{ActionProvider, ProviderCapabilities};

    use super::*;

    /// A provider that takes longer than any budget a test will give it.
    #[derive(Debug)]
    struct SlowProvider;

    #[async_trait]
    impl ActionProvider for SlowProvider {
        fn name(&self) -> ProviderName {
            ProviderName::new("slow")
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::new([ActionVerb::new("wait")])
        }

        async fn execute(&self, _context: ActionContext) -> Result<ActionResult, ActionError> {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            Ok(ActionResult::completed())
        }
    }

    /// A provider that needs a capability before it may run.
    #[derive(Debug)]
    struct GuardedProvider;

    #[async_trait]
    impl ActionProvider for GuardedProvider {
        fn name(&self) -> ProviderName {
            ProviderName::new("guarded")
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::new([ActionVerb::new("run")])
                .requiring([Permission::ShellExecute])
        }

        async fn execute(&self, _context: ActionContext) -> Result<ActionResult, ActionError> {
            Ok(ActionResult::completed())
        }
    }

    fn dispatcher_with(
        providers: Vec<Arc<dyn ActionProvider>>,
        granted: PermissionSet,
    ) -> ActionDispatcher {
        let mut registry = ProviderRegistry::new();
        for provider in providers {
            registry
                .register(provider)
                .expect("namespaces are distinct");
        }
        ActionDispatcher::new(Arc::new(registry), granted)
    }

    async fn dispatch(
        dispatcher: &ActionDispatcher,
        selector: &str,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::bare(
            selector
                .parse::<ActionSelector>()
                .expect("well-formed selector"),
        );
        dispatcher
            .dispatch(
                definition,
                SurfaceContext::empty(),
                CorrelationId::generate(),
            )
            .await
    }

    #[tokio::test]
    async fn an_action_with_no_provider_fails_validation_before_anything_runs() {
        let dispatcher = dispatcher_with(Vec::new(), PermissionSet::empty());
        let error = dispatch(&dispatcher, "nothing.here")
            .await
            .expect_err("no provider");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_missing_permission_stops_the_action_before_the_provider_is_reached() {
        let dispatcher = dispatcher_with(vec![Arc::new(GuardedProvider)], PermissionSet::empty());
        let error = dispatch(&dispatcher, "guarded.run")
            .await
            .expect_err("no shell permission");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[tokio::test]
    async fn a_granted_permission_lets_the_action_through() {
        let granted = PermissionSet::from_iter([Permission::ShellExecute]);
        let dispatcher = dispatcher_with(vec![Arc::new(GuardedProvider)], granted);
        let result = dispatch(&dispatcher, "guarded.run")
            .await
            .expect("permission is granted");
        assert_eq!(result.status, ActionStatus::Completed);
    }

    #[tokio::test(start_paused = true)]
    async fn an_action_that_overruns_its_budget_is_abandoned_rather_than_awaited() {
        let dispatcher = dispatcher_with(vec![Arc::new(SlowProvider)], PermissionSet::empty())
            .with_budget(Duration::from_millis(50));

        let error = dispatch(&dispatcher, "slow.wait")
            .await
            .expect_err("the provider never returns");
        assert!(matches!(error, ActionError::TimedOut { .. }));
        assert!(
            error.is_retryable(),
            "a timeout may be retried, subject to idempotency"
        );
    }
}
