//! Describing live work to a control client.
//!
//! Studio binds a control to a session, so it has to be able to see what is
//! running and, more importantly, how to name it. Each session is therefore
//! described twice: once exactly, and once by the role or name that will still
//! mean the right thing tomorrow.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_agents::AgentSupervisor;
use pushos_api::SessionSource;
use pushos_api::protocol::{SessionInfo, SessionKind};
use pushos_terminal::TerminalSupervisor;

/// The agent sessions, described for a control client.
#[derive(Debug)]
pub struct AgentSessions {
    supervisor: Arc<AgentSupervisor>,
}

impl AgentSessions {
    /// Describes the sessions a supervisor is running.
    pub const fn new(supervisor: Arc<AgentSupervisor>) -> Self {
        Self { supervisor }
    }
}

#[async_trait]
impl SessionSource for AgentSessions {
    async fn sessions(&self) -> Vec<SessionInfo> {
        let selected = self.supervisor.selected().await.map(|session| session.id);

        self.supervisor
            .sessions()
            .await
            .into_iter()
            .map(|session| {
                // A role, optionally pinned to a workspace: the form a binding
                // should use, because it survives this session ending.
                let standing = match &session.workspace {
                    Some(workspace) => format!("workspace:{workspace}/role:{}", session.agent),
                    None => format!("role:{}", session.agent),
                };

                SessionInfo {
                    id: session.id.to_string(),
                    kind: SessionKind::Agent,
                    name: session.agent.to_string(),
                    status: session.state.to_string(),
                    live: session.is_live(),
                    selected: selected.as_ref() == Some(&session.id),
                    target: format!("session:{}", session.id),
                    standing_target: Some(standing),
                    detail: session.last_message.clone(),
                    workspace: session.workspace.as_ref().map(ToString::to_string),
                }
            })
            .collect()
    }
}

/// The terminals, described for a control client.
#[derive(Debug)]
pub struct TerminalSessions {
    supervisor: Arc<TerminalSupervisor>,
}

impl TerminalSessions {
    /// Describes the terminals a supervisor is running.
    pub const fn new(supervisor: Arc<TerminalSupervisor>) -> Self {
        Self { supervisor }
    }
}

#[async_trait]
impl SessionSource for TerminalSessions {
    async fn sessions(&self) -> Vec<SessionInfo> {
        self.supervisor
            .summaries()
            .await
            .into_iter()
            .map(|terminal| SessionInfo {
                id: terminal.id.to_string(),
                kind: SessionKind::Terminal,
                name: terminal.name.clone(),
                status: crate::actors::describe_terminal(terminal.status).to_owned(),
                live: terminal.status.is_live(),
                selected: terminal.selected,
                target: format!("session:{}", terminal.id),
                standing_target: Some(format!("name:{}", terminal.name)),
                detail: terminal.last_line.clone(),
                workspace: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use pushos_terminal::OpenTerminal;
    use pushos_testkit::{FakeTerminal, RecordingTerminalObserver};

    use super::*;

    #[tokio::test]
    async fn a_terminal_is_described_both_exactly_and_by_name() {
        let observer = RecordingTerminalObserver::new();
        let host = FakeTerminal::new(Arc::new(observer));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(host),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        ));
        supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        let described = TerminalSessions::new(supervisor).sessions().await;
        assert_eq!(described.len(), 1);
        assert_eq!(described[0].kind, SessionKind::Terminal);
        assert_eq!(described[0].name, "tests");
        assert!(described[0].live);
        assert!(described[0].selected);
        assert_eq!(described[0].target, format!("session:{}", described[0].id));
        assert_eq!(described[0].standing_target.as_deref(), Some("name:tests"));
    }

    #[tokio::test]
    async fn a_terminal_that_finished_is_listed_but_not_live() {
        // Studio shows it so the operator can see how it went; binding to it
        // would be binding to something that has ended.
        let observer = RecordingTerminalObserver::new();
        let host = FakeTerminal::new(Arc::new(observer.clone()));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(host.clone()),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        ));
        let opened = supervisor
            .open(OpenTerminal::named("tests"))
            .await
            .expect("the fake succeeds by default");

        host.finish(&opened.id, Some(1));
        for (id, event) in observer.events() {
            supervisor.apply(&id, &event).await;
        }

        let described = TerminalSessions::new(supervisor).sessions().await;
        assert_eq!(described.len(), 1);
        assert!(!described[0].live);
        assert_eq!(described[0].status, "failed");
    }

    #[tokio::test]
    async fn a_runtime_with_no_terminals_lists_none() {
        let observer = RecordingTerminalObserver::new();
        let host = FakeTerminal::new(Arc::new(observer));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(host),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp")),
        ));
        assert!(
            TerminalSessions::new(supervisor)
                .sessions()
                .await
                .is_empty()
        );
    }
}
