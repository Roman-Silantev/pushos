//! Every session PushOS knows about.
//!
//! One owner, mutated through it, read as snapshots. Sessions come and go while
//! the surface is being drawn and while actions are running, so nothing else
//! holds a reference to one.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Instant;

use pushos_domain::agent::AgentState;
use pushos_domain::ids::{AgentId, SessionId, WorkspaceId};
use pushos_domain::ports::{AgentEvent, SessionHandle};
use tracing::debug;

use crate::session::{PendingApproval, Session};

/// How many finished sessions are kept per role.
///
/// Enough to answer "how did that go", bounded so a long day does not grow
/// without limit.
const KEPT_FINISHED: usize = 3;

/// Every session, live and recently finished.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    sessions: HashMap<SessionId, Session>,
    /// The session the operator most recently chose, which unqualified actions
    /// act on.
    selected: Option<SessionId>,
}

impl SessionRegistry {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a session that has just opened, and selects it.
    ///
    /// Selecting on open is what makes "start the builder, then talk to it" two
    /// gestures rather than three.
    pub fn opened(
        &mut self,
        handle: &SessionHandle,
        agent: AgentId,
        workspace: Option<WorkspaceId>,
        now: Instant,
    ) -> Session {
        let session = Session::opened(handle, agent, workspace, now);
        self.selected = Some(session.id.clone());
        self.sessions.insert(session.id.clone(), session.clone());
        self.forget_stale();
        session
    }

    /// Applies something a session did.
    ///
    /// Returns the session as it now stands, or `None` when the event names a
    /// session that has already been forgotten.
    pub fn apply(
        &mut self,
        session: &SessionId,
        event: &AgentEvent,
        now: Instant,
    ) -> Option<&Session> {
        let entry = self.sessions.get_mut(session)?;
        entry.last_activity = now;

        match event {
            AgentEvent::StateChanged { state } => {
                entry.state = *state;
                // Leaving a waiting state means the question is settled, however
                // it was settled; a stale question on the display would invite
                // an answer that goes nowhere.
                if !state.needs_operator() {
                    entry.pending_approval = None;
                }
            }
            AgentEvent::Said { text } => {
                entry.last_message = Some(text.clone());
            }
            AgentEvent::UsedTool { title, finished } => {
                if !finished {
                    entry.last_message = Some(title.clone());
                }
            }
            AgentEvent::AskedPermission {
                request,
                question,
                options,
            } => {
                entry.state = AgentState::WaitingApproval;
                entry.pending_approval = Some(PendingApproval {
                    request: request.clone(),
                    question: question.clone(),
                    options: options.clone(),
                });
            }
            AgentEvent::Ended { reason } => {
                entry.state = reason.resulting_state();
                entry.pending_approval = None;
            }
        }

        Some(&self.sessions[session])
    }

    /// The session with this identity.
    pub fn get(&self, session: &SessionId) -> Option<&Session> {
        self.sessions.get(session)
    }

    /// The session the operator most recently chose.
    pub fn selected(&self) -> Option<&Session> {
        self.selected.as_ref().and_then(|id| self.sessions.get(id))
    }

    /// Chooses a session, if it exists.
    pub fn select(&mut self, session: &SessionId) -> bool {
        if self.sessions.contains_key(session) {
            self.selected = Some(session.clone());
            return true;
        }
        false
    }

    /// The live session filling a role, if one is.
    ///
    /// The most recently active wins, so asking twice in a row gets the same
    /// answer even when an old session for the role is still lingering.
    pub fn filling(&self, agent: &AgentId, workspace: Option<&WorkspaceId>) -> Option<&Session> {
        self.sessions
            .values()
            .filter(|session| session.is_live() && session.fills(agent, workspace))
            .max_by_key(|session| session.last_activity)
    }

    /// Every session, most recently active first.
    pub fn all(&self) -> Vec<&Session> {
        let mut sessions: Vec<_> = self.sessions.values().collect();
        sessions.sort_by_key(|session| Reverse(session.last_activity));
        sessions
    }

