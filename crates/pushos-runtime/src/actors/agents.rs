//! Putting agent activity on the surface.
//!
//! Sessions live in the supervisor; this turns what they are doing into
//! something the display can show and the lights can say, and it reports
//! activity onto the event bus so nothing else has to poll.

use std::sync::Arc;

use pushos_agents::{AgentSupervisor, Session};
use pushos_domain::agent::AgentState;
use pushos_domain::event::{DomainEvent, EventEnvelope, EventSource};
use pushos_domain::ids::{CorrelationId, SessionId};
use pushos_domain::ports::{AgentEvent, AgentObserver};
use pushos_ui::{SLOT_COUNT, SessionLine, Tone};
use tokio::sync::mpsc;
use tracing::debug;

use crate::bus::EventBus;

/// Turns session activity into runtime events.
///
/// Handed to every backend, so an adapter reports progress without knowing
/// anything about registries, buses or the display.
#[derive(Debug)]
pub struct AgentReporter {
    updates: mpsc::UnboundedSender<(SessionId, AgentEvent)>,
}

impl AgentReporter {
    /// Builds a reporter and the stream the runtime drains it through.
    ///
    /// Unbounded on purpose, and the only unbounded channel in PushOS: this is
    /// called from the thread reading an agent's output, and blocking there
    /// would stall the provider. The stream is drained continuously, and every
    /// item on it is a state transition worth keeping.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<(SessionId, AgentEvent)>) {
        let (updates, stream) = mpsc::unbounded_channel();
        (Self { updates }, stream)
    }
}

impl AgentObserver for AgentReporter {
    fn observe(&self, session: &SessionId, event: AgentEvent) {
        if self.updates.send((session.clone(), event)).is_err() {
            debug!(%session, "the runtime stopped listening to agent activity");
        }
    }
}

/// Applies agent activity to the registry and announces it.
pub struct AgentTask {
    supervisor: Arc<AgentSupervisor>,
    stream: mpsc::UnboundedReceiver<(SessionId, AgentEvent)>,
    bus: EventBus,
    /// Workflows waiting on an agent, when any are configured.
    workflows: Option<Arc<pushos_workflows::WorkflowEngine>>,
}

impl std::fmt::Debug for AgentTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTask").finish_non_exhaustive()
    }
}

impl AgentTask {
    /// Builds the task.
    pub fn new(
        supervisor: Arc<AgentSupervisor>,
        stream: mpsc::UnboundedReceiver<(SessionId, AgentEvent)>,
        bus: EventBus,
    ) -> Self {
        Self {
            supervisor,
            stream,
            bus,
            workflows: None,
        }
    }

    /// Tells workflows when the agent they are waiting on has finished.
    #[must_use]
    pub fn feeding(mut self, workflows: Arc<pushos_workflows::WorkflowEngine>) -> Self {
        self.workflows = Some(workflows);
        self
    }

    /// Runs until the runtime stops.
    ///
    /// Calls `changed` whenever the surface should be redrawn, which is
    /// whenever a session's state or last words moved.
    pub async fn run(mut self, shutdown: crate::shutdown::Shutdown, changed: impl Fn()) {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                update = self.stream.recv() => {
                    let Some((session, event)) = update else { break };
                    self.apply(&session, &event, &changed).await;
                }
            }
        }
    }

    async fn apply(&self, session: &SessionId, event: &AgentEvent, changed: &impl Fn()) {
        let Some(updated) = self.supervisor.record(session, event).await else {
            return;
        };

        // An agent that has stopped and is waiting earns the operator's
        // attention. One working quietly earns a line on the display.
        if updated.state.needs_operator() {
            self.bus.publish(EventEnvelope::root(
                EventSource::Actions,
                CorrelationId::generate(),
                DomainEvent::ActionProgressed {
                    selector: pushos_domain::action::ActionSelector::new("agent", "waiting"),
                    status: pushos_domain::action::ActionStatus::Waiting,
                    message: Some(format!("{} needs you", updated.agent)),
                },
            ));
        }

        // A workflow parked on this agent is waiting for exactly this.
        if let Some(workflows) = &self.workflows {
            workflows.agent_finished(session, updated.state).await;
        }

        changed();
    }
}

