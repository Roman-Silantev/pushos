//! Workflow runs, kept in the database.
//!
//! Durability here is not best-effort. The engine writes a step and waits for
//! the answer before running the step after it, so these calls acknowledge the
//! commit rather than dropping the work on a queue and returning.
//!
//! What is written is meant to be readable by a person with a database viewer.
//! A run that went wrong is something an operator will want to look at, and a
//! column of opaque bytes would be no help at all.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pushos_domain::ids::{NodeId, RunId, SessionId, WorkflowId, WorkspaceId};
use pushos_domain::ports::{RunStore, RunStoreError};
use pushos_domain::run::{Run, RunState, Transition, Waiting};
use pushos_domain::workflow::Outcome;

use crate::error::StorageError;
use crate::writer::Storage;

#[async_trait]
impl RunStore for Storage {
    async fn record(&self, run: &Run) -> Result<(), RunStoreError> {
        self.save_run(RunRow::from(run))
            .await
            .map_err(|error| RunStoreError::backend("recording a workflow run", error))
    }

    async fn append(&self, transition: &Transition) -> Result<(), RunStoreError> {
        self.append_transition(TransitionRow {
            run_id: transition.run.to_string(),
            from_node: transition.from.as_ref().map(ToString::to_string),
            to_node: transition.to.to_string(),
            note: transition.note.clone(),
            recorded_at: seconds(transition.at),
        })
        .await
        .map_err(|error| RunStoreError::backend("recording a workflow step", error))
    }

    async fn unfinished(&self) -> Result<Vec<Run>, RunStoreError> {
        let rows = self
            .unfinished_runs()
            .await
            .map_err(|error| RunStoreError::backend("reading what was running", error))?;

        Ok(rows.iter().filter_map(RunRow::to_run).collect())
    }

    async fn forget(&self, run: &RunId) -> Result<(), RunStoreError> {
        self.forget_run(run.to_string())
            .await
            .map_err(|error| RunStoreError::backend("forgetting a workflow run", error))
    }
}

/// One run, as a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    /// The run.
    pub id: String,
    /// The workflow it is running.
    pub workflow_id: String,
    /// The project it belongs to.
    pub workspace_id: Option<String>,
    /// Where it is.
    pub node_id: String,
    /// What it is doing: `running`, `waiting` or `finished`.
    pub state: String,
    /// What it is waiting for, when it is waiting.
    pub waiting: Option<String>,
    /// How it ended, when it has.
    pub outcome: Option<String>,
    /// How many steps it has taken.
    pub steps: i64,
    /// Whether the step before this one succeeded.
    pub last_succeeded: bool,
    /// The last thing worth saying about it.
    pub note: Option<String>,
    /// When it started.
    pub started_at: i64,
    /// When it last did anything.
    pub updated_at: i64,
}

impl From<&Run> for RunRow {
    fn from(run: &Run) -> Self {
        let (state, waiting, outcome) = match &run.state {
            RunState::Running => ("running", None, None),
            RunState::Waiting(waiting) => ("waiting", Some(encode_waiting(waiting)), None),
            RunState::Finished { outcome } => ("finished", None, Some(outcome.to_string())),
        };

        Self {
            id: run.id.to_string(),
            workflow_id: run.workflow.to_string(),
            workspace_id: run.workspace.as_ref().map(ToString::to_string),
            node_id: run.at.to_string(),
            state: state.to_owned(),
            waiting,
            outcome,
            steps: i64::from(run.steps),
            last_succeeded: run.last_succeeded,
            note: run.note.clone(),
            started_at: seconds(run.started_at),
            updated_at: seconds(run.updated_at),
        }
    }
}

impl RunRow {
    /// Reads a row back, ignoring one that cannot be understood.
    ///
    /// A row PushOS cannot read is a row from a future version or a hand edit.
    /// Skipping it loses one run; refusing to start loses all of them.
    pub fn to_run(&self) -> Option<Run> {
        let state = match self.state.as_str() {
            "running" => RunState::Running,
            "waiting" => RunState::Waiting(decode_waiting(self.waiting.as_deref()?)?),
            "finished" => RunState::Finished {
                outcome: decode_outcome(self.outcome.as_deref()?)?,
            },
            _ => return None,
        };

        Some(Run {
            id: RunId::from_str(&self.id).ok()?,
            workflow: WorkflowId::new(&self.workflow_id),
            workspace: self.workspace_id.as_deref().map(WorkspaceId::new),
            at: NodeId::new(&self.node_id),
            state,
            steps: u32::try_from(self.steps).unwrap_or(u32::MAX),
            last_succeeded: self.last_succeeded,
            note: self.note.clone(),
            started_at: at(self.started_at),
            updated_at: at(self.updated_at),
        })
    }
}

