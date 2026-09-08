//! The action provider port.

use async_trait::async_trait;

use crate::action::{ActionContext, ActionResult};
use crate::error::ActionError;
use crate::ids::{ActionVerb, ProviderName};
use crate::permissions::Permission;

/// Executes the verbs of one namespace.
///
/// Registering a provider must never require a change to the hardware adapter,
/// the binding resolver or the renderer.
#[async_trait]
pub trait ActionProvider: Send + Sync + std::fmt::Debug {
    /// The namespace this provider claims, such as `media`.
    fn name(&self) -> ProviderName;

    /// What the provider can do and what it needs to do it.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Carries out one action.
    ///
    /// Long-running work should return [`crate::action::ActionStatus::Started`]
    /// promptly and report completion through the event bus, so that the
    /// dispatcher is never blocked behind a slow backend.
    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError>;
}

/// The verbs a provider implements and the capabilities they need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Every verb the provider accepts.
    verbs: Vec<ActionVerb>,
    /// Capabilities the provider needs before any of its verbs may run.
    required: Vec<Permission>,
}

impl ProviderCapabilities {
    /// Declares a provider's verbs.
    pub fn new(verbs: impl IntoIterator<Item = ActionVerb>) -> Self {
        Self {
            verbs: verbs.into_iter().collect(),
            required: Vec::new(),
        }
    }

    /// Declares the capabilities the provider needs.
    #[must_use]
    pub fn requiring(mut self, permissions: impl IntoIterator<Item = Permission>) -> Self {
        self.required = permissions.into_iter().collect();
        self
    }

    /// Whether the provider accepts the verb.
    pub fn accepts(&self, verb: &ActionVerb) -> bool {
        self.verbs.contains(verb)
    }

    /// The verbs, in declaration order.
    pub fn verbs(&self) -> &[ActionVerb] {
        &self.verbs
    }

    /// The capabilities the provider needs.
    pub fn required_permissions(&self) -> &[Permission] {
        &self.required
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_accepts_only_the_verbs_it_declared() {
        let capabilities = ProviderCapabilities::new([
            ActionVerb::new("play_pause"),
            ActionVerb::new("next_track"),
        ]);

        assert!(capabilities.accepts(&ActionVerb::new("play_pause")));
        assert!(!capabilities.accepts(&ActionVerb::new("delete_library")));
    }

    #[test]
    fn required_permissions_default_to_none() {
        let capabilities = ProviderCapabilities::new([ActionVerb::new("noop")]);
        assert!(capabilities.required_permissions().is_empty());

        let guarded = capabilities.requiring([Permission::ShellExecute]);
        assert_eq!(guarded.required_permissions(), [Permission::ShellExecute]);
    }
}
