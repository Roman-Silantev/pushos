//! Putting terminal activity on the surface.
//!
//! Terminals live in the supervisor; this turns what they are doing into
//! something the display can show, and reports the ones that ended badly onto
//! the event bus so nothing has to poll for them.

use std::sync::Arc;

use pushos_domain::event::{DomainEvent, EventEnvelope, EventSource};
use pushos_domain::ids::{CorrelationId, SessionId};
use pushos_domain::ports::{TerminalEvent, TerminalObserver, TerminalStatus};
use pushos_terminal::{TerminalSummary, TerminalSupervisor};
use pushos_ui::{SessionLine, Tone};
use tokio::sync::mpsc;
use tracing::debug;

use crate::bus::EventBus;

/// Turns terminal activity into runtime events.
///
/// Handed to the pseudo-terminal host, so the thread reading a program's output
/// reports progress without knowing anything about registries or the display.
#[derive(Debug)]
pub struct TerminalReporter {
    updates: mpsc::UnboundedSender<(SessionId, TerminalEvent)>,
}

impl TerminalReporter {
    /// Builds a reporter and the stream the runtime drains it through.
    ///
    /// Unbounded for the same reason the agent reporter is: this is called from
    /// the thread reading a terminal, and blocking there would stall the
    /// program on the other end of it.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<(SessionId, TerminalEvent)>) {
        let (updates, stream) = mpsc::unbounded_channel();
        (Self { updates }, stream)
    }
}

impl TerminalObserver for TerminalReporter {
    fn observe(&self, terminal: &SessionId, event: TerminalEvent) {
        if self.updates.send((terminal.clone(), event)).is_err() {
            debug!(%terminal, "the runtime stopped listening to terminal activity");
        }
    }
}

/// Applies terminal activity to the registry and announces it.
pub struct TerminalTask {
    supervisor: Arc<TerminalSupervisor>,
    stream: mpsc::UnboundedReceiver<(SessionId, TerminalEvent)>,
    bus: EventBus,
    /// Workflows waiting on a command, when any are configured.
    workflows: Option<Arc<pushos_workflows::WorkflowEngine>>,
}

impl std::fmt::Debug for TerminalTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalTask").finish_non_exhaustive()
    }
}

impl TerminalTask {
    /// Builds the task.
    pub fn new(
        supervisor: Arc<TerminalSupervisor>,
        stream: mpsc::UnboundedReceiver<(SessionId, TerminalEvent)>,
        bus: EventBus,
    ) -> Self {
        Self {
            supervisor,
            stream,
            bus,
            workflows: None,
        }
    }

    /// Tells workflows when the command they are waiting on has exited.
    #[must_use]
    pub fn feeding(mut self, workflows: Arc<pushos_workflows::WorkflowEngine>) -> Self {
        self.workflows = Some(workflows);
        self
    }

    /// Runs until the runtime stops.
    ///
    /// Calls `changed` whenever the surface should be redrawn.
    pub async fn run(mut self, shutdown: crate::shutdown::Shutdown, changed: impl Fn()) {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                update = self.stream.recv() => {
                    let Some((terminal, event)) = update else { break };
                    self.apply(&terminal, &event, &changed).await;
                }
            }
        }

        // PushOS started these processes, so it stops them rather than leaving
        // them running with nothing reading them.
        self.supervisor.shutdown().await;
    }

    async fn apply(&self, terminal: &SessionId, event: &TerminalEvent, changed: &impl Fn()) {
        let Some(updated) = self.supervisor.apply(terminal, event).await else {
            return;
        };

        // A terminal that failed is the one thing here worth announcing. One
        // producing output is producing output; that is what a display is for,
        // and one the operator stopped is the operator getting what they asked.
        if updated.status.is_failure() {
            self.bus.publish(EventEnvelope::root(
                EventSource::Actions,
                CorrelationId::generate(),
                DomainEvent::ActionProgressed {
                    selector: pushos_domain::action::ActionSelector::new("terminal", "exited"),
                    status: pushos_domain::action::ActionStatus::Failed,
                    message: Some(format!("{} {}", updated.name, describe(updated.status))),
                },
            ));
        }

        // A workflow parked on this terminal is waiting for exactly this.
        if let Some(workflows) = &self.workflows {
            workflows
                .terminal_finished(&updated.name, updated.status)
                .await;
        }

        changed();
    }
}

