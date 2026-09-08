//! The agent model.
//!
//! PushOS core knows nothing about Claude, Codex, Cursor or any other vendor.
//! It knows that an agent is a role, that a session is an instance of that role
//! doing work, and that both report progress in one normalised vocabulary.
//! Everything provider-specific is translated at the adapter boundary.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ProviderName, SessionId, WorkspaceId};
use crate::permissions::PermissionSet;

/// What an agent session is doing, in terms every provider maps onto.
///
/// Providers have their own vocabularies and change them between releases.
/// Bindings, lights and the display are written against this one, so a provider
/// renaming a state cannot ripple through the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Configured, with nothing running.
    Sleeping,
    /// Asked to work, waiting for a turn.
    Queued,
    /// Coming up.
    Starting,
    /// Working.
    Working,
    /// Blocked until the operator says something.
    WaitingInput,
    /// Blocked until the operator allows something.
    WaitingApproval,
    /// Deliberately held.
    Paused,
    /// Finished what it was asked to do.
    Completed,
    /// Stopped because something went wrong.
    Failed,
    /// Stopped because it was told to.
    Cancelled,
    /// The provider is not reachable.
    Offline,
}

impl AgentState {
    /// Every state, for exhaustive handling and documentation.
    pub const ALL: [Self; 11] = [
        Self::Sleeping,
        Self::Queued,
        Self::Starting,
        Self::Working,
        Self::WaitingInput,
        Self::WaitingApproval,
        Self::Paused,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Offline,
    ];

    /// The stable textual form used in configuration and logs.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Sleeping => "sleeping",
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Working => "working",
            Self::WaitingInput => "waiting_input",
            Self::WaitingApproval => "waiting_approval",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Offline => "offline",
        }
    }

    /// Whether the session is doing something right now.
    pub const fn is_busy(self) -> bool {
        matches!(self, Self::Queued | Self::Starting | Self::Working)
    }

    /// Whether the session is blocked on the operator.
    ///
    /// This is the state that earns an interruption. An agent working quietly
    /// deserves none; an agent waiting for a decision has stopped.
    pub const fn needs_operator(self) -> bool {
        matches!(self, Self::WaitingInput | Self::WaitingApproval)
    }

    /// Whether the session has stopped and will not resume on its own.
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether a prompt could be sent right now.
    ///
    /// Only an unreachable provider refuses one. A session that is starting or
    /// already working queues the prompt behind what it is doing, which is what
    /// an operator pressing a pad twice expects; refusing there would block the
    /// most common gesture on the surface.
    pub const fn accepts_prompt(self) -> bool {
        !matches!(self, Self::Offline)
    }

    /// The light that communicates this state.
    pub const fn status_color(self) -> crate::color::StatusColor {
        use crate::color::StatusColor;
        match self {
            Self::Sleeping | Self::Cancelled => StatusColor::Idle,
            Self::Queued | Self::Starting | Self::Working => StatusColor::Working,
            Self::WaitingInput | Self::WaitingApproval | Self::Paused => StatusColor::Waiting,
            Self::Completed => StatusColor::Complete,
            Self::Failed => StatusColor::Failed,
            Self::Offline => StatusColor::Unassigned,
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// An agent role, as the operator configured it.
///
/// A role, not a process: "reviewer" is a job that some provider does, and
/// which provider does it can change without any binding changing.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentDefinition {
    /// Stable identity, referenced by bindings and workspaces.
    pub id: AgentId,
    /// Name shown on the display.
    pub name: String,
    /// What the role is for, given to the provider as its standing instruction.
    pub objective: String,
    /// Which providers may take this role, best first.
    pub preferred: Vec<ProviderName>,
    /// What this role is allowed to do.
    ///
    /// Applied on top of whatever the provider itself permits: the stricter of
    /// the two wins, so a role can narrow a provider but never widen it.
    pub permissions: PermissionSet,
}

impl AgentDefinition {
    /// Builds a definition with no permissions granted.
    pub fn new(id: impl Into<AgentId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            objective: String::new(),
            preferred: Vec::new(),
            permissions: PermissionSet::empty(),
        }
    }

    /// Whether a provider may take this role.
    ///
    /// A definition naming no providers accepts any, so an operator who does
    /// not care need not say so.
    pub fn accepts(&self, provider: &ProviderName) -> bool {
        self.preferred.is_empty() || self.preferred.contains(provider)
    }
}

