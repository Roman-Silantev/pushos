//! Workspace memory that keeps nothing.
//!
//! For building a manager where there is no database to write to: listing what
//! PushOS can do, checking a configuration, and anything else that must not
//! touch the operator's state to answer a question about it.

use async_trait::async_trait;
use pushos_domain::context::WorkspaceMemory;
use pushos_domain::ids::WorkspaceId;
use pushos_domain::ports::WorkspaceMemoryStore;

/// A store that remembers nothing and says so by returning nothing.
#[derive(Debug, Clone, Copy)]
pub struct ForgetfulMemory;

#[async_trait]
impl WorkspaceMemoryStore for ForgetfulMemory {
    async fn recall(&self, _workspace: &WorkspaceId) -> Option<WorkspaceMemory> {
        None
    }

    async fn remember(&self, _workspace: &WorkspaceId, _memory: &WorkspaceMemory) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nothing_is_kept_and_nothing_comes_back() {
        let store = ForgetfulMemory;
        let workspace = WorkspaceId::new("sydclaw");

        store
            .remember(&workspace, &WorkspaceMemory::default())
            .await;
        assert_eq!(store.recall(&workspace).await, None);
    }
}
