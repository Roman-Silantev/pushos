//! Turning a target into a session.
//!
//! A binding names a role, because a provider's session identifier will not
//! outlive the day and a pad is expected to. Resolving that role is this
//! component's whole job: find the session filling it, or open one.

use std::collections::HashMap;
use std::sync::Arc;

use pushos_domain::agent::{AgentDefinition, AgentTarget};
use pushos_domain::ids::{AgentId, ProviderName, SessionId, WorkspaceId};
use pushos_domain::ports::{AgentBackend, AgentError, SessionRequest};

/// The agent roles configured, and the backends that can fill them.
#[derive(Debug, Default)]
pub struct AgentRoster {
    definitions: HashMap<AgentId, AgentDefinition>,
    backends: HashMap<ProviderName, Arc<dyn AgentBackend>>,
    order: Vec<ProviderName>,
}

impl AgentRoster {
    /// Builds an empty roster.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a role.
    pub fn define(&mut self, definition: AgentDefinition) {
        self.definitions.insert(definition.id.clone(), definition);
    }

    /// Installs a backend, which may then fill roles that accept its provider.
    ///
    /// Registration order is the tie-break when a role expresses no preference,
    /// so the first backend installed is the default.
    pub fn install(&mut self, backend: Arc<dyn AgentBackend>) {
        let provider = backend.provider();
        if self.backends.insert(provider.clone(), backend).is_none() {
            self.order.push(provider);
        }
    }

    /// The role with this identity.
    pub fn definition(&self, agent: &AgentId) -> Option<&AgentDefinition> {
        self.definitions.get(agent)
    }

    /// Every configured role, by name.
    pub fn roles(&self) -> Vec<&AgentDefinition> {
        let mut roles: Vec<_> = self.definitions.values().collect();
        roles.sort_by(|a, b| a.id.cmp(&b.id));
        roles
    }

    /// The providers installed, in registration order.
    pub fn providers(&self) -> &[ProviderName] {
        &self.order
    }

    /// The backend that should fill a role.
    ///
    /// Takes the role's first preference that is actually installed, so an
    /// operator can name a provider they have not set up yet without that
    /// stopping the role from working.
    pub fn backend_for(&self, definition: &AgentDefinition) -> Option<&Arc<dyn AgentBackend>> {
        definition
            .preferred
            .iter()
            .find_map(|provider| self.backends.get(provider))
            .or_else(|| {
                self.order
                    .iter()
                    .filter(|provider| definition.accepts(provider))
                    .find_map(|provider| self.backends.get(provider))
            })
    }

    /// The backend running a session.
    pub fn backend_named(&self, provider: &ProviderName) -> Option<&Arc<dyn AgentBackend>> {
        self.backends.get(provider)
    }
}

/// What a target resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// A session that already exists.
    Existing(SessionId),
    /// No session exists; one should be opened for this role.
    Start(Box<StartRequest>),
}

/// What to open, and with which backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRequest {
    /// The provider that will run it.
    pub provider: ProviderName,
    /// What to ask the backend for.
    pub request: SessionRequest,
}

/// Why a target could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    /// The binding named a role that is not configured.
    #[error("no agent named `{agent}` is configured")]
    UnknownRole {
        /// The role that was named.
        agent: AgentId,
    },

    /// The role is configured but nothing can run it.
    #[error("no installed provider can fill the `{agent}` role")]
    NoProvider {
        /// The role that cannot be filled.
        agent: AgentId,
    },

    /// The binding named a session that is not open.
    #[error("session `{session}` is not open")]
    NoSuchSession {
        /// The session that was named.
        session: SessionId,
    },

    /// The binding asked for the selected session and there is none.
    #[error("nothing is selected; choose a session first")]
    NothingSelected,
}

impl From<RoutingError> for AgentError {
    fn from(error: RoutingError) -> Self {
        match error {
            RoutingError::NoSuchSession { session } => Self::NoSuchSession { session },
            other => Self::backend(
                other.to_string(),
                pushos_domain::error::ErrorClass::Validation,
                std::io::Error::other(other.to_string()),
            ),
        }
    }
}

/// Resolves targets against the roster and the sessions currently open.
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentRouter;

