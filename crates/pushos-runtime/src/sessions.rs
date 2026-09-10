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
use pushos_workflows::WorkflowEngine;

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

/// The terminals PushOS did not start, described for a control client.
///
/// The display shows these and Studio did not, which meant the panel and the
/// window disagreed about what was running on the machine. They are the same
/// runtime and should answer the same question the same way.
#[derive(Debug)]
pub struct AttachedSessions {
    provider: Arc<pushos_actions::providers::session::SessionProvider>,
}

impl AttachedSessions {
    /// Describes the terminals a provider can see.
    pub const fn new(provider: Arc<pushos_actions::providers::session::SessionProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl SessionSource for AttachedSessions {
    async fn sessions(&self) -> Vec<SessionInfo> {
        let selected = self.provider.selected();
        // Asked rather than remembered, and quiet about a failure: the panel
        // reports a lost permission where an operator will see it, and a
        // listing that turned into an error would be a worse way to find out.
        let Ok(open) = self.provider.discover().await else {
            return Vec::new();
        };

        open.into_iter()
            .map(|session| SessionInfo {
                id: session.id.to_string(),
                kind: SessionKind::Attached,
                name: session.label().to_owned(),
                status: session.activity.describe().to_owned(),
                // Always: PushOS did not start it and cannot tell when it ends,
                // so anything it can still see is something it can still type
                // into.
                live: true,
                selected: selected.as_ref() == Some(&session.id),
                target: format!("device:{}", session.id),
                // The title moves as the work does, so there is no durable
                // name. A pad names a position instead, which is what the
                // preset binds.
                standing_target: None,
                detail: Some(session.device().to_owned()),
                workspace: None,
            })
            .collect()
    }
}

/// The workflow runs, described for a control client.
#[derive(Debug)]
pub struct RunSessions {
    engine: Arc<WorkflowEngine>,
}

impl RunSessions {
    /// Describes the runs an engine is driving.
    pub const fn new(engine: Arc<WorkflowEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl SessionSource for RunSessions {
    async fn sessions(&self) -> Vec<SessionInfo> {
        self.engine
            .runs()
            .await
            .into_iter()
            .map(|run| SessionInfo {
                id: run.id.to_string(),
                kind: SessionKind::Workflow,
                name: run.workflow.to_string(),
                status: crate::actors::describe_run(&run).to_owned(),
                live: run.is_live(),
                // A run is not something unqualified actions act on the way a
                // session is; the one that needs a person is chosen instead.
                selected: run.is_waiting_on_a_person(),
                target: format!("run:{}", run.id),
                standing_target: Some(run.workflow.to_string()),
                detail: Some(run.describe()),
                workspace: run.workspace.as_ref().map(ToString::to_string),
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
