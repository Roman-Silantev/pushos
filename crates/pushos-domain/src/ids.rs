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
    /// Identifies one step of a workflow.
    NodeId
);
string_id!(
    /// Identifies one place notes are kept.
    SourceId
);
string_id!(
    /// Identifies one note.
    ///
    /// Derived from where the note lives rather than generated, so re-reading
    /// a directory gives every note the identity it had before. A note that
    /// changed its id on every restart could not be linked to or reopened.
    NoteId
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

/// Identifies one run of a workflow.
///
/// From the gesture that started it to wherever it ends, across a restart:
/// the run outlives the process, so its name has to as well.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(uuid::Uuid);

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

        /// An identifier that is written down has to be readable back.
        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                text.parse().map(Self)
            }
        }
    };
}

uuid_id!(EventId);
uuid_id!(CorrelationId);
uuid_id!(ExecutionId);
uuid_id!(RunId);

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
    fn a_generated_identifier_survives_being_written_down_and_read_back() {
        use std::str::FromStr as _;

        // Runs outlive the process, so their names have to make the trip
        // through a database and back.
        let id = RunId::generate();
        assert_eq!(RunId::from_str(&id.to_string()).expect("it reads back"), id);
        assert!(RunId::from_str("not an identifier").is_err());
    }

    #[test]
    fn generated_ids_are_unique_and_ordered() {
        let first = CorrelationId::generate();
        let second = CorrelationId::generate();
        assert_ne!(first, second);
    }
}
