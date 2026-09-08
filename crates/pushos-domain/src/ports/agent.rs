//! The agent backend port.
//!
//! One trait, implemented once per protocol rather than once per vendor. An
//! adapter translates a provider's own vocabulary into the normalised states
//! and events below before anything else in PushOS sees it, so nothing above
//! this line can come to depend on a particular model vendor.

use async_trait::async_trait;

use crate::agent::AgentState;
use crate::ids::{AgentId, ProviderName, SessionId, WorkspaceId};

/// What a provider can do.
///
/// Negotiated once when a backend is reached. A capability that is absent is
/// hidden rather than offered and refused, so the surface never advertises
/// something the provider will not do.
///
/// A row of flags is the right shape here: these are genuinely independent
/// answers from the provider, not a state with a name.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCapabilities {
    /// A session can be picked up again after PushOS restarts.
    pub resume: bool,
    /// Work in progress can be interrupted.
    pub cancel: bool,
    /// The provider asks before doing things, rather than just doing them.
    pub permissions: bool,
    /// The provider can run commands in a terminal it owns.
    pub terminal: bool,
    /// The provider can work in a directory of its own.
    pub worktrees: bool,
    /// The models this provider offers, when it offers a choice.
    pub models: Vec<String>,
}

impl AgentCapabilities {
    /// The capabilities of a provider that only accepts prompts.
    pub fn minimal() -> Self {
        Self::default()
    }

    /// Whether the operator can stop work once it has started.
    ///
    /// Worth knowing before binding a control to cancelling: a pad that does
    /// nothing is worse than no pad.
    pub const fn is_interruptible(&self) -> bool {
        self.cancel
    }
}

/// What to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRequest {
    /// The role the session fills.
    pub agent: AgentId,
    /// Where it works.
    pub workspace: Option<WorkspaceId>,
    /// The directory it works in.
    pub cwd: std::path::PathBuf,
    /// The role's standing instruction.
    pub objective: String,
}

/// A session the provider has opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionHandle {
    /// What PushOS calls it, which outlives the provider's own identifier.
    pub id: SessionId,
    /// What the provider calls it, needed to resume.
    pub provider_session: Option<String>,
    /// Which provider is running it.
    pub provider: ProviderName,
}

/// Something a session did, in normalised terms.
///
/// Deliberately exhaustive. This is an internal vocabulary, not a published
/// API, and adding a variant should force every consumer to decide what to do
/// with it rather than letting a wildcard drop it on the floor.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// The session moved to a new state.
    StateChanged {
        /// Where it is now.
        state: AgentState,
    },
    /// The session said something.
    ///
    /// Chunks arrive continuously while an agent works; a consumer may coalesce
    /// them, and PushOS shows only the most recent line.
    Said {
        /// What it said.
        text: String,
    },
    /// The session started or finished using a tool.
    UsedTool {
        /// What the tool is doing, in the provider's words.
        title: String,
        /// Whether it has finished.
        finished: bool,
    },
    /// The session is asking to be allowed to do something.
    ///
    /// Nothing proceeds until an answer is given, so this must never be lost.
    AskedPermission {
        /// Identifies the question when answering it.
        request: ApprovalId,
        /// What it wants to do.
        question: String,
        /// The answers it will accept, in the order to offer them.
        options: Vec<ApprovalOption>,
    },
    /// The session ended.
    Ended {
        /// Why.
        reason: StopReason,
    },
}

/// Identifies one outstanding permission question.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ApprovalId(pub String);

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One answer a session will accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalOption {
    /// What the provider calls this answer.
    pub id: String,
    /// What to show the operator.
    pub label: String,
    /// Whether choosing this lets the work continue.
    pub allows: bool,
}

/// Why a session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// It finished what it was asked to do.
    Completed,
    /// It was interrupted.
    Cancelled,
    /// It ran out of whatever it was budgeted.
    Exhausted,
    /// It was refused something it needed.
    Refused,
    /// Something went wrong.
    Failed,
}

impl StopReason {
    /// The state a session lands in when it stops for this reason.
    pub const fn resulting_state(self) -> AgentState {
        match self {
            Self::Completed => AgentState::Completed,
            Self::Cancelled => AgentState::Cancelled,
            Self::Exhausted | Self::Refused | Self::Failed => AgentState::Failed,
        }
    }
}

