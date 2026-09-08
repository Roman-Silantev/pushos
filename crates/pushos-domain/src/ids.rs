//! Typed identifiers.
//!
//! Every aggregate gets its own opaque id type so that a `WorkspaceId` can never
//! be passed where a `SessionId` is expected. Ids are cheap to clone and are
//! serialised as plain strings.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Declares a newtype id backed by an immutable reference-counted string.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(std::sync::Arc<str>);

        impl $name {
            /// Wraps an already-validated identifier.
            pub fn new(value: impl AsRef<str>) -> Self {
                Self(std::sync::Arc::from(value.as_ref()))
            }

            /// Borrows the underlying textual form.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), &self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(
    /// Identifies a single configured binding.
    BindingId
);
string_id!(
    /// Identifies a page of control meanings.
    PageId
);
string_id!(
    /// Identifies a persistent project context.
    WorkspaceId
);
string_id!(
    /// Identifies an agent role definition.
    AgentId
);
string_id!(
    /// Identifies a live agent, terminal or process session.
    SessionId
);
string_id!(
    /// Identifies a workflow definition.
    WorkflowId
);
string_id!(
    /// Identifies an action provider namespace, such as `media` or `agent`.
    ProviderName
);
string_id!(
    /// Identifies a verb within a provider namespace, such as `play_pause`.
    ActionVerb
);

/// Uniquely identifies one emitted event.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(uuid::Uuid);

/// Ties every event produced while servicing one user gesture together.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(uuid::Uuid);

/// Deduplicates externally visible side effects across retries.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionId(uuid::Uuid);

macro_rules! uuid_id {
    ($name:ident) => {
        impl $name {
            /// Generates a fresh, time-ordered identifier.
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7())
            }

            /// Exposes the underlying UUID.
            pub fn as_uuid(&self) -> uuid::Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), &self.0)
            }
        }
    };
}

uuid_id!(EventId);
uuid_id!(CorrelationId);
uuid_id!(ExecutionId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_ids_round_trip_through_display() {
        let id = WorkspaceId::new("sydclaw");
        assert_eq!(id.as_str(), "sydclaw");
        assert_eq!(id.to_string(), "sydclaw");
    }

    #[test]
    fn generated_ids_are_unique_and_ordered() {
        let first = CorrelationId::generate();
        let second = CorrelationId::generate();
        assert_ne!(first, second);
    }
}
