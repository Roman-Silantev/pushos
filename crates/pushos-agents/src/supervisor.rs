//! The one owner of agent sessions.
//!
//! Everything that starts, prompts, cancels or answers a session goes through
//! here, so the registry has a single writer and two pads pressed together
//! cannot race each other into two sessions for one role.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pushos_domain::agent::{AgentState, AgentTarget};
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{AgentError, AgentEvent, AgentObserver};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::registry::SessionRegistry;
use crate::router::{AgentRoster, AgentRouter, Resolution};
use crate::session::Session;

/// Runs agent operations and keeps the session registry.
#[derive(Debug)]
pub struct AgentSupervisor {
    roster: Arc<AgentRoster>,
    /// Held across an await deliberately: starting a session is the operation
    /// that must not interleave, or two pads pressed together open two builders.
    /// Every other operation is short.
    sessions: Mutex<SessionRegistry>,
    observer: Arc<dyn AgentObserver>,
    workspace_root: PathBuf,
}

impl AgentSupervisor {
    /// Builds a supervisor over a roster.
    pub fn new(
        roster: Arc<AgentRoster>,
        observer: Arc<dyn AgentObserver>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            roster,
            sessions: Mutex::new(SessionRegistry::new()),
            observer,
            workspace_root: workspace_root.into(),
        }
    }

    /// The roles and backends available.
    pub fn roster(&self) -> &Arc<AgentRoster> {
        &self.roster
    }

    /// Reads the sessions currently known.
    pub async fn sessions(&self) -> Vec<Session> {
        self.sessions
            .lock()
            .await
            .all()
            .into_iter()
            .cloned()
            .collect()
    }

    /// The session the operator most recently chose.
    pub async fn selected(&self) -> Option<Session> {
        self.sessions.lock().await.selected().cloned()
    }

    /// Chooses a session.
    pub async fn select(&self, session: &SessionId) -> bool {
        self.sessions.lock().await.select(session)
    }

    /// Applies something a session did.
    ///
    /// Called from the observer side, so an adapter reporting progress never
    /// has to know about the registry.
    pub async fn record(&self, session: &SessionId, event: &AgentEvent) -> Option<Session> {
        self.sessions
            .lock()
            .await
            .apply(session, event, Instant::now())
            .cloned()
    }

    /// Finds the session a target means, opening one if the role has none.
    pub async fn resolve(&self, target: &AgentTarget) -> Result<Session, AgentError> {
        let decision = {
            let sessions = self.sessions.lock().await;
            AgentRouter::new().resolve(&self.roster, &sessions, target, &self.workspace_root)?
        };

        match decision {
            Resolution::Existing(id) => self
                .sessions
                .lock()
                .await
                .get(&id)
                .cloned()
                .ok_or(AgentError::NoSuchSession { session: id }),

            Resolution::Start(start) => {
                let backend =
                    self.roster
                        .backend_named(&start.provider)
                        .ok_or(AgentError::Unavailable {
                            provider: start.provider.clone(),
                        })?;

                let agent = start.request.agent.clone();
                let workspace = start.request.workspace.clone();
                info!(%agent, provider = %start.provider, "starting an agent session");

                let handle = backend.start(start.request).await?;
                let session = {
                    let mut sessions = self.sessions.lock().await;
                    sessions.opened(&handle, agent, workspace, Instant::now())
                };

                // The provider will report its own progress; until it does, the
                // session is starting rather than pretending to work.
                self.observer.observe(
                    &session.id,
                    AgentEvent::StateChanged {
                        state: AgentState::Starting,
                    },
                );
                Ok(session)
            }
        }
    }

    /// Sends the operator's words to whichever session the target means.
    pub async fn prompt(&self, target: &AgentTarget, text: &str) -> Result<Session, AgentError> {
        let session = self.resolve(target).await?;
        if !session.state.accepts_prompt() {
            return Err(AgentError::Backend {
                context: format!(
                    "`{}` is {} and cannot take a prompt",
                    session.agent, session.state
                ),
                class: pushos_domain::error::ErrorClass::UserActionRequired,
                source: Box::new(std::io::Error::other("session is not ready")),
            });
        }

        self.backend_for(&session)?
            .prompt(&session.handle(), text)
            .await?;
        self.mark(&session, AgentState::Queued).await
    }

    /// Interrupts whatever the target is doing.
    pub async fn cancel(&self, target: &AgentTarget) -> Result<Session, AgentError> {
        let session = self.resolve(target).await?;
        let backend = self.backend_for(&session)?;

        if !backend.capabilities().await?.is_interruptible() {
            return Err(AgentError::Unsupported {
                provider: session.provider.clone(),
                capability: "cancel",
            });
        }

        backend.cancel(&session.handle()).await?;
        // The provider reports the actual stop; this only records that it was
        // asked, so the surface stops showing work that is being wound up.
        self.mark(&session, AgentState::Paused).await
    }

    /// Answers an outstanding permission question.
    ///
    /// Refuses when there is no question, rather than sending an answer the
    /// provider is not waiting for.
    pub async fn answer(&self, target: &AgentTarget, allow: bool) -> Result<Session, AgentError> {
        let session = self.resolve(target).await?;
        let Some(approval) = session.pending_approval.clone() else {
            return Err(AgentError::Backend {
                context: format!("`{}` is not waiting for a decision", session.agent),
                class: pushos_domain::error::ErrorClass::Validation,
                source: Box::new(std::io::Error::other("nothing to answer")),
            });
        };

        let option = if allow {
            approval.allowing()
        } else {
            approval.refusing()
        };
        let Some(option) = option else {
            return Err(AgentError::Backend {
                context: "the question offers no such answer".to_owned(),
                class: pushos_domain::error::ErrorClass::Validation,
                source: Box::new(std::io::Error::other("no matching option")),
            });
        };

        debug!(session = %session.id, answer = %option.id, "answering a permission question");
        self.backend_for(&session)?
            .answer(&session.handle(), &approval.request, &option.id)
            .await?;

        self.mark(
            &session,
            if allow {
                AgentState::Working
            } else {
                AgentState::Cancelled
            },
        )
        .await
    }

    /// Ends a session and forgets it.
    pub async fn stop(&self, target: &AgentTarget) -> Result<Session, AgentError> {
        let session = self.resolve(target).await?;
        if let Err(error) = self.backend_for(&session)?.stop(&session.handle()).await {
            // Forgetting it locally still matters: a session PushOS cannot
            // reach is not one it should keep offering to the operator.
            warn!(%error, session = %session.id, "the provider did not stop cleanly");
        }

        self.sessions.lock().await.remove(&session.id);
        Ok(session)
    }

    fn backend_for(
        &self,
        session: &Session,
    ) -> Result<&Arc<dyn pushos_domain::ports::AgentBackend>, AgentError> {
        self.roster
            .backend_named(&session.provider)
            .ok_or_else(|| AgentError::Unavailable {
                provider: session.provider.clone(),
            })
    }

    /// Records a state PushOS knows to be true, and tells everyone.
    async fn mark(&self, session: &Session, state: AgentState) -> Result<Session, AgentError> {
        let event = AgentEvent::StateChanged { state };
        let updated = self
            .sessions
            .lock()
            .await
            .apply(&session.id, &event, Instant::now())
            .cloned();

        self.observer.observe(&session.id, event);
        updated.ok_or_else(|| AgentError::NoSuchSession {
            session: session.id.clone(),
        })
    }
}

/// Reports session activity into a supervisor.
///
/// An adapter is handed one of these and calls it from whatever thread it reads
/// the provider on, without knowing anything about registries or event buses.
#[derive(Debug)]
pub struct SupervisorObserver {
    inner: Arc<dyn AgentObserver>,
}

impl SupervisorObserver {
    /// Wraps an observer.
    pub fn new(inner: Arc<dyn AgentObserver>) -> Self {
        Self { inner }
    }
}

impl AgentObserver for SupervisorObserver {
    fn observe(&self, session: &SessionId, event: AgentEvent) {
        self.inner.observe(session, event);
    }
}

/// Convenience for a supervisor that reports nowhere.
#[derive(Debug, Default)]
pub struct SilentObserver;

impl AgentObserver for SilentObserver {
    fn observe(&self, _session: &SessionId, _event: AgentEvent) {}
}
