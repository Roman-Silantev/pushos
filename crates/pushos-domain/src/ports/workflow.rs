//! The workflow ports.
//!
//! A run outlives the process, so where it is kept is not the engine's to
//! decide, and what it waits for is not the engine's to poll.

use async_trait::async_trait;

use crate::action::{ActionDefinition, ActionResult};
use crate::context::SurfaceContext;
use crate::error::ActionError;
use crate::ids::RunId;
use crate::run::{Run, Transition};

/// Runs one action.
///
/// The seam between a workflow and everything a pad can do. A workflow step
/// goes through the same dispatcher and the same permission check a gesture
/// does, so a workflow can never do something a control could not.
#[async_trait]
pub trait ActionRunner: Send + Sync + std::fmt::Debug {
    /// Carries out one action.
    async fn run(
        &self,
        definition: ActionDefinition,
        surface: SurfaceContext,
    ) -> Result<ActionResult, ActionError>;
}

/// Where runs are kept between steps, and across restarts.
///
/// The contract that makes durability mean anything: a transition is written
/// before the step after it runs. A crash therefore loses at most the step that
/// was in flight, never one that had already been reported as taken.
#[async_trait]
pub trait RunStore: Send + Sync + std::fmt::Debug {
    /// Writes where a run is now.
    ///
    /// Must not return until it is safe to lose the process.
    async fn record(&self, run: &Run) -> Result<(), RunStoreError>;

    /// Appends one step to the run's history.
    async fn append(&self, transition: &Transition) -> Result<(), RunStoreError>;

    /// Every run that had not finished.
    ///
    /// Read at startup. What comes back is what PushOS was in the middle of
    /// when it stopped.
    async fn unfinished(&self) -> Result<Vec<Run>, RunStoreError>;

    /// Forgets a run and its history.
    async fn forget(&self, run: &RunId) -> Result<(), RunStoreError>;
}

/// Why a run could not be written down.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RunStoreError {
    /// The store reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl RunStoreError {
    /// Wraps a fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use crate::ids::{NodeId, WorkflowId};

    use super::*;

    #[derive(Debug, Default)]
    struct Nowhere;

    #[async_trait]
    impl RunStore for Nowhere {
        async fn record(&self, _run: &Run) -> Result<(), RunStoreError> {
            Ok(())
        }
        async fn append(&self, _transition: &Transition) -> Result<(), RunStoreError> {
            Ok(())
        }
        async fn unfinished(&self) -> Result<Vec<Run>, RunStoreError> {
            Ok(Vec::new())
        }
        async fn forget(&self, _run: &RunId) -> Result<(), RunStoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_store_that_keeps_nothing_still_satisfies_the_port() {
        // What PushOS behaves like with no database: workflows run, they just
        // do not survive a restart, and nothing downstream needs a special case.
        let store = Nowhere;
        let run = Run::beginning(
            WorkflowId::new("build"),
            NodeId::new("plan"),
            None,
            SystemTime::UNIX_EPOCH,
        );

        store.record(&run).await.expect("it keeps nothing, happily");
        assert!(store.unfinished().await.expect("no fault").is_empty());
    }

    #[test]
    fn a_fault_says_what_was_being_attempted() {
        let error =
            RunStoreError::backend("recording a run", std::io::Error::other("the disk is full"));
        assert!(error.to_string().contains("recording a run"));
    }
}
