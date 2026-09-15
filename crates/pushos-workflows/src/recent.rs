//! How many finished runs the engine keeps in memory.
//!
//! A run still going is always kept: it is what the engine drives. A finished
//! one is kept only so the surface can show how it ended, and the last few of
//! each workflow are all anyone looks at. Kept for ever, every build a pad ever
//! started would stay in memory, and be copied and sorted on every look, for as
//! long as PushOS runs. The database keeps a longer record.

use std::cmp::Reverse;
use std::collections::HashMap;

use pushos_domain::ids::{RunId, WorkflowId};
use pushos_domain::run::Run;

/// The most finished runs of one workflow kept in memory.
pub(crate) const KEPT_FINISHED: usize = 3;

/// Drops a workflow's oldest finished runs, keeping the newest few.
pub(crate) fn forget_stale(runs: &mut HashMap<RunId, Run>, workflow: &WorkflowId) {
    let mut finished: Vec<(RunId, std::time::SystemTime)> = runs
        .values()
        .filter(|run| &run.workflow == workflow && !run.is_live())
        .map(|run| (run.id, run.updated_at))
        .collect();
    if finished.len() <= KEPT_FINISHED {
        return;
    }
    finished.sort_by_key(|(_, updated)| Reverse(*updated));
    for (id, _) in finished.split_off(KEPT_FINISHED) {
        runs.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use pushos_domain::ids::NodeId;
    use pushos_domain::run::RunState;
    use pushos_domain::workflow::Outcome;

    use super::*;

    fn run(workflow: &str, finished: bool, seconds: u64) -> Run {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let mut run = Run::beginning(WorkflowId::new(workflow), NodeId::new("start"), None, at);
        if finished {
            run.state = RunState::Finished {
                outcome: Outcome::Succeeded,
            };
        }
        run
    }

    fn book(runs: impl IntoIterator<Item = Run>) -> HashMap<RunId, Run> {
        runs.into_iter().map(|run| (run.id, run)).collect()
    }

    #[test]
    fn only_the_newest_finished_runs_of_a_workflow_stay() {
        let mut runs = book((0..10).map(|second| run("build", true, second)));

        forget_stale(&mut runs, &WorkflowId::new("build"));

        let mut kept: Vec<u64> = runs
            .values()
            .map(|run| {
                run.updated_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("after the epoch")
                    .as_secs()
            })
            .collect();
        kept.sort_unstable();
        assert_eq!(kept, [7, 8, 9]);
    }

    #[test]
    fn a_run_still_going_and_another_workflows_runs_are_never_dropped() {
        let mut runs = book(
            (0..10)
                .map(|second| run("build", true, second))
                .chain([run("build", false, 0), run("deploy", true, 0)]),
        );

        forget_stale(&mut runs, &WorkflowId::new("build"));

        assert!(runs.values().any(Run::is_live), "still going");
        assert!(
            runs.values()
                .any(|run| run.workflow == WorkflowId::new("deploy")),
            "not this workflow's to drop"
        );
        assert_eq!(runs.len(), KEPT_FINISHED + 2);
    }
}