/// Which session an action is aimed at.
///
/// Bindings should normally name a role in a workspace rather than a session
/// identifier, because a provider's session will not outlive the day and a pad
/// is expected to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTarget {
    /// The session currently filling a role, starting one if none is.
    Role {
        /// The role to fill.
        agent: AgentId,
        /// Where it works, when the operator pinned it to somewhere.
        workspace: Option<WorkspaceId>,
    },
    /// One specific session, by identifier.
    ///
    /// Exact and fragile: useful for a temporary layout, wrong as a default.
    Session(SessionId),
    /// Whichever session the operator most recently selected.
    Selected,
}

impl AgentTarget {
    /// Aims at a role.
    pub fn role(agent: impl Into<AgentId>) -> Self {
        Self::Role {
            agent: agent.into(),
            workspace: None,
        }
    }

    /// Narrows a role to one workspace.
    #[must_use]
    pub fn in_workspace(self, workspace: impl Into<WorkspaceId>) -> Self {
        match self {
            Self::Role { agent, .. } => Self::Role {
                agent,
                workspace: Some(workspace.into()),
            },
            other => other,
        }
    }

    /// Whether this target names a session that may not exist tomorrow.
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Session(_))
    }
}

impl std::str::FromStr for AgentTarget {
    type Err = MalformedTarget;

    /// Reads the form a binding writes:
    ///
    /// ```text
    /// role:builder
    /// workspace:sydclaw/role:builder
    /// session:abc123
    /// selected
    /// ```
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() || text == "selected" {
            return Ok(Self::Selected);
        }

        let mut agent = None;
        let mut workspace = None;
        let mut session = None;

        for part in text.split('/') {
            let (key, value) = part
                .split_once(':')
                .ok_or_else(|| MalformedTarget(text.to_owned()))?;
            let value = value.trim();
            if value.is_empty() {
                return Err(MalformedTarget(text.to_owned()));
            }

            match key.trim() {
                "role" | "agent" => agent = Some(AgentId::new(value)),
                "workspace" => workspace = Some(WorkspaceId::new(value)),
                "session" => session = Some(SessionId::new(value)),
                _ => return Err(MalformedTarget(text.to_owned())),
            }
        }

        // A session identifier is exact and cannot be combined with a role: the
        // two would disagree the moment the session ended.
        match (session, agent) {
            (Some(session), None) => Ok(Self::Session(session)),
            (None, Some(agent)) => Ok(Self::Role { agent, workspace }),
            // Naming both, or neither, says nothing PushOS can act on.
            _ => Err(MalformedTarget(text.to_owned())),
        }
    }
}

/// A target that is not written in a form PushOS understands.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a target; write `role:builder`, `workspace:x/role:builder`, \
     `session:abc` or `selected`"
)]
pub struct MalformedTarget(pub String);

