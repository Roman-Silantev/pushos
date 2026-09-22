//! What everything is doing, as one message to the surface.
//!
//! The display needs lines to draw; the pads that start roles need to know what
//! each role's agent is doing; and the pads that stand for places in the fleet
//! need to know who is standing in each. All three come from the same look at
//! the supervisors, so they travel together and can never disagree.

use pushos_agents::Session;
use pushos_domain::agent::AgentState;
use pushos_domain::ids::{AgentId, WorkspaceId};
use pushos_ui::SessionLine;

/// What everything is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Progress {
    /// The columns on the display.
    pub(crate) lines: Vec<SessionLine>,
    /// What each role's most recent session is doing, most recent first.
    pub(crate) roles: Vec<RoleActivity>,
    /// What the fleet is doing, in the order it opened.
    ///
    /// One entry per live agent session. Position is meaningful here in a way
    /// it is not in the other two: the nth entry is what the pad bound to
    /// `slot:n` stands for.
    pub(crate) fleet: Vec<AgentState>,
}

/// What the most recent session of one role is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoleActivity {
    /// The role.
    pub(crate) agent: AgentId,
    /// The project it works in, if it is pinned to one.
    pub(crate) workspace: Option<WorkspaceId>,
    /// What it is doing.
    pub(crate) state: AgentState,
}

/// What each place in the fleet is doing, in the order the sessions opened.
///
/// Ordered by when a session opened rather than by what it is doing, so an
/// agent never swaps pads with its neighbour while someone is reaching for it.
/// Started-at alone can tie, so the id breaks it and the order is total.
pub(crate) fn fleet_of(sessions: &[Session]) -> Vec<AgentState> {
    let mut ordered: Vec<&Session> = sessions.iter().filter(|s| s.is_live()).collect();
    ordered.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    ordered.into_iter().map(|session| session.state).collect()
}

impl RoleActivity {
    /// The roles the sessions fill, most recent session first.
    ///
    /// Finished sessions are kept, because a role whose last session failed
    /// should say so on its pad until it is started again.
    pub(crate) fn of(sessions: &[Session]) -> Vec<Self> {
        let mut ordered: Vec<&Session> = sessions.iter().collect();
        ordered.sort_by_key(|session| std::cmp::Reverse(session.last_activity));
        ordered
            .into_iter()
            .map(|session| Self {
                agent: session.agent.clone(),
                workspace: session.workspace.clone(),
                state: session.state,
            })
            .collect()
    }

    /// What a role is doing, in a project or wherever it runs.
    ///
    /// The same reading of a role as the one that decides which session a pad
    /// reaches: a session pinned to a project does not answer for the role in
    /// another.
    pub(crate) fn state_of(
        roles: &[Self],
        agent: &AgentId,
        workspace: Option<&WorkspaceId>,
    ) -> Option<AgentState> {
        roles
            .iter()
            .find(|role| {
                role.agent == *agent
                    && (workspace.is_none() || role.workspace.as_ref() == workspace)
            })
            .map(|role| role.state)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use pushos_domain::ids::{ProviderName, SessionId};

    use super::*;

    fn session(id: &str, agent: &str, state: AgentState, age: u64) -> Session {
        let started = Instant::now();
        let handle = pushos_domain::ports::SessionHandle {
            id: SessionId::new(id),
            provider_session: None,
            provider: ProviderName::new("claude"),
        };
        let mut session = Session::opened(&handle, AgentId::new(agent), None, started);
        session.state = state;
        session.last_activity = started + Duration::from_secs(100 - age);
        session
    }

    #[test]
    fn a_role_is_described_by_its_most_recent_session() {
        let roles = RoleActivity::of(&[
            session("old", "builder", AgentState::Failed, 50),
            session("new", "builder", AgentState::Working, 5),
        ]);
        assert_eq!(
            RoleActivity::state_of(&roles, &AgentId::new("builder"), None),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn a_role_nothing_has_filled_has_no_state() {
        let roles = RoleActivity::of(&[session("one", "builder", AgentState::Working, 1)]);
        assert_eq!(
            RoleActivity::state_of(&roles, &AgentId::new("reviewer"), None),
            None
        );
    }

    #[test]
    fn a_session_in_one_project_does_not_answer_for_the_role_in_another() {
        let mut pinned = session("one", "builder", AgentState::Working, 1);
        pinned.workspace = Some(WorkspaceId::new("sydclaw"));
        let roles = RoleActivity::of(&[pinned]);

        assert_eq!(
            RoleActivity::state_of(
                &roles,
                &AgentId::new("builder"),
                Some(&WorkspaceId::new("other"))
            ),
            None
        );
        assert_eq!(
            RoleActivity::state_of(
                &roles,
                &AgentId::new("builder"),
                Some(&WorkspaceId::new("sydclaw"))
            ),
            Some(AgentState::Working)
        );
    }
}
