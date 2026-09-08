//! A run store that keeps nothing.
//!
//! For building an engine where there is no database to write to: listing what
//! PushOS can do, checking a configuration, and anything else that must not
//! touch the operator's state to answer a question about it.

use async_trait::async_trait;
use pushos_domain::ids::RunId;
use pushos_domain::ports::{RunStore, RunStoreError};
use pushos_domain::run::{Run, Transition};

/// A store that remembers nothing and says so by returning nothing.
#[derive(Debug, Clone, Copy)]
pub struct ForgetfulRuns;

#[async_trait]
impl RunStore for ForgetfulRuns {
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

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use pushos_domain::ids::{NodeId, WorkflowId};

    use super::*;

    #[tokio::test]
    async fn nothing_is_kept_and_nothing_comes_back() {
        let store = ForgetfulRuns;
        let run = Run::beginning(
            WorkflowId::new("ship"),
            NodeId::new("plan"),
            None,
            SystemTime::UNIX_EPOCH,
        );

        store.record(&run).await.expect("it keeps nothing, happily");
        assert!(store.unfinished().await.expect("no fault").is_empty());
    }
}
