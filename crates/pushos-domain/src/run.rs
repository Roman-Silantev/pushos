//! One workflow, running.
//!
//! A run outlives the process that started it. Everything here is therefore
//! written in terms that survive being put in a database and read back by a
//! different process: names rather than handles, and wall-clock times rather
//! than instants, which mean nothing after a restart.

use std::fmt;
use std::time::SystemTime;

use crate::ids::{NodeId, RunId, SessionId, WorkflowId, WorkspaceId};
use crate::workflow::Outcome;

/// One run of a workflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// What this run is called.
    pub id: RunId,
    /// The workflow it is running.
    pub workflow: WorkflowId,
    /// The project it belongs to, when one was in effect.
    pub workspace: Option<WorkspaceId>,
    /// Where it is.
    pub at: NodeId,
    /// What it is doing there.
    pub state: RunState,
    /// How many steps it has taken.
    ///
    /// Counted rather than derived, because it is what stops a loop.
    pub steps: u32,
    /// Whether the step before this one succeeded.
    pub last_succeeded: bool,
    /// The last thing worth saying about it, for the display.
    pub note: Option<String>,
    /// When it started, by the clock rather than the process.
    pub started_at: SystemTime,
    /// When it last did anything.
    pub updated_at: SystemTime,
}

impl Run {
    /// Begins a run at a workflow's first step.
    pub fn beginning(
        workflow: WorkflowId,
        start: NodeId,
        workspace: Option<WorkspaceId>,
        now: SystemTime,
    ) -> Self {
        Self {
            id: RunId::generate(),
            workflow,
            workspace,
            at: start,
            state: RunState::Running,
            steps: 0,
            last_succeeded: true,
            note: None,
            started_at: now,
            updated_at: now,
        }
    }

    /// Whether it is still going.
    pub const fn is_live(&self) -> bool {
        !matches!(self.state, RunState::Finished { .. })
    }

    /// How it ended, if it has.
    pub const fn outcome(&self) -> Option<Outcome> {
        match self.state {
            RunState::Finished { outcome } => Some(outcome),
            _ => None,
        }
    }

    /// Whether it has stopped and needs a person.
    ///
    /// The one state that earns the operator's attention rather than a line on
    /// the display, which is what makes a single approve pad useful.
    pub const fn is_waiting_on_a_person(&self) -> bool {
        matches!(self.state, RunState::Waiting(Waiting::Approval { .. }))
    }

    /// What it is doing, in a few words, for the display.
    pub fn describe(&self) -> String {
        match &self.state {
            RunState::Running => format!("at {}", self.at),
            RunState::Waiting(waiting) => waiting.to_string(),
            RunState::Finished { outcome } => outcome.to_string(),
        }
    }
}

/// What a run is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Taking a step.
    Running,
    /// Stopped, waiting for something outside itself.
    Waiting(Waiting),
    /// Over.
    Finished {
        /// How it ended.
        outcome: Outcome,
    },
}

/// What a stopped run is waiting for.
///
/// This is the state a restart has to be able to read and act on, so each
/// variant names what it is waiting for in a way that outlives the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// An agent is working.
    Agent {
        /// The session that is working, when one was started.
        session: Option<SessionId>,
    },
    /// A command is running.
    Terminal {
        /// The terminal it is running in, by name.
        terminal: String,
    },
    /// The operator has been asked something.
    Approval {
        /// What they were asked.
        question: String,
    },
    /// Time is passing.
    Delay {
        /// When it may go on, by the clock.
        until: SystemTime,
    },
}

impl fmt::Display for Waiting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent { .. } => f.write_str("waiting for an agent"),
            Self::Terminal { terminal } => write!(f, "running in {terminal}"),
            Self::Approval { question } => write!(f, "asking: {question}"),
            Self::Delay { .. } => f.write_str("waiting"),
        }
    }
}

/// One step a run took, as it is written down.
///
/// Recorded before the step after it runs, so that what a restart reads is
/// never ahead of what actually happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// The run that took it.
    pub run: RunId,
    /// Where it came from, absent for the first step.
    pub from: Option<NodeId>,
    /// Where it went.
    pub to: NodeId,
    /// What happened, when there is anything to say.
    pub note: Option<String>,
    /// When.
    pub at: SystemTime,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn run() -> Run {
        Run::beginning(
            WorkflowId::new("build"),
            NodeId::new("plan"),
            None,
            SystemTime::UNIX_EPOCH,
        )
    }

    #[test]
    fn a_new_run_starts_where_the_workflow_does_and_has_taken_no_steps() {
        let run = run();
        assert_eq!(run.at, NodeId::new("plan"));
        assert_eq!(run.steps, 0);
        assert!(run.is_live());
        assert_eq!(run.outcome(), None);
    }

    #[test]
    fn a_finished_run_is_not_live_and_says_how_it_ended() {
        let mut run = run();
        run.state = RunState::Finished {
            outcome: Outcome::Succeeded,
        };

        assert!(!run.is_live());
        assert_eq!(run.outcome(), Some(Outcome::Succeeded));
        assert_eq!(run.describe(), "succeeded");
    }

    #[test]
    fn a_waiting_run_says_what_it_is_waiting_for() {
        // This is what the display shows while nothing appears to be happening,
        // which is exactly when the operator wants to know.
        let mut run = run();
        run.state = RunState::Waiting(Waiting::Terminal {
            terminal: "tests".to_owned(),
        });
        assert_eq!(run.describe(), "running in tests");

        run.state = RunState::Waiting(Waiting::Approval {
            question: "Ship it?".to_owned(),
        });
        assert_eq!(run.describe(), "asking: Ship it?");
    }

    #[test]
    fn a_run_waiting_on_time_names_a_moment_a_restart_can_read() {
        // An instant means nothing to the process that reads it back.
        let until = SystemTime::UNIX_EPOCH + Duration::from_secs(90);
        let waiting = Waiting::Delay { until };

        let Waiting::Delay { until: read } = waiting else {
            panic!("expected a delay");
        };
        assert_eq!(read, until);
    }
}
