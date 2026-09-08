//! The provider registry.
//!
//! Providers claim a namespace and are looked up by it. Registering one
//! requires no change to the hardware adapter, the binding resolver or the
//! renderer, which is the architectural test the specification sets.

use std::collections::HashMap;
use std::sync::Arc;

use pushos_domain::action::ActionSelector;
use pushos_domain::error::ActionError;
use pushos_domain::ids::ProviderName;
use pushos_domain::ports::ActionProvider;

/// Every action provider currently installed, indexed by namespace.
#[derive(Debug, Default)]
pub struct ProviderRegistry {
    providers: HashMap<ProviderName, Arc<dyn ActionProvider>>,
}

impl ProviderRegistry {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs a provider under the namespace it claims.
    ///
    /// Two providers may not claim the same namespace: silently replacing one
    /// would make the surface's behaviour depend on registration order.
    pub fn register(&mut self, provider: Arc<dyn ActionProvider>) -> Result<(), DuplicateProvider> {
        let name = provider.name();
        if self.providers.contains_key(&name) {
            return Err(DuplicateProvider { name });
        }
        self.providers.insert(name, provider);
        Ok(())
    }

    /// Finds the provider that implements a selector, checking the verb too.
    pub fn resolve(
        &self,
        selector: &ActionSelector,
    ) -> Result<&Arc<dyn ActionProvider>, ActionError> {
        let provider =
            self.providers
                .get(&selector.provider)
                .ok_or_else(|| ActionError::UnknownProvider {
                    provider: selector.provider.clone(),
                })?;

        if provider.capabilities().accepts(&selector.verb) {
            Ok(provider)
        } else {
            Err(ActionError::UnknownVerb {
                provider: selector.provider.clone(),
                verb: selector.verb.to_string(),
            })
        }
    }

    /// The namespaces currently installed, in a stable order.
    pub fn namespaces(&self) -> Vec<ProviderName> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// How many providers are installed.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether no providers are installed.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// Two providers claimed the same namespace.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a provider is already registered for `{name}`")]
pub struct DuplicateProvider {
    /// The contested namespace.
    pub name: ProviderName,
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use pushos_domain::action::{ActionContext, ActionResult};
    use pushos_domain::ids::ActionVerb;
    use pushos_domain::ports::ProviderCapabilities;

    use super::*;

    #[derive(Debug)]
    struct Stub(&'static str, &'static [&'static str]);

    #[async_trait]
    impl ActionProvider for Stub {
        fn name(&self) -> ProviderName {
            ProviderName::new(self.0)
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::new(self.1.iter().copied().map(ActionVerb::new))
        }

        async fn execute(&self, _context: ActionContext) -> Result<ActionResult, ActionError> {
            Ok(ActionResult::completed())
        }
    }

    fn registry_with(providers: &[Stub]) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        for &Stub(name, verbs) in providers {
            registry
                .register(Arc::new(Stub(name, verbs)))
                .expect("namespaces are distinct");
        }
        registry
    }

    #[test]
    fn a_registered_provider_answers_its_own_verbs() {
        let registry = registry_with(&[Stub("media", &["play_pause"])]);
        let selector: ActionSelector = "media.play_pause".parse().expect("well-formed");
        assert!(registry.resolve(&selector).is_ok());
    }

    #[test]
    fn an_unclaimed_namespace_is_reported_as_such() {
        let registry = registry_with(&[Stub("media", &["play_pause"])]);
        let selector: ActionSelector = "agent.start".parse().expect("well-formed");
        let error = registry
            .resolve(&selector)
            .expect_err("no agent provider is installed");
        assert!(matches!(error, ActionError::UnknownProvider { .. }));
    }

    #[test]
    fn a_known_provider_still_rejects_an_unknown_verb() {
        let registry = registry_with(&[Stub("media", &["play_pause"])]);
        let selector: ActionSelector = "media.self_destruct".parse().expect("well-formed");
        let error = registry
            .resolve(&selector)
            .expect_err("the verb is not implemented");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }

    #[test]
    fn a_second_provider_cannot_quietly_take_over_a_namespace() {
        let mut registry = registry_with(&[Stub("media", &["play_pause"])]);
        let error = registry
            .register(Arc::new(Stub("media", &["something_else"])))
            .expect_err("the namespace is taken");
        assert_eq!(error.name.as_str(), "media");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn namespaces_are_listed_in_a_stable_order() {
        let registry = registry_with(&[
            Stub("shell", &["run"]),
            Stub("app", &["open"]),
            Stub("media", &["x"]),
        ]);
        let names: Vec<_> = registry
            .namespaces()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(names, ["app", "media", "shell"]);
    }
}