/// Receives everything a session does.
///
/// A plain callback rather than a stream, so the port needs no async runtime of
/// its own and an adapter can report from whatever thread it happens to be on.
pub trait AgentObserver: Send + Sync + std::fmt::Debug {
    /// Reports one thing a session did.
    ///
    /// Must not block: an adapter calls this from the thread reading the
    /// provider's output, and holding it up stalls the provider.
    fn observe(&self, session: &SessionId, event: AgentEvent);
}

/// Runs agent sessions for one protocol.
#[async_trait]
pub trait AgentBackend: Send + Sync + std::fmt::Debug {
    /// Which provider this speaks for.
    fn provider(&self) -> ProviderName;

    /// What this provider can do.
    async fn capabilities(&self) -> Result<AgentCapabilities, AgentError>;

    /// Opens a session.
    async fn start(&self, request: SessionRequest) -> Result<SessionHandle, AgentError>;

    /// Picks up a session the provider still remembers.
    async fn resume(&self, handle: &SessionHandle) -> Result<(), AgentError>;

    /// Sends the operator's words to a session.
    async fn prompt(&self, handle: &SessionHandle, text: &str) -> Result<(), AgentError>;

    /// Interrupts a session's current work.
    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError>;

    /// Answers an outstanding permission question.
    async fn answer(
        &self,
        handle: &SessionHandle,
        request: &ApprovalId,
        option: &str,
    ) -> Result<(), AgentError>;

    /// Ends a session and releases whatever it was holding.
    async fn stop(&self, handle: &SessionHandle) -> Result<(), AgentError>;
}

/// Why an agent operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// The provider is not installed or not reachable.
    #[error("`{provider}` is not available")]
    Unavailable {
        /// Which provider.
        provider: ProviderName,
    },

    /// The provider does not do this.
    #[error("`{provider}` does not support {capability}")]
    Unsupported {
        /// Which provider.
        provider: ProviderName,
        /// What was asked of it.
        capability: &'static str,
    },

    /// PushOS does not know that session.
    #[error("session `{session}` is not open")]
    NoSuchSession {
        /// The session that was named.
        session: SessionId,
    },

    /// The provider needs the operator to sign in.
    #[error("`{provider}` needs you to sign in first")]
    NeedsAuthentication {
        /// Which provider.
        provider: ProviderName,
    },

    /// The provider reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl AgentError {
    /// Classifies the failure.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::Unavailable { .. } => ErrorClass::ComponentFailure,
            Self::Unsupported { .. } | Self::NoSuchSession { .. } => ErrorClass::Validation,
            Self::NeedsAuthentication { .. } => ErrorClass::UserActionRequired,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Wraps a provider fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_provider_advertises_nothing_it_cannot_do() {
        let capabilities = AgentCapabilities::minimal();
        assert!(!capabilities.resume);
        assert!(!capabilities.is_interruptible());
        assert!(capabilities.models.is_empty());
    }

    #[test]
    fn every_stop_reason_lands_in_a_finished_state() {
        for reason in [
            StopReason::Completed,
            StopReason::Cancelled,
            StopReason::Exhausted,
            StopReason::Refused,
            StopReason::Failed,
        ] {
            assert!(
                reason.resulting_state().is_finished(),
                "{reason:?} left the session unfinished"
            );
        }
    }

    #[test]
    fn only_a_clean_finish_counts_as_completed() {
        assert_eq!(
            StopReason::Completed.resulting_state(),
            AgentState::Completed
        );
        assert_eq!(
            StopReason::Cancelled.resulting_state(),
            AgentState::Cancelled
        );
        assert_eq!(StopReason::Exhausted.resulting_state(), AgentState::Failed);
        assert_eq!(StopReason::Refused.resulting_state(), AgentState::Failed);
    }

    #[test]
    fn needing_a_sign_in_asks_the_operator_rather_than_retrying() {
        let error = AgentError::NeedsAuthentication {
            provider: ProviderName::new("claude"),
        };
        assert_eq!(error.class(), crate::error::ErrorClass::UserActionRequired);
        assert!(!error.class().is_retryable());
    }

    #[test]
    fn asking_a_provider_for_something_it_cannot_do_is_a_validation_error() {
        let error = AgentError::Unsupported {
            provider: ProviderName::new("codex"),
            capability: "resume",
        };
        assert_eq!(error.class(), crate::error::ErrorClass::Validation);
    }
}
