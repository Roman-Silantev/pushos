//! Putting workflow runs on the surface.
//!
//! Runs live in the engine; this turns what they are doing into something the
//! display can show, and reports the ones that need a person onto the event bus
//! so nothing has to poll for them.

use pushos_domain::event::{DomainEvent, EventEnvelope, EventSource};
use pushos_domain::ids::CorrelationId;
use pushos_domain::run::{Run, RunState};
use pushos_domain::workflow::Outcome;
use pushos_ui::{SessionLine, Tone};
use pushos_workflows::RunObserver;
use tokio::sync::mpsc;

use crate::bus::EventBus;

/// Turns run activity into runtime events.
///
/// Handed to the engine, so it reports progress without knowing anything about
/// buses or displays.
#[derive(Debug)]
pub struct RunReporter {
    updates: mpsc::UnboundedSender<Run>,
}

impl RunReporter {
    /// Builds a reporter and the stream the runtime drains it through.
    ///
    /// Unbounded for the same reason the others are: this is called from
    /// wherever a run happens to be stepping, and blocking there would stall it.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Run>) {
        let (updates, stream) = mpsc::unbounded_channel();
        (Self { updates }, stream)
    }
}

impl RunObserver for RunReporter {
    fn observe(&self, run: &Run) {
        if self.updates.send(run.clone()).is_err() {
            tracing::debug!(run = %run.id, "the runtime stopped listening to workflow runs");
        }
    }
}

/// Announces what runs are doing and redraws the surface.
pub struct WorkflowTask {
    stream: mpsc::UnboundedReceiver<Run>,
    bus: EventBus,
}

impl std::fmt::Debug for WorkflowTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowTask").finish_non_exhaustive()
    }
}

impl WorkflowTask {
    /// Builds the task.
    pub fn new(stream: mpsc::UnboundedReceiver<Run>, bus: EventBus) -> Self {
        Self { stream, bus }
    }

    /// Runs until the runtime stops.
    pub async fn run(mut self, shutdown: crate::shutdown::Shutdown, changed: impl Fn()) {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                update = self.stream.recv() => {
                    let Some(run) = update else { break };
                    self.announce(&run);
                    changed();
                }
            }
        }
    }

    fn announce(&self, run: &Run) {
        // A run that has stopped for a person, or ended badly, is worth saying.
        // One taking a step is taking a step, and that is what a display shows.
        let worth_saying = run.is_waiting_on_a_person()
            || matches!(
                run.state,
                RunState::Finished {
                    outcome: Outcome::Failed | Outcome::Exhausted
                }
            );

        if !worth_saying {
            return;
        }

        self.bus.publish(EventEnvelope::root(
            EventSource::Actions,
            CorrelationId::generate(),
            DomainEvent::ActionProgressed {
                selector: pushos_domain::action::ActionSelector::new("workflow", "progressed"),
                status: if run.is_live() {
                    pushos_domain::action::ActionStatus::Waiting
                } else {
                    pushos_domain::action::ActionStatus::Failed
                },
                message: Some(format!("{} {}", run.workflow, run.describe())),
            },
        ));
    }
}

/// The lines the display should show for the runs that matter.
///
/// Live ones, and ones that ended badly: a workflow that has just failed is
/// exactly what an operator wants left on the display.
pub(crate) fn lines_for(runs: &[Run]) -> Vec<SessionLine> {
    runs.iter()
        .filter(|run| run.is_live() || run.outcome().is_some_and(is_bad))
        .map(|run| {
            let mut line = SessionLine::new(run.workflow.to_string(), short(run), tone_for(run));
            if let Some(said) = &run.note {
                line = line.with_detail(said.clone());
            }
            line
        })
        .collect()
}

const fn is_bad(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Failed | Outcome::Exhausted)
}

/// What a run is doing, in one word.
fn short(run: &Run) -> String {
    match &run.state {
        RunState::Running => run.at.to_string(),
        RunState::Waiting(_) => run.describe(),
        RunState::Finished { outcome } => outcome.to_string(),
    }
}

const fn tone_for(run: &Run) -> Tone {
    match &run.state {
        RunState::Waiting(waiting) => match waiting {
            // Stopped and needing a person is the one state that earns
            // attention rather than a line.
            pushos_domain::run::Waiting::Approval { .. } => Tone::Attention,
            _ => Tone::Active,
        },
        RunState::Running => Tone::Active,
        RunState::Finished { outcome } => match outcome {
            Outcome::Succeeded => Tone::Normal,
            Outcome::Failed | Outcome::Exhausted => Tone::Failure,
            Outcome::Cancelled => Tone::Muted,
        },
    }
}

/// Everything a run needs to be reported as a session.
pub(crate) fn describe(run: &Run) -> &'static str {
    match run.state {
        RunState::Running => "running",
        RunState::Waiting(_) => "waiting",
        RunState::Finished { .. } => "finished",
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::{NodeId, WorkflowId};
    use pushos_domain::run::Waiting;

    use super::*;

    fn run() -> Run {
        Run::beginning(
            WorkflowId::new("ship"),
            NodeId::new("plan"),
            None,
            std::time::SystemTime::UNIX_EPOCH,
        )
    }

    #[test]
    fn a_run_that_needs_a_person_asks_for_attention() {
        let mut waiting = run();
        waiting.state = RunState::Waiting(Waiting::Approval {
            question: "Ship it?".to_owned(),
        });

        let lines = lines_for(&[waiting]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].tone, Tone::Attention);
        assert_eq!(lines[0].state, "asking: Ship it?");
    }

    #[test]
    fn a_run_that_finished_cleanly_leaves_the_display() {
        // The columns are for what is happening now.
        let mut done = run();
        done.state = RunState::Finished {
            outcome: Outcome::Succeeded,
        };
        assert!(lines_for(&[done]).is_empty());
    }

    #[test]
    fn a_run_that_failed_stays_on_the_display() {
        let mut failed = run();
        failed.state = RunState::Finished {
            outcome: Outcome::Failed,
        };

        let lines = lines_for(&[failed]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].tone, Tone::Failure);
    }

    #[test]
    fn a_run_the_operator_stopped_leaves_quietly() {
        let mut stopped = run();
        stopped.state = RunState::Finished {
            outcome: Outcome::Cancelled,
        };
        assert!(lines_for(&[stopped]).is_empty());
    }

    #[test]
    fn a_running_step_shows_which_step_it_is_on() {
        let lines = lines_for(&[run()]);
        assert_eq!(lines[0].name, "ship");
        assert_eq!(lines[0].state, "plan");
        assert_eq!(lines[0].tone, Tone::Active);
    }
}