/// One step, as a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionRow {
    /// The run that took it.
    pub run_id: String,
    /// Where it came from.
    pub from_node: Option<String>,
    /// Where it went.
    pub to_node: String,
    /// What happened.
    pub note: Option<String>,
    /// When.
    pub recorded_at: i64,
}

/// Writes what a run is waiting for, in a form a person can read.
fn encode_waiting(waiting: &Waiting) -> String {
    match waiting {
        Waiting::Agent { session } => format!(
            "agent:{}",
            session
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        ),
        Waiting::Terminal { terminal } => format!("terminal:{terminal}"),
        Waiting::Approval { question } => format!("approval:{question}"),
        Waiting::Delay { until } => format!("delay:{}", seconds(*until)),
    }
}

/// Reads it back, refusing anything it does not recognise.
fn decode_waiting(text: &str) -> Option<Waiting> {
    // Split once, so a question with a colon in it survives.
    let (kind, rest) = text.split_once(':')?;
    match kind {
        "agent" => Some(Waiting::Agent {
            session: (!rest.is_empty()).then(|| SessionId::new(rest)),
        }),
        "terminal" => Some(Waiting::Terminal {
            terminal: rest.to_owned(),
        }),
        "approval" => Some(Waiting::Approval {
            question: rest.to_owned(),
        }),
        "delay" => Some(Waiting::Delay {
            until: at(rest.parse().ok()?),
        }),
        _ => None,
    }
}

fn decode_outcome(text: &str) -> Option<Outcome> {
    match text {
        "succeeded" => Some(Outcome::Succeeded),
        "failed" => Some(Outcome::Failed),
        "cancelled" => Some(Outcome::Cancelled),
        "exhausted" => Some(Outcome::Exhausted),
        _ => None,
    }
}

fn seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

fn at(seconds: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
}

