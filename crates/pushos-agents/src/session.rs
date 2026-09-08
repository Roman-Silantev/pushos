//! What PushOS knows about a live session.

use std::time::Instant;

use pushos_domain::agent::AgentState;
use pushos_domain::ids::{AgentId, ProviderName, SessionId, WorkspaceId};
use pushos_domain::ports::SessionHandle;

/// One session, and everything the surface needs to say about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// What PushOS calls it. Outlives the provider's own identifier.
    pub id: SessionId,
    /// The role it fills.
    pub agent: AgentId,
    /// Where it works.
    pub workspace: Option<WorkspaceId>,
    /// Which provider is running it.
    pub provider: ProviderName,
    /// What the provider calls it, needed to pick it up again.
    pub provider_session: Option<String>,
    /// What it is doing.
    pub state: AgentState,
    /// The last thing it said, for the display.
    pub last_message: Option<String>,
    /// An outstanding permission question, if it is waiting on one.
    pub pending_approval: Option<PendingApproval>,
    /// When it opened.
    pub started_at: Instant,
    /// When it last did anything.
    pub last_activity: Instant,
}

/// A permission question waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    /// Identifies the question when answering it.
    pub request: pushos_domain::ports::ApprovalId,
    /// What the session wants to do.
    pub question: String,
    /// The answers it will accept.
    pub options: Vec<pushos_domain::ports::ApprovalOption>,
}

impl PendingApproval {
    /// The option that lets the work continue, preferring the first offered.
    ///
    /// Used when the operator approves with a single gesture rather than
    /// choosing from the list.
    pub fn allowing(&self) -> Option<&pushos_domain::ports::ApprovalOption> {
        self.options.iter().find(|option| option.allows)
    }

    /// The option that refuses.
    pub fn refusing(&self) -> Option<&pushos_domain::ports::ApprovalOption> {
        self.options.iter().find(|option| !option.allows)
    }
}

impl Session {
    /// Records a session that has just opened.
    pub fn opened(
        handle: &SessionHandle,
        agent: AgentId,
        workspace: Option<WorkspaceId>,
        now: Instant,
    ) -> Self {
        Self {
            id: handle.id.clone(),
            agent,
            workspace,
            provider: handle.provider.clone(),
            provider_session: handle.provider_session.clone(),
            state: AgentState::Starting,
            last_message: None,
            pending_approval: None,
            started_at: now,
            last_activity: now,
        }
    }

    /// The handle a backend needs to act on this session.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            id: self.id.clone(),
            provider_session: self.provider_session.clone(),
            provider: self.provider.clone(),
        }
    }

    /// Whether this session fills a role, optionally in a given workspace.
    ///
    /// A session pinned to a workspace does not answer for a role asked about
    /// generally: two builders in two projects are two different jobs.
    pub fn fills(&self, agent: &AgentId, workspace: Option<&WorkspaceId>) -> bool {
        self.agent == *agent && (workspace.is_none() || self.workspace.as_ref() == workspace)
    }

    /// Whether this session could be handed work.
    ///
    /// A finished session is kept so its outcome can still be read, but it is
    /// not the answer to "who is the builder here".
    pub const fn is_live(&self) -> bool {
        !matches!(self.state, AgentState::Offline) && !self.state.is_finished()
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::{ApprovalId, ApprovalOption};

    use super::*;

    fn handle() -> SessionHandle {
        SessionHandle {
            id: SessionId::new("s1"),
            provider_session: Some("provider-abc".to_owned()),
            provider: ProviderName::new("claude"),
        }
    }

    fn session(workspace: Option<&str>) -> Session {
        Session::opened(
            &handle(),
            AgentId::new("builder"),
            workspace.map(WorkspaceId::new),
            Instant::now(),
        )
    }

    #[test]
    fn a_new_session_starts_rather_than_claiming_to_be_working() {
        let session = session(None);
        assert_eq!(session.state, AgentState::Starting);
        assert!(session.is_live());
        assert!(session.pending_approval.is_none());
    }

    #[test]
    fn a_handle_carries_what_the_provider_needs_to_find_it_again() {
        let session = session(None);
        let handle = session.handle();
        assert_eq!(handle.provider_session.as_deref(), Some("provider-abc"));
        assert_eq!(handle.provider.as_str(), "claude");
    }

    #[test]
    fn a_session_pinned_to_a_workspace_does_not_answer_for_the_role_generally() {
        let pinned = session(Some("sydclaw"));
        let builder = AgentId::new("builder");

        assert!(pinned.fills(&builder, Some(&WorkspaceId::new("sydclaw"))));
        assert!(!pinned.fills(&builder, Some(&WorkspaceId::new("other"))));
        // Asked about the role generally, a pinned session still answers: it is
        // a builder, and the caller did not say where.
        assert!(pinned.fills(&builder, None));
    }

    #[test]
    fn a_finished_session_is_kept_but_is_no_longer_the_one_to_hand_work_to() {
        let mut session = session(None);
        session.state = AgentState::Completed;
        assert!(!session.is_live());

        session.state = AgentState::Failed;
        assert!(!session.is_live());

        session.state = AgentState::WaitingApproval;
        assert!(
            session.is_live(),
            "a blocked session is still the one doing the job"
        );
    }

    #[test]
    fn an_approval_knows_which_answer_allows_and_which_refuses() {
        let approval = PendingApproval {
            request: ApprovalId("r1".to_owned()),
            question: "write to main.rs?".to_owned(),
            options: vec![
                ApprovalOption {
                    id: "no".to_owned(),
                    label: "Reject".to_owned(),
                    allows: false,
                },
                ApprovalOption {
                    id: "yes".to_owned(),
                    label: "Allow".to_owned(),
                    allows: true,
                },
            ],
        };

        assert_eq!(
            approval.allowing().map(|option| option.id.as_str()),
            Some("yes")
        );
        assert_eq!(
            approval.refusing().map(|option| option.id.as_str()),
            Some("no")
        );
    }
}
