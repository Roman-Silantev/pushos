//! One display, two kinds of work.
//!
//! Agents and terminals are supervised separately and report separately, but
//! the surface has eight columns and no notion of which is which. This is the
//! one place that decides what goes in them.

use std::sync::Arc;

use pushos_agents::AgentSupervisor;
use pushos_terminal::TerminalSupervisor;
use pushos_ui::{SLOT_COUNT, SessionLine, Tone};
use pushos_workflows::WorkflowEngine;
use tokio::sync::watch;

use super::input::SessionRefresh;

/// Publishes what everything is doing to the display.
///
/// Both supervisors call this when something moves. Reading them needs locks
/// the renderer must never take, so the lines are built here and published,
/// and the render path only ever reads a finished list.
#[derive(Clone)]
pub(crate) struct SessionPublisher {
    agents: Option<Arc<AgentSupervisor>>,
    terminals: Option<Arc<TerminalSupervisor>>,
    workflows: Option<Arc<WorkflowEngine>>,
    /// Sessions PushOS did not start, as last seen.
    ///
    /// Followed rather than asked for: asking costs a subprocess, and the
    /// display is redrawn far more often than terminal windows open.
    attached: Option<watch::Receiver<Vec<pushos_domain::attached::Attached>>>,
    /// Which of those the operator is looking at.
    looking_at: Option<watch::Receiver<Option<pushos_domain::ids::AttachedId>>>,
    refresh: SessionRefresh,
}

impl std::fmt::Debug for SessionPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPublisher")
            .field("agents", &self.agents.is_some())
            .field("terminals", &self.terminals.is_some())
            .field("workflows", &self.workflows.is_some())
            .field("attached", &self.attached.is_some())
            .finish_non_exhaustive()
    }
}

impl SessionPublisher {
    /// Builds a publisher that has nothing to report yet.
    pub(crate) const fn new(refresh: SessionRefresh) -> Self {
        Self {
            agents: None,
            terminals: None,
            workflows: None,
            attached: None,
            looking_at: None,
            refresh,
        }
    }

    /// Includes the agents.
    pub(crate) fn with_agents(mut self, supervisor: Arc<AgentSupervisor>) -> Self {
        self.agents = Some(supervisor);
        self
    }

    /// Includes the terminals.
    pub(crate) fn with_terminals(mut self, supervisor: Arc<TerminalSupervisor>) -> Self {
        self.terminals = Some(supervisor);
        self
    }

    /// Includes the workflow runs.
    pub(crate) fn with_workflows(mut self, engine: Arc<WorkflowEngine>) -> Self {
        self.workflows = Some(engine);
        self
    }

    /// Includes the sessions PushOS did not start.
    pub(crate) fn with_attached(
        mut self,
        found: watch::Receiver<Vec<pushos_domain::attached::Attached>>,
        looking_at: watch::Receiver<Option<pushos_domain::ids::AttachedId>>,
    ) -> Self {
        self.attached = Some(found);
        self.looking_at = Some(looking_at);
        self
    }

    /// Rebuilds the lines and sends them, off the caller's thread.
    ///
    /// Called from the synchronous point where something changed, which must
    /// not wait on a supervisor's lock.
    pub(crate) fn publish(&self) {
        let publisher = self.clone();
        tokio::spawn(async move {
            let _ = publisher.refresh.send(publisher.lines().await);
        });
    }

    /// What the display should show right now.
    async fn lines(&self) -> Vec<SessionLine> {
        let mut lines = Vec::new();

        if let Some(agents) = &self.agents {
            let sessions = agents.sessions().await;
            let selected = agents.selected().await.map(|session| session.id);
            lines.extend(super::agent_lines(&sessions, selected.as_ref()));
        }

        if let Some(terminals) = &self.terminals {
            lines.extend(super::terminal_lines(&terminals.summaries().await));
        }

        if let Some(workflows) = &self.workflows {
            lines.extend(super::run_lines(&workflows.runs().await));
        }

        if let Some(attached) = &self.attached {
            let looking_at = self
                .looking_at
                .as_ref()
                .and_then(|held| held.borrow().clone());
            lines.extend(super::attached_lines(
                &attached.borrow(),
                looking_at.as_ref(),
            ));
        }

        merge(lines)
    }
}

/// Merges everything that is running into the lines the display shows.
///
/// Ordered by how much attention each one wants rather than by kind: an agent
/// waiting on a decision and a build that just failed both matter more than a
/// terminal quietly compiling, whoever is running them.
pub(crate) fn merge(lines: impl IntoIterator<Item = SessionLine>) -> Vec<SessionLine> {
    let mut all: Vec<SessionLine> = lines.into_iter().collect();
    // Stable, so sessions of equal urgency keep the order their supervisor
    // produced them in and nothing moves under the operator's finger.
    all.sort_by_key(|line| urgency(line.tone));
    all.truncate(SLOT_COUNT);
    all
}

/// How much attention a line wants, lowest first.
const fn urgency(tone: Tone) -> u8 {
    match tone {
        Tone::Attention => 0,
        Tone::Failure => 1,
        Tone::Active => 2,
        Tone::Normal => 3,
        Tone::Muted => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str, tone: Tone) -> SessionLine {
        SessionLine::new(name, "state", tone)
    }

    #[test]
    fn what_needs_a_decision_comes_before_everything_else() {
        let merged = merge([
            line("compiling", Tone::Active),
            line("builder", Tone::Attention),
            line("idle", Tone::Muted),
        ]);
        assert_eq!(merged[0].name, "builder");
    }

    #[test]
    fn a_failure_outranks_work_in_progress() {
        let merged = merge([
            line("compiling", Tone::Active),
            line("tests", Tone::Failure),
        ]);
        assert_eq!(merged[0].name, "tests");
    }

    #[test]
    fn sessions_of_equal_urgency_keep_the_order_they_arrived_in() {
        // A pad that meant one column must not come to mean another because
        // something unrelated changed.
        let merged = merge([
            line("first", Tone::Active),
            line("second", Tone::Active),
            line("third", Tone::Active),
        ]);
        let names: Vec<&str> = merged.iter().map(|line| line.name.as_str()).collect();
        assert_eq!(names, ["first", "second", "third"]);
    }

    #[test]
    fn no_more_lines_are_produced_than_there_are_columns() {
        let many = (0..SLOT_COUNT * 3).map(|index| line(&format!("s{index}"), Tone::Active));
        assert_eq!(merge(many).len(), SLOT_COUNT);
    }

    #[test]
    fn the_columns_that_survive_are_the_ones_that_matter() {
        let mut lines: Vec<SessionLine> = (0..SLOT_COUNT)
            .map(|index| line(&format!("quiet{index}"), Tone::Muted))
            .collect();
        lines.push(line("waiting", Tone::Attention));

        let merged = merge(lines);
        assert_eq!(merged.len(), SLOT_COUNT);
        assert_eq!(merged[0].name, "waiting");
    }
}