/// The lines the display should show for the sessions currently open.
///
/// Sessions waiting on the operator come first: with only eight columns, the
/// ones that have stopped and need a decision are the ones that must be visible.
pub(crate) fn lines_for(sessions: &[Session], selected: Option<&SessionId>) -> Vec<SessionLine> {
    let mut ordered: Vec<&Session> = sessions.iter().filter(|s| s.is_live()).collect();
    ordered.sort_by_key(|session| (!session.state.needs_operator(), !session.state.is_busy()));

    ordered
        .into_iter()
        .take(SLOT_COUNT)
        .map(|session| {
            let mut line = SessionLine::new(
                session.agent.to_string(),
                session.state.to_string(),
                tone_for(session.state),
            );
            if let Some(said) = &session.last_message {
                line = line.with_detail(said.clone());
            }
            if selected == Some(&session.id) {
                line = line.selected();
            }
            line
        })
        .collect()
}

/// How a state should read at a glance.
const fn tone_for(state: AgentState) -> Tone {
    match state {
        AgentState::Working | AgentState::Queued | AgentState::Starting => Tone::Active,
        AgentState::WaitingInput | AgentState::WaitingApproval | AgentState::Paused => {
            Tone::Attention
        }
        AgentState::Failed => Tone::Failure,
        AgentState::Sleeping | AgentState::Cancelled | AgentState::Offline => Tone::Muted,
        AgentState::Completed => Tone::Normal,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use pushos_domain::ids::{AgentId, ProviderName};
    use pushos_domain::ports::SessionHandle;

    use super::*;

    fn session(id: &str, role: &str, state: AgentState) -> Session {
        let mut session = Session::opened(
            &SessionHandle {
                id: SessionId::new(id),
                provider_session: None,
                provider: ProviderName::new("claude"),
            },
            AgentId::new(role),
            None,
            Instant::now(),
        );
        session.state = state;
        session
    }

    #[test]
    fn a_session_waiting_for_a_decision_comes_first() {
        let sessions = vec![
            session("s1", "architect", AgentState::Working),
            session("s2", "builder", AgentState::WaitingApproval),
            session("s3", "reviewer", AgentState::Sleeping),
        ];

        let lines = lines_for(&sessions, None);
        assert_eq!(
            lines[0].name, "builder",
            "the one that has stopped must be visible"
        );
        assert_eq!(lines[0].tone, Tone::Attention);
    }

    #[test]
    fn work_in_progress_comes_before_idle_roles() {
        let sessions = vec![
            session("s1", "idle", AgentState::Sleeping),
            session("s2", "busy", AgentState::Working),
        ];

        let lines = lines_for(&sessions, None);
        assert_eq!(lines[0].name, "busy");
        assert_eq!(lines[0].tone, Tone::Active);
    }

    #[test]
    fn a_finished_session_is_not_shown_as_running() {
        let sessions = vec![
            session("s1", "done", AgentState::Completed),
            session("s2", "busy", AgentState::Working),
        ];

        let lines = lines_for(&sessions, None);
        assert_eq!(lines.len(), 1, "a finished session takes no column");
        assert_eq!(lines[0].name, "busy");
    }

    #[test]
    fn no_more_sessions_are_shown_than_there_are_columns() {
        let sessions: Vec<_> = (0..20)
            .map(|index| {
                session(
                    &format!("s{index}"),
                    &format!("role{index}"),
                    AgentState::Working,
                )
            })
            .collect();

        assert_eq!(lines_for(&sessions, None).len(), SLOT_COUNT);
    }

    #[test]
    fn the_chosen_session_is_marked() {
        let sessions = vec![
            session("s1", "builder", AgentState::Working),
            session("s2", "reviewer", AgentState::Working),
        ];

        let lines = lines_for(&sessions, Some(&SessionId::new("s2")));
        let reviewer = lines
            .iter()
            .find(|line| line.name == "reviewer")
            .expect("shown");
        assert!(reviewer.selected);
        assert!(
            !lines
                .iter()
                .find(|line| line.name == "builder")
                .expect("shown")
                .selected
        );
    }

    #[test]
    fn a_failed_session_never_reads_as_idle() {
        assert_eq!(tone_for(AgentState::Failed), Tone::Failure);
        for state in AgentState::ALL.into_iter().filter(|s| s.needs_operator()) {
            assert_eq!(tone_for(state), Tone::Attention);
        }
    }
}