/// The lines the display should show for the terminals that matter.
///
/// Running ones, and ones that failed: a build that has just gone red is
/// exactly what an operator wants left on the display.
pub(crate) fn lines_for(terminals: &[TerminalSummary]) -> Vec<SessionLine> {
    terminals
        .iter()
        .filter(|terminal| terminal.status.is_live() || terminal.status.is_failure())
        .map(|terminal| {
            let mut line = SessionLine::new(
                terminal.name.clone(),
                describe(terminal.status),
                tone_for(terminal.status),
            );
            if let Some(said) = &terminal.last_line {
                line = line.with_detail(said.clone());
            }
            if terminal.selected {
                line = line.selected();
            }
            line
        })
        .collect()
}

/// How a status should read on the display.
pub(crate) const fn describe(status: TerminalStatus) -> &'static str {
    match status {
        TerminalStatus::Starting => "starting",
        TerminalStatus::Running => "running",
        TerminalStatus::Exited { code: Some(0) } => "done",
        TerminalStatus::Stopped => "stopped",
        TerminalStatus::Exited { .. } => "failed",
        TerminalStatus::Failed => "no start",
    }
}

/// How a status should read at a glance.
const fn tone_for(status: TerminalStatus) -> Tone {
    match status {
        TerminalStatus::Starting | TerminalStatus::Running => Tone::Active,
        TerminalStatus::Exited { code: Some(0) } => Tone::Normal,
        TerminalStatus::Stopped => Tone::Muted,
        TerminalStatus::Exited { .. } | TerminalStatus::Failed => Tone::Failure,
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::SessionId;

    use super::*;

    fn summary(name: &str, status: TerminalStatus) -> TerminalSummary {
        TerminalSummary {
            id: SessionId::new(name),
            name: name.to_owned(),
            program: "bash".to_owned(),
            status,
            pid: None,
            selected: false,
            last_line: None,
            recent: Vec::new(),
        }
    }

    #[test]
    fn a_running_terminal_gets_a_line() {
        let lines = lines_for(&[summary("tests", TerminalStatus::Running)]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name, "tests");
        assert_eq!(lines[0].tone, Tone::Active);
    }

    #[test]
    fn a_terminal_that_finished_cleanly_leaves_the_display() {
        // The columns are for what is happening now.
        let lines = lines_for(&[summary("tests", TerminalStatus::Exited { code: Some(0) })]);
        assert!(lines.is_empty());
    }

    #[test]
    fn a_terminal_that_failed_stays_on_the_display() {
        // A build that has just gone red is the thing worth looking at.
        let lines = lines_for(&[summary("tests", TerminalStatus::Exited { code: Some(1) })]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].tone, Tone::Failure);
        assert_eq!(lines[0].state, "failed");
    }

    #[test]
    fn a_terminal_the_operator_stopped_leaves_the_display_quietly() {
        // Red for a job the operator deliberately ended is a lie the surface
        // would go on telling until something else pushed it off.
        let lines = lines_for(&[summary("server", TerminalStatus::Stopped)]);
        assert!(lines.is_empty());
    }

    #[test]
    fn a_terminal_that_never_started_is_shown_as_such() {
        let lines = lines_for(&[summary("tests", TerminalStatus::Failed)]);
        assert_eq!(lines[0].state, "no start");
        assert_eq!(lines[0].tone, Tone::Failure);
    }

    #[test]
    fn the_last_thing_a_terminal_said_becomes_the_detail() {
        let mut running = summary("tests", TerminalStatus::Running);
        running.last_line = Some("77 passed".to_owned());
        running.selected = true;

        let lines = lines_for(&[running]);
        assert_eq!(lines[0].detail.as_deref(), Some("77 passed"));
        assert!(lines[0].selected);
    }
}
