//! Typed failures.
//!
//! Errors are classified, not stringified. The classification decides whether a
//! caller may retry, must ask the operator, or should tear a component down.

use crate::action::MissingParam;
use crate::ids::ProviderName;
use crate::permissions::PermissionDenied;

/// How a caller should react to a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// A transient fault. Retrying with backoff is reasonable.
    Retryable,
    /// The request was malformed. Retrying will fail identically.
    Validation,
    /// A capability was missing.
    Permission,
    /// Progress needs a decision from the operator.
    UserActionRequired,
    /// A subsystem is unhealthy and should be restarted or reported.
    ComponentFailure,
}

impl ErrorClass {
    /// Whether an automatic retry could plausibly succeed.
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Retryable)
    }
}

/// Why an action did not run, or did not finish.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ActionError {
    /// No provider claims the selector's namespace.
    #[error("no provider is registered for `{provider}`")]
    UnknownProvider {
        /// The unclaimed namespace.
        provider: ProviderName,
    },

    /// The provider exists but does not implement the verb.
    #[error("provider `{provider}` does not implement `{verb}`")]
    UnknownVerb {
        /// The provider that was asked.
        provider: ProviderName,
        /// The verb it rejected.
        verb: String,
    },

    /// A required parameter was absent or of the wrong type.
    #[error(transparent)]
    Parameter(#[from] MissingParam),

    /// A capability was not granted.
    #[error(transparent)]
    Permission(#[from] PermissionDenied),

    /// The action needs a decision before it can proceed.
    #[error("waiting on the operator: {reason}")]
    NeedsConfirmation {
        /// What the operator must decide.
        reason: String,
    },

    /// The action ran too long and was abandoned.
    #[error("`{selector}` exceeded its {timeout_ms}ms budget")]
    TimedOut {
        /// The action that overran.
        selector: String,
        /// The budget it exceeded.
        timeout_ms: u64,
    },

    /// The action was cancelled before finishing.
    #[error("cancelled")]
    Cancelled,

    /// The underlying system reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ActionError {
    /// Classifies the failure.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::UnknownProvider { .. } | Self::UnknownVerb { .. } | Self::Parameter(_) => {
                ErrorClass::Validation
            }
            Self::Permission(_) => ErrorClass::Permission,
            Self::NeedsConfirmation { .. } => ErrorClass::UserActionRequired,
            Self::TimedOut { .. } => ErrorClass::Retryable,
            Self::Cancelled => ErrorClass::ComponentFailure,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Wraps a backend fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }

    /// Whether an automatic retry could plausibly succeed.
    ///
    /// A caller must still check idempotency before acting on this.
    pub const fn is_retryable(&self) -> bool {
        self.class().is_retryable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Permission;

    #[test]
    fn validation_failures_are_never_retried() {
        let error = ActionError::UnknownProvider {
            provider: "nope".into(),
        };
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(!error.is_retryable());
    }

    #[test]
    fn permission_failures_keep_their_class_through_conversion() {
        let error: ActionError = PermissionDenied {
            permission: Permission::GitPush,
        }
        .into();
        assert_eq!(error.class(), ErrorClass::Permission);
        assert!(!error.is_retryable());
    }

    #[test]
    fn timeouts_are_retryable_but_cancellation_is_not() {
        let timeout = ActionError::TimedOut {
            selector: "shell.run".to_owned(),
            timeout_ms: 5_000,
        };
        assert!(timeout.is_retryable());
        assert!(!ActionError::Cancelled.is_retryable());
    }

    #[test]
    fn backend_faults_carry_their_source() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such binary");
        let error = ActionError::backend("launching cursor", ErrorClass::Validation, io);
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(std::error::Error::source(&error).is_some());
    }
}
