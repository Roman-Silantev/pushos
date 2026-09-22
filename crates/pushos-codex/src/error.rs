//! What can go wrong between PushOS and the app-server.
//!
//! Typed, and classified where the domain needs to know whether to retry, ask
//! the operator for something, or give up. A failure that arrives as a string
//! tells the surface nothing it can act on.

use std::sync::Arc;

use pushos_domain::error::ErrorClass;
use pushos_domain::ids::ProviderName;
use pushos_domain::ports::AgentError;

/// A failure talking to a Codex app-server.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CodexError {
    /// The program could not be started at all.
    #[error("`{program}` could not be started: {source}")]
    CouldNotStart {
        /// What PushOS tried to run.
        program: String,
        /// Why the operating system refused.
        source: Arc<std::io::Error>,
    },
    /// The child was started but gave PushOS no pipes to talk over.
    #[error("the app-server started without the pipes to talk over it")]
    NoPipe,
    /// The server is gone.
    #[error("the app-server has stopped")]
    Stopped,
    /// The server did not answer in time.
    #[error("the app-server did not answer `{method}` in time")]
    TimedOut {
        /// The call that was waiting.
        method: String,
    },
    /// The server refused the call.
    #[error("the app-server refused: {detail}")]
    Refused {
        /// What it said.
        detail: String,
    },
    /// Too many calls are already waiting.
    #[error("{in_flight} calls are already waiting on the app-server")]
    TooBusy {
        /// How many were outstanding.
        in_flight: usize,
    },
    /// The server's answer was not shaped the way the protocol says.
    #[error("the app-server said something PushOS could not read: {detail}")]
    Malformed {
        /// What was wrong with it.
        detail: String,
    },
}

impl CodexError {
    /// How the error boundary should treat this.
    pub const fn class(&self) -> ErrorClass {
        match self {
            // A busy or slow server is worth trying again; a stopped one needs
            // starting, which the supervisor does rather than the caller.
            Self::TooBusy { .. } | Self::TimedOut { .. } => ErrorClass::Retryable,
            Self::CouldNotStart { .. } | Self::NoPipe | Self::Stopped => {
                ErrorClass::ComponentFailure
            }
            Self::Refused { .. } | Self::Malformed { .. } => ErrorClass::ComponentFailure,
        }
    }

    /// Turns the failure into one the agent port speaks.
    pub fn into_agent_error(self, provider: &ProviderName) -> AgentError {
        if matches!(self, Self::CouldNotStart { .. }) {
            return AgentError::Unavailable {
                provider: provider.clone(),
            };
        }
        let class = self.class();
        let said = self.to_string();
        AgentError::backend(said.clone(), class, std::io::Error::other(said))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_or_slow_server_is_worth_trying_again() {
        assert_eq!(
            CodexError::TooBusy { in_flight: 256 }.class(),
            ErrorClass::Retryable
        );
        assert_eq!(
            CodexError::TimedOut {
                method: "turn/start".to_owned()
            }
            .class(),
            ErrorClass::Retryable
        );
    }

    #[test]
    fn a_server_that_will_not_start_is_reported_as_the_provider_being_unavailable() {
        let provider = ProviderName::new("codex");
        let error = CodexError::CouldNotStart {
            program: "codex".to_owned(),
            source: Arc::new(std::io::Error::other("not found")),
        }
        .into_agent_error(&provider);

        assert!(
            matches!(error, AgentError::Unavailable { provider: named } if named == provider),
            "a missing program is the provider being unavailable, not a backend fault"
        );
    }

    #[test]
    fn other_failures_keep_what_the_server_said() {
        let provider = ProviderName::new("codex");
        let error = CodexError::Refused {
            detail: "thread/items/list is not supported yet".to_owned(),
        }
        .into_agent_error(&provider);
        assert!(error.to_string().contains("not supported yet"));
    }
}