impl fmt::Display for AgentTarget {
    /// Writes the same form the parser reads, so anything PushOS prints in a
    /// log or on the display can be pasted straight back into a binding.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Role {
                agent,
                workspace: Some(workspace),
            } => {
                write!(f, "workspace:{workspace}/role:{agent}")
            }
            Self::Role {
                agent,
                workspace: None,
            } => write!(f, "role:{agent}"),
            Self::Session(session) => write!(f, "session:{session}"),
            Self::Selected => f.write_str("selected"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_a_unique_name_that_round_trips() {
        let mut slugs: Vec<_> = AgentState::ALL.iter().map(|state| state.slug()).collect();
        slugs.sort_unstable();
        let total = slugs.len();
        slugs.dedup();
        assert_eq!(total, slugs.len(), "state names must be unique");

        for state in AgentState::ALL {
            let encoded = serde_json::to_string(&state).expect("serialisable");
            assert_eq!(encoded, format!("\"{}\"", state.slug()));
        }
    }

    #[test]
    fn busy_finished_and_blocked_are_mutually_exclusive() {
        for state in AgentState::ALL {
            let flags = [state.is_busy(), state.needs_operator(), state.is_finished()];
            assert!(
                flags.iter().filter(|set| **set).count() <= 1,
                "{state} claims to be more than one of busy, blocked and finished"
            );
        }
    }

    #[test]
    fn only_waiting_states_earn_an_interruption() {
        assert!(AgentState::WaitingInput.needs_operator());
        assert!(AgentState::WaitingApproval.needs_operator());
        assert!(
            !AgentState::Working.needs_operator(),
            "quiet work must not interrupt"
        );
        assert!(!AgentState::Completed.needs_operator());
    }

    #[test]
    fn a_state_that_needs_the_operator_is_never_shown_as_idle() {
        use crate::color::StatusColor;
        for state in AgentState::ALL
            .into_iter()
            .filter(|state| state.needs_operator())
        {
            assert_eq!(state.status_color(), StatusColor::Waiting);
        }
    }

    #[test]
    fn only_an_unreachable_provider_refuses_a_prompt() {
        assert!(!AgentState::Offline.accepts_prompt());

        for state in AgentState::ALL
            .into_iter()
            .filter(|s| *s != AgentState::Offline)
        {
            assert!(
                state.accepts_prompt(),
                "{state} refused a prompt; the operator would have nothing to press"
            );
        }
    }

    #[test]
    fn a_definition_naming_no_providers_accepts_any() {
        let open = AgentDefinition::new("builder", "Builder");
        assert!(open.accepts(&ProviderName::new("claude")));
        assert!(open.accepts(&ProviderName::new("codex")));

        let mut fussy = AgentDefinition::new("reviewer", "Reviewer");
        fussy.preferred = vec![ProviderName::new("codex")];
        assert!(fussy.accepts(&ProviderName::new("codex")));
        assert!(!fussy.accepts(&ProviderName::new("claude")));
    }

    #[test]
    fn only_a_session_target_is_exact() {
        assert!(!AgentTarget::role("builder").is_exact());
        assert!(
            !AgentTarget::role("builder")
                .in_workspace("sydclaw")
                .is_exact()
        );
        assert!(!AgentTarget::Selected.is_exact());
        assert!(AgentTarget::Session(SessionId::new("abc123")).is_exact());
    }

    #[test]
    fn every_written_target_parses_back_to_what_it_says() {
        use std::str::FromStr as _;

        assert_eq!(
            AgentTarget::from_str("role:builder").expect("valid"),
            AgentTarget::role("builder")
        );
        assert_eq!(
            AgentTarget::from_str("workspace:sydclaw/role:builder").expect("valid"),
            AgentTarget::role("builder").in_workspace("sydclaw")
        );
        assert_eq!(
            AgentTarget::from_str("role:builder/workspace:sydclaw").expect("order is free"),
            AgentTarget::role("builder").in_workspace("sydclaw")
        );
        assert_eq!(
            AgentTarget::from_str("session:abc123").expect("valid"),
            AgentTarget::Session(SessionId::new("abc123"))
        );
        assert_eq!(
            AgentTarget::from_str("selected").expect("valid"),
            AgentTarget::Selected
        );
        assert_eq!(
            AgentTarget::from_str("").expect("empty means selected"),
            AgentTarget::Selected
        );
    }

    #[test]
    fn everything_a_target_renders_as_parses_back() {
        use std::str::FromStr as _;

        for target in [
            AgentTarget::role("builder"),
            AgentTarget::role("builder").in_workspace("sydclaw"),
            AgentTarget::Session(SessionId::new("abc")),
            AgentTarget::Selected,
        ] {
            let written = target.to_string();
            assert_eq!(
                AgentTarget::from_str(&written).expect("what PushOS prints must parse"),
                target
            );
        }
    }

    #[test]
    fn a_target_that_cannot_be_understood_is_rejected_rather_than_guessed_at() {
        use std::str::FromStr as _;

        for text in [
            "builder",
            "role:",
            "nonsense:builder",
            "session:abc/role:builder",
            "workspace:sydclaw",
        ] {
            assert!(
                AgentTarget::from_str(text).is_err(),
                "`{text}` should not have parsed"
            );
        }
    }

    #[test]
    fn a_target_reads_the_way_it_is_configured() {
        assert_eq!(
            AgentTarget::role("builder")
                .in_workspace("sydclaw")
                .to_string(),
            "workspace:sydclaw/role:builder"
        );
        assert_eq!(AgentTarget::role("builder").to_string(), "role:builder");
    }
}
