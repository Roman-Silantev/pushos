//! A run store that lives only as long as the test.
//!
//! Durability is the whole point of workflows, so this is written to be handed
//! from one engine to another: what the first wrote, the second reads, which is
//! exactly what a restart looks like from the inside.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use pushos_domain::ids::RunId;
use pushos_domain::ports::{RunStore, RunStoreError};
use pushos_domain::run::{Run, RunState, Transition};

/// Runs and their history, kept in memory.
#[derive(Debug, Clone, Default)]
pub struct FakeRunStore {
    runs: Arc<Mutex<HashMap<RunId, Run>>>,
    history: Arc<Mutex<Vec<Transition>>>,
    fail: Arc<Mutex<bool>>,
}

impl FakeRunStore {
    /// Builds a store that has kept nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every step every run has taken, in the order they were written.
    pub fn history(&self) -> Vec<Transition> {
        lock(&self.history).clone()
    }

    /// The steps one run took, by name.
    pub fn steps_of(&self, run: &RunId) -> Vec<String> {
        lock(&self.history)
            .iter()
            .filter(|transition| transition.run == *run)
            .map(|transition| transition.to.to_string())
            .collect()
    }

    /// What is written down for a run.
    pub fn run(&self, id: &RunId) -> Option<Run> {
        lock(&self.runs).get(id).cloned()
    }

    /// How many runs have been written down.
    pub fn len(&self) -> usize {
        lock(&self.runs).len()
    }

    /// Whether anything has been written down.
    pub fn is_empty(&self) -> bool {
        lock(&self.runs).is_empty()
    }

    /// Puts a run in the store without going through the port.
    ///
    /// Used to hand one engine what another wrote, which is what a restart
    /// looks like from the inside.
    pub fn preload(&self, run: &Run) {
        lock(&self.runs).insert(run.id, run.clone());
    }

    /// Makes every subsequent write fail.
    pub fn fail_writes(&self) {
        *lock(&self.fail) = true;
    }

    fn check(&self) -> Result<(), RunStoreError> {
        if *lock(&self.fail) {
            Err(RunStoreError::backend(
                "the fake was told to fail",
                std::io::Error::other("injected failure"),
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl RunStore for FakeRunStore {
    async fn record(&self, run: &Run) -> Result<(), RunStoreError> {
        self.check()?;
        lock(&self.runs).insert(run.id, run.clone());
        Ok(())
    }

    async fn append(&self, transition: &Transition) -> Result<(), RunStoreError> {
        self.check()?;
        lock(&self.history).push(transition.clone());
        Ok(())
    }

    async fn unfinished(&self) -> Result<Vec<Run>, RunStoreError> {
        self.check()?;
        Ok(lock(&self.runs)
            .values()
            .filter(|run| !matches!(run.state, RunState::Finished { .. }))
            .cloned()
            .collect())
    }

    async fn forget(&self, run: &RunId) -> Result<(), RunStoreError> {
        self.check()?;
        lock(&self.runs).remove(run);
        lock(&self.history).retain(|transition| transition.run != *run);
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use pushos_domain::ids::{NodeId, WorkflowId};
    use pushos_domain::workflow::Outcome;

    use super::*;

    fn run() -> Run {
        Run::beginning(
            WorkflowId::new("build"),
            NodeId::new("plan"),
            None,
            SystemTime::UNIX_EPOCH,
        )
    }

    #[tokio::test]
    async fn a_run_that_has_not_finished_comes_back() {
        let store = FakeRunStore::new();
        store.record(&run()).await.expect("the fake keeps it");

        assert_eq!(store.unfinished().await.expect("no fault").len(), 1);
    }

    #[tokio::test]
    async fn a_finished_run_is_not_something_to_pick_up() {
        let store = FakeRunStore::new();
        let mut done = run();
        done.state = RunState::Finished {
            outcome: Outcome::Succeeded,
        };
        store.record(&done).await.expect("the fake keeps it");

        assert!(store.unfinished().await.expect("no fault").is_empty());
        assert_eq!(store.len(), 1, "it is still written down");
    }

    #[tokio::test]
    async fn the_history_is_kept_in_the_order_it_was_written() {
        let store = FakeRunStore::new();
        let run = run();

        for step in ["plan", "build", "test"] {
            store
                .append(&Transition {
                    run: run.id,
                    from: None,
                    to: NodeId::new(step),
                    note: None,
                    at: SystemTime::UNIX_EPOCH,
                })
                .await
                .expect("the fake keeps it");
        }

        assert_eq!(store.steps_of(&run.id), ["plan", "build", "test"]);
    }
}