    /// Every live session.
    pub fn live(&self) -> Vec<&Session> {
        self.all()
            .into_iter()
            .filter(|session| session.is_live())
            .collect()
    }

    /// Every session waiting on the operator.
    ///
    /// What the surface interrupts for. An agent working quietly earns nothing;
    /// an agent that has stopped and is waiting earns attention.
    pub fn waiting(&self) -> Vec<&Session> {
        self.all()
            .into_iter()
            .filter(|session| session.state.needs_operator())
            .collect()
    }

    /// How many sessions are held.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Forgets a session entirely.
    pub fn remove(&mut self, session: &SessionId) -> Option<Session> {
        if self.selected.as_ref() == Some(session) {
            self.selected = None;
        }
        self.sessions.remove(session)
    }

    /// Drops the oldest finished sessions for each role, keeping a few.
    fn forget_stale(&mut self) {
        let mut finished: Vec<_> = self
            .sessions
            .values()
            .filter(|session| !session.is_live())
            .map(|session| {
                (
                    session.agent.clone(),
                    session.id.clone(),
                    session.last_activity,
                )
            })
            .collect();
        finished.sort_by_key(|(_, _, activity)| Reverse(*activity));

        let mut kept: HashMap<AgentId, usize> = HashMap::new();
        for (agent, id, _) in finished {
            let seen = kept.entry(agent).or_insert(0);
            *seen += 1;
            if *seen > KEPT_FINISHED {
                debug!(%id, "forgetting an old finished session");
                self.sessions.remove(&id);
                if self.selected.as_ref() == Some(&id) {
                    self.selected = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pushos_domain::ids::ProviderName;
    use pushos_domain::ports::{ApprovalId, ApprovalOption, StopReason};

    use super::*;

    fn handle(id: &str) -> SessionHandle {
        SessionHandle {
            id: SessionId::new(id),
            provider_session: Some(format!("provider-{id}")),
            provider: ProviderName::new("claude"),
        }
    }

    fn open(registry: &mut SessionRegistry, id: &str, agent: &str, workspace: Option<&str>) {
        registry.opened(
            &handle(id),
            AgentId::new(agent),
            workspace.map(WorkspaceId::new),
            Instant::now(),
        );
    }

    #[test]
    fn opening_a_session_selects_it() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);

        assert_eq!(
            registry.selected().map(|s| s.id.to_string()),
            Some("s1".to_owned())
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_role_resolves_to_its_live_session() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", Some("sydclaw"));
        open(&mut registry, "s2", "reviewer", Some("sydclaw"));

        let builder = registry
            .filling(&AgentId::new("builder"), Some(&WorkspaceId::new("sydclaw")))
            .expect("the builder is live");
        assert_eq!(builder.id.as_str(), "s1");

        assert!(
            registry
                .filling(&AgentId::new("builder"), Some(&WorkspaceId::new("other")))
                .is_none(),
            "another workspace has no builder"
        );
    }

    #[test]
    fn a_finished_session_no_longer_fills_its_role() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);

        registry.apply(
            &SessionId::new("s1"),
            &AgentEvent::Ended {
                reason: StopReason::Completed,
            },
            Instant::now(),
        );

        assert!(registry.filling(&AgentId::new("builder"), None).is_none());
        assert_eq!(registry.len(), 1, "it is kept so its outcome can be read");
    }

    #[test]
    fn three_sessions_run_side_by_side_without_interfering() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "architect", Some("sydclaw"));
        open(&mut registry, "s2", "builder", Some("sydclaw"));
        open(&mut registry, "s3", "reviewer", Some("sydclaw"));

        let now = Instant::now();
        registry.apply(
            &SessionId::new("s1"),
            &AgentEvent::StateChanged {
                state: AgentState::Working,
            },
            now,
        );
        registry.apply(
            &SessionId::new("s2"),
            &AgentEvent::StateChanged {
                state: AgentState::WaitingApproval,
            },
            now,
        );
        registry.apply(
            &SessionId::new("s3"),
            &AgentEvent::Ended {
                reason: StopReason::Completed,
            },
            now,
        );

