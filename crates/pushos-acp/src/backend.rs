//! The backend PushOS binds to.
//!
//! Implements the domain's agent port in terms of the protocol. Everything
//! above it works in normalised states; everything below it is one agent
//! process per session.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::{ProviderName, SessionId};
use pushos_domain::ports::{
    AgentBackend, AgentCapabilities, AgentError, AgentObserver, ApprovalId, SessionHandle,
    SessionRequest,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::command::SessionCommand;
use crate::launcher::AgentCommand;
use crate::session::{self, RunningSession};

/// An agent provider reached over the Agent Client Protocol.
#[derive(Debug)]
pub struct AcpBackend {
    provider: ProviderName,
    command: AgentCommand,
    observer: Arc<dyn AgentObserver>,
    sessions: Mutex<HashMap<SessionId, RunningSession>>,
}

impl AcpBackend {
    /// Builds a backend that starts the given command for each session.
    pub fn new(
        provider: impl Into<ProviderName>,
        command: AgentCommand,
        observer: Arc<dyn AgentObserver>,
    ) -> Self {
        Self {
            provider: provider.into(),
            command,
            observer,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// The command this backend runs.
    pub const fn command(&self) -> &AgentCommand {
        &self.command
    }

    async fn running(&self, session: &SessionId) -> Result<RunningHandle, AgentError> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session)
            .map(|running| RunningHandle {
                commands: running.clone_sender(),
                pending: running.clone_pending(),
            })
            .ok_or_else(|| AgentError::NoSuchSession {
                session: session.clone(),
            })
    }

    fn failed(context: impl Into<String>, class: ErrorClass) -> AgentError {
        let context = context.into();
        AgentError::backend(context.clone(), class, std::io::Error::other(context))
    }
}

/// A borrowed view of a running session, so the registry lock is not held
/// across the await that talks to the agent.
struct RunningHandle {
    commands: tokio::sync::mpsc::Sender<SessionCommand>,
    pending: crate::session::PendingAnswers,
}

impl RunningHandle {
    async fn send(
        &self,
        build: impl FnOnce(tokio::sync::oneshot::Sender<Result<(), String>>) -> SessionCommand,
    ) -> Result<(), String> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| "the session has stopped".to_owned())?;
        answer
            .await
            .map_err(|_| "the session stopped before answering".to_owned())?
    }
}

#[async_trait]
impl AgentBackend for AcpBackend {
    fn provider(&self) -> ProviderName {
        self.provider.clone()
    }

    async fn capabilities(&self) -> Result<AgentCapabilities, AgentError> {
        // The protocol negotiates these per connection, and PushOS opens a
        // connection per session. These are what the protocol itself
        // guarantees; anything finer is read from the session when it opens.
        Ok(AgentCapabilities {
            resume: false,
            cancel: true,
            permissions: true,
            terminal: false,
            worktrees: false,
            models: Vec::new(),
        })
    }

    async fn start(&self, request: SessionRequest) -> Result<SessionHandle, AgentError> {
        if !self.command.is_runnable() {
            return Err(AgentError::Unavailable {
                provider: self.provider.clone(),
            });
        }

        let id = SessionId::new(format!(
            "{}-{}-{}",
            self.provider,
            request.agent,
            pushos_domain::ids::ExecutionId::generate()
        ));

        info!(
            provider = %self.provider,
            command = %self.command.describe(),
            session = %id,
            "starting an agent"
        );

        let running = session::launch(
            self.command.clone(),
            id.clone(),
            request.cwd,
            request.objective,
            Arc::clone(&self.observer),
        )
        .await
        .map_err(|reason| {
            // A provider that will not start is a component failure, not
            // something the operator did wrong.
            Self::failed(
                format!("`{}` would not start: {reason}", self.provider),
                ErrorClass::ComponentFailure,
            )
        })?;

        self.sessions.lock().await.insert(id.clone(), running);

        Ok(SessionHandle {
            id,
            // The protocol's own session identifier lives inside the task; it
            // is not durable across a restart, so PushOS does not pretend to
            // resume until the provider says it can.
            provider_session: None,
            provider: self.provider.clone(),
        })
    }

    async fn resume(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        Err(AgentError::Unsupported {
            provider: handle.provider.clone(),
            capability: "resume",
        })
    }

    async fn prompt(&self, handle: &SessionHandle, text: &str) -> Result<(), AgentError> {
        let running = self.running(&handle.id).await?;
        let text = text.to_owned();
        running
            .send(move |reply| SessionCommand::Prompt { text, reply })
            .await
            .map_err(|reason| Self::failed(reason, ErrorClass::ComponentFailure))
    }

    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let running = self.running(&handle.id).await?;
        running
            .send(|reply| SessionCommand::Cancel { reply })
            .await
            .map_err(|reason| Self::failed(reason, ErrorClass::ComponentFailure))
    }

    async fn answer(
        &self,
        handle: &SessionHandle,
        request: &ApprovalId,
        option: &str,
    ) -> Result<(), AgentError> {
        let running = self.running(&handle.id).await?;

        // The question is parked inside the session task waiting on a channel;
        // handing the answer straight to it is what unblocks the agent.
        if crate::session::deliver_answer(&running.pending, request, option) {
            return Ok(());
        }

        Err(Self::failed(
            "that question is no longer open".to_owned(),
            ErrorClass::Validation,
        ))
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let running = self.sessions.lock().await.remove(&handle.id);
        let Some(running) = running else {
            return Err(AgentError::NoSuchSession {
                session: handle.id.clone(),
            });
        };

        if let Err(reason) = running.send(|reply| SessionCommand::Stop { reply }).await {
            // The session is already forgotten locally, which is what matters:
            // a process PushOS cannot reach is not one to keep offering.
            warn!(session = %handle.id, reason, "the agent did not stop cleanly");
        }
        Ok(())
    }
}