impl AgentRouter {
    /// Builds a router.
    pub const fn new() -> Self {
        Self
    }

    /// Works out which session a target means.
    ///
    /// Holds no state of its own: everything it needs is passed in, which is
    /// what makes routing decisions reproducible from a log.
    pub fn resolve(
        self,
        roster: &AgentRoster,
        sessions: &crate::SessionRegistry,
        target: &AgentTarget,
        workspace_root: &std::path::Path,
    ) -> Result<Resolution, RoutingError> {
        match target {
            AgentTarget::Session(session) => {
                if sessions.get(session).is_some() {
                    Ok(Resolution::Existing(session.clone()))
                } else {
                    Err(RoutingError::NoSuchSession {
                        session: session.clone(),
                    })
                }
            }

            AgentTarget::Selected => sessions
                .selected()
                .map(|session| Resolution::Existing(session.id.clone()))
                .ok_or(RoutingError::NothingSelected),

            AgentTarget::Role { agent, workspace } => {
                Self::resolve_role(roster, sessions, agent, workspace.as_ref(), workspace_root)
            }
        }
    }

    fn resolve_role(
        roster: &AgentRoster,
        sessions: &crate::SessionRegistry,
        agent: &AgentId,
        workspace: Option<&WorkspaceId>,
        workspace_root: &std::path::Path,
    ) -> Result<Resolution, RoutingError> {
        // An existing session wins. Starting a second builder because the first
        // was busy is how an operator ends up with six of them.
        if let Some(session) = sessions.filling(agent, workspace) {
            return Ok(Resolution::Existing(session.id.clone()));
        }

        let definition = roster
            .definition(agent)
            .ok_or_else(|| RoutingError::UnknownRole {
                agent: agent.clone(),
            })?;
        let backend = roster
            .backend_for(definition)
            .ok_or_else(|| RoutingError::NoProvider {
                agent: agent.clone(),
            })?;

        Ok(Resolution::Start(Box::new(StartRequest {
            provider: backend.provider(),
            request: SessionRequest {
                agent: agent.clone(),
                workspace: workspace.cloned(),
                cwd: workspace_root.to_path_buf(),
                objective: definition.objective.clone(),
            },
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use async_trait::async_trait;
    use pushos_domain::ports::{AgentCapabilities, ApprovalId, SessionHandle};

    use super::*;
    use crate::SessionRegistry;

    #[derive(Debug)]
    struct Stub(&'static str);

    #[async_trait]
    impl AgentBackend for Stub {
        fn provider(&self) -> ProviderName {
            ProviderName::new(self.0)
        }
        async fn capabilities(&self) -> Result<AgentCapabilities, AgentError> {
            Ok(AgentCapabilities::minimal())
        }
        async fn start(&self, request: SessionRequest) -> Result<SessionHandle, AgentError> {
            Ok(SessionHandle {
                id: SessionId::new(request.agent.as_str()),
                provider_session: None,
                provider: self.provider(),
            })
        }
        async fn resume(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
            Ok(())
        }
        async fn prompt(&self, _handle: &SessionHandle, _text: &str) -> Result<(), AgentError> {
            Ok(())
        }
        async fn cancel(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
            Ok(())
        }
        async fn answer(
            &self,
            _handle: &SessionHandle,
            _request: &ApprovalId,
            _option: &str,
        ) -> Result<(), AgentError> {
            Ok(())
        }
        async fn stop(&self, _handle: &SessionHandle) -> Result<(), AgentError> {
            Ok(())
        }
    }

    fn roster(preferred: &[&str], installed: &[&'static str]) -> AgentRoster {
        let mut roster = AgentRoster::new();
        let mut builder = AgentDefinition::new("builder", "Builder");
        builder.objective = "Build things".to_owned();
        builder.preferred = preferred
            .iter()
            .map(|name| ProviderName::new(*name))
            .collect();
        roster.define(builder);

        for name in installed {
            roster.install(Arc::new(Stub(name)));
        }
        roster
    }

    fn resolve(
        roster: &AgentRoster,
        sessions: &SessionRegistry,
        target: &AgentTarget,
    ) -> Result<Resolution, RoutingError> {
        AgentRouter::new().resolve(roster, sessions, target, Path::new("/tmp/project"))
    }

    #[test]
    fn a_role_with_no_session_asks_for_one_to_be_started() {
        let roster = roster(&[], &["claude"]);
        let sessions = SessionRegistry::new();

        let Resolution::Start(start) =
            resolve(&roster, &sessions, &AgentTarget::role("builder")).expect("resolvable")
        else {
            panic!("expected a start request");
        };

        assert_eq!(start.provider.as_str(), "claude");
        assert_eq!(start.request.objective, "Build things");
        assert_eq!(start.request.cwd, Path::new("/tmp/project"));
    }

    #[test]
    fn a_role_with_a_live_session_reuses_it_rather_than_starting_another() {
        let roster = roster(&[], &["claude"]);
        let mut sessions = SessionRegistry::new();
        sessions.opened(
            &SessionHandle {
                id: SessionId::new("s1"),
                provider_session: None,
                provider: ProviderName::new("claude"),
            },
            AgentId::new("builder"),
            None,
            Instant::now(),
        );

        assert_eq!(
            resolve(&roster, &sessions, &AgentTarget::role("builder")).expect("resolvable"),
            Resolution::Existing(SessionId::new("s1"))
        );
    }

    #[test]
    fn a_role_prefers_the_provider_it_names() {
        let roster = roster(&["codex"], &["claude", "codex"]);
        let Resolution::Start(start) = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::role("builder"),
        )
        .expect("resolvable") else {
            panic!("expected a start request");
        };
        assert_eq!(start.provider.as_str(), "codex");
    }

    #[test]
    fn a_preference_that_is_not_installed_falls_back_rather_than_failing() {
        // Naming a provider you have not set up yet should not stop the role
        // from working with one you have.
        let roster = roster(&["cursor"], &["claude"]);
        let result = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::role("builder"),
        );
        assert!(matches!(result, Err(RoutingError::NoProvider { .. })));
    }

    #[test]
    fn a_role_expressing_no_preference_takes_the_first_provider_installed() {
        let roster = roster(&[], &["codex", "claude"]);
        let Resolution::Start(start) = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::role("builder"),
        )
        .expect("resolvable") else {
            panic!("expected a start request");
        };
        assert_eq!(start.provider.as_str(), "codex");
    }

    #[test]
    fn an_unconfigured_role_is_named_in_the_failure() {
        let roster = roster(&[], &["claude"]);
        let error = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::role("nobody"),
        )
        .expect_err("no such role");
        assert!(error.to_string().contains("nobody"));
    }

    #[test]
    fn a_role_with_no_installed_provider_says_so() {
        let roster = roster(&[], &[]);
        let error = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::role("builder"),
        )
        .expect_err("nothing can run it");
        assert!(matches!(error, RoutingError::NoProvider { .. }));
    }

    #[test]
    fn an_exact_session_target_must_actually_exist() {
        let roster = roster(&[], &["claude"]);
        let error = resolve(
            &roster,
            &SessionRegistry::new(),
            &AgentTarget::Session(SessionId::new("gone")),
        )
        .expect_err("that session is not open");
        assert!(matches!(error, RoutingError::NoSuchSession { .. }));
    }

    #[test]
    fn asking_for_the_selection_when_nothing_is_selected_says_so() {
        let roster = roster(&[], &["claude"]);
        let error = resolve(&roster, &SessionRegistry::new(), &AgentTarget::Selected)
            .expect_err("nothing selected");
        assert!(matches!(error, RoutingError::NothingSelected));
    }

    #[test]
    fn two_workspaces_get_their_own_session_for_the_same_role() {
        let roster = roster(&[], &["claude"]);
        let mut sessions = SessionRegistry::new();
        sessions.opened(
            &SessionHandle {
                id: SessionId::new("syd"),
                provider_session: None,
                provider: ProviderName::new("claude"),
            },
            AgentId::new("builder"),
            Some(WorkspaceId::new("sydclaw")),
            Instant::now(),
        );

        let elsewhere = AgentTarget::role("builder").in_workspace("other");
        assert!(
            matches!(
                resolve(&roster, &sessions, &elsewhere).expect("resolvable"),
                Resolution::Start(_)
            ),
            "another project needs its own builder"
        );
    }
}