        assert_eq!(registry.live().len(), 2);
        assert_eq!(registry.waiting().len(), 1);
        assert_eq!(
            registry.get(&SessionId::new("s1")).map(|s| s.state),
            Some(AgentState::Working)
        );
        assert_eq!(
            registry.get(&SessionId::new("s3")).map(|s| s.state),
            Some(AgentState::Completed)
        );
    }

    #[test]
    fn a_permission_question_puts_the_session_in_a_waiting_state() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);

        registry.apply(
            &SessionId::new("s1"),
            &AgentEvent::AskedPermission {
                request: ApprovalId("r1".to_owned()),
                question: "run `cargo test`?".to_owned(),
                options: vec![ApprovalOption {
                    id: "yes".to_owned(),
                    label: "Allow".to_owned(),
                    allows: true,
                }],
            },
            Instant::now(),
        );

        let session = registry.get(&SessionId::new("s1")).expect("still there");
        assert_eq!(session.state, AgentState::WaitingApproval);
        assert!(session.pending_approval.is_some());
        assert_eq!(registry.waiting().len(), 1);
    }

    #[test]
    fn leaving_a_waiting_state_clears_the_question() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);
        let id = SessionId::new("s1");

        registry.apply(
            &id,
            &AgentEvent::AskedPermission {
                request: ApprovalId("r1".to_owned()),
                question: "?".to_owned(),
                options: Vec::new(),
            },
            Instant::now(),
        );
        registry.apply(
            &id,
            &AgentEvent::StateChanged {
                state: AgentState::Working,
            },
            Instant::now(),
        );

        let session = registry.get(&id).expect("still there");
        assert!(
            session.pending_approval.is_none(),
            "a settled question must not stay on the display"
        );
    }

    #[test]
    fn an_event_for_a_forgotten_session_is_ignored_rather_than_resurrecting_it() {
        let mut registry = SessionRegistry::new();
        assert!(
            registry
                .apply(
                    &SessionId::new("gone"),
                    &AgentEvent::StateChanged {
                        state: AgentState::Working
                    },
                    Instant::now(),
                )
                .is_none()
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn the_most_recently_active_session_answers_for_a_role() {
        let mut registry = SessionRegistry::new();
        let early = Instant::now();
        registry.opened(&handle("old"), AgentId::new("builder"), None, early);
        registry.opened(
            &handle("new"),
            AgentId::new("builder"),
            None,
            early + Duration::from_secs(10),
        );

        assert_eq!(
            registry
                .filling(&AgentId::new("builder"), None)
                .map(|s| s.id.to_string()),
            Some("new".to_owned())
        );
    }

    #[test]
    fn finished_sessions_are_forgotten_once_enough_have_piled_up() {
        let mut registry = SessionRegistry::new();
        let start = Instant::now();

        for index in 0..8 {
            let id = format!("s{index}");
            registry.opened(
                &handle(&id),
                AgentId::new("builder"),
                None,
                start + Duration::from_secs(index),
            );
            registry.apply(
                &SessionId::new(&id),
                &AgentEvent::Ended {
                    reason: StopReason::Completed,
                },
                start + Duration::from_secs(index),
            );
        }

        // Opening one more prunes the pile.
        registry.opened(&handle("live"), AgentId::new("builder"), None, start);
        assert!(
            registry.len() <= KEPT_FINISHED + 1,
            "held {} sessions",
            registry.len()
        );
        assert!(registry.filling(&AgentId::new("builder"), None).is_some());
    }

    #[test]
    fn removing_a_selected_session_leaves_nothing_selected() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);

        registry.remove(&SessionId::new("s1"));
        assert!(registry.selected().is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn selecting_something_that_does_not_exist_changes_nothing() {
        let mut registry = SessionRegistry::new();
        open(&mut registry, "s1", "builder", None);

        assert!(!registry.select(&SessionId::new("nope")));
        assert_eq!(
            registry.selected().map(|s| s.id.to_string()),
            Some("s1".to_owned())
        );
    }
}