/// So the storage errors read the same way whichever call produced them.
impl From<StorageError> for RunStoreError {
    fn from(error: StorageError) -> Self {
        Self::backend("talking to the database", error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting_round_trips(waiting: &Waiting) {
        let written = encode_waiting(waiting);
        assert_eq!(
            decode_waiting(&written).as_ref(),
            Some(waiting),
            "`{written}` did not read back"
        );
    }

    #[test]
    fn everything_a_run_can_wait_for_survives_being_written_down() {
        // This is what a restart reads, so anything that does not round trip is
        // a run that can never be picked up.
        waiting_round_trips(&Waiting::Agent {
            session: Some(SessionId::new("s1")),
        });
        waiting_round_trips(&Waiting::Agent { session: None });
        waiting_round_trips(&Waiting::Terminal {
            terminal: "tests".to_owned(),
        });
        waiting_round_trips(&Waiting::Delay {
            until: at(1_700_000_000),
        });
    }

    #[test]
    fn a_question_with_a_colon_in_it_survives() {
        waiting_round_trips(&Waiting::Approval {
            question: "Ship it: really?".to_owned(),
        });
    }

    #[test]
    fn a_row_that_cannot_be_understood_is_skipped_rather_than_fatal() {
        // One unreadable run should not stop PushOS picking up the others.
        assert!(decode_waiting("nonsense").is_none());
        assert!(decode_waiting("agent").is_none());
        assert!(decode_outcome("exploded").is_none());
    }

    #[test]
    fn a_run_survives_being_written_down_and_read_back() {
        let mut run = Run::beginning(
            WorkflowId::new("ship"),
            NodeId::new("plan"),
            Some(WorkspaceId::new("sydclaw")),
            at(1_700_000_000),
        );
        run.steps = 4;
        run.last_succeeded = false;
        run.note = Some("tests failed".to_owned());
        run.state = RunState::Waiting(Waiting::Terminal {
            terminal: "tests".to_owned(),
        });

        let read = RunRow::from(&run).to_run().expect("it reads back");
        assert_eq!(read, run);
    }

    #[test]
    fn a_finished_run_keeps_how_it_ended() {
        let mut run = Run::beginning(
            WorkflowId::new("ship"),
            NodeId::new("done"),
            None,
            at(1_700_000_000),
        );
        run.state = RunState::Finished {
            outcome: Outcome::Exhausted,
        };

        let read = RunRow::from(&run).to_run().expect("it reads back");
        assert_eq!(read.outcome(), Some(Outcome::Exhausted));
    }
}

#[cfg(test)]
mod against_a_real_database {
    use pushos_domain::ids::PageId;

    use pushos_domain::ports::WorkspaceMemoryStore as _;

    use crate::StorageWriter;

    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pushos-runs-{}.sqlite",
            pushos_domain::ids::ExecutionId::generate()
        ))
    }

    fn run_at(node: &str) -> Run {
        Run::beginning(
            WorkflowId::new("ship"),
            NodeId::new(node),
            None,
            SystemTime::now(),
        )
    }

    #[tokio::test]
    async fn a_run_written_before_a_restart_is_there_after_one() {
        // The whole claim of the phase, against the database that has to keep
        // it rather than against a fake.
        let path = scratch();
        let mut run = run_at("tests");
        run.state = RunState::Waiting(Waiting::Terminal {
            terminal: "tests".to_owned(),
        });

        {
            let writer = StorageWriter::open(&path).expect("a fresh database opens");
            writer.handle().record(&run).await.expect("it is written");
        }

        // A new process, over the same file.
        let writer = StorageWriter::open(&path).expect("the database reopens");
        let unfinished = writer.handle().unfinished().await.expect("it reads");

        assert_eq!(unfinished.len(), 1);
        let read = &unfinished[0];
        assert_eq!(read.id, run.id);
        assert_eq!(read.at, run.at);
        assert_eq!(
            read.state, run.state,
            "and it knows what it was waiting for"
        );

        // Times are kept to the second. A workflow that cared about
        // milliseconds across a restart would be measuring the wrong thing.
        assert!(
            read.started_at <= run.started_at
                && run
                    .started_at
                    .duration_since(read.started_at)
                    .expect("ordered")
                    < Duration::from_secs(1)
        );
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn a_finished_run_is_not_picked_up_again() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();

        let mut run = run_at("done");
        storage.record(&run).await.expect("it is written");
        run.state = RunState::Finished {
            outcome: Outcome::Succeeded,
        };
        storage.record(&run).await.expect("it is updated");

        assert!(storage.unfinished().await.expect("it reads").is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn a_run_is_updated_in_place_rather_than_written_twice() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();

        let mut run = run_at("plan");
        storage.record(&run).await.expect("written");
        run.at = NodeId::new("build");
        run.steps = 1;
        storage.record(&run).await.expect("written");

        let unfinished = storage.unfinished().await.expect("it reads");
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].at, NodeId::new("build"));
        assert_eq!(unfinished[0].steps, 1);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn every_step_is_kept_in_the_order_it_happened() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();
        let run = run_at("plan");
        storage.record(&run).await.expect("written");

        for (from, to) in [("plan", "build"), ("build", "tests")] {
            storage
                .append(&Transition {
                    run: run.id,
                    from: Some(NodeId::new(from)),
                    to: NodeId::new(to),
                    note: Some(format!("{from} finished")),
                    at: SystemTime::now(),
                })
                .await
                .expect("written");
        }

        storage.forget(&run.id).await.expect("forgotten");
        assert!(storage.unfinished().await.expect("it reads").is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn writing_a_run_does_not_disturb_what_else_is_in_the_database() {
        // The runs share a file with the audit trail and the workspaces, and a
        // migration that broke either would be a bad way to find out.
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();

        storage.record(&run_at("plan")).await.expect("written");
        storage
            .remember(
                &WorkspaceId::new("sydclaw"),
                &pushos_domain::context::WorkspaceMemory {
                    last_page: Some(PageId::new("development")),
                    selected_session: None,
                },
            )
            .await;

        assert_eq!(
            storage
                .recall(&WorkspaceId::new("sydclaw"))
                .await
                .and_then(|kept| kept.last_page),
            Some(PageId::new("development"))
        );
        std::fs::remove_file(&path).ok();
    }
}
