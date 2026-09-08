//! Workspace memory, kept in the database.
//!
//! What a project *is* comes from configuration the operator wrote. What it
//! *was doing* is state PushOS accumulated, and belongs here. Losing this is an
//! inconvenience; losing the configuration would not be PushOS's to lose.

use async_trait::async_trait;
use pushos_domain::context::WorkspaceMemory;
use pushos_domain::ids::{PageId, SessionId, WorkspaceId};
use pushos_domain::ports::WorkspaceMemoryStore;
use tracing::warn;

use crate::records::WorkspaceMemoryRow;
use crate::writer::Storage;

#[async_trait]
impl WorkspaceMemoryStore for Storage {
    async fn recall(&self, workspace: &WorkspaceId) -> Option<WorkspaceMemory> {
        match self.recall_workspace(workspace.as_str()).await {
            Ok(row) => row.map(|row| WorkspaceMemory {
                last_page: row.last_page.as_deref().map(PageId::new),
                selected_session: row.selected_session.as_deref().map(SessionId::new),
            }),
            Err(error) => {
                // Not being able to read what a project was doing is no reason
                // to refuse to go to it.
                warn!(%workspace, %error, "could not recall a workspace");
                None
            }
        }
    }

    async fn remember(&self, workspace: &WorkspaceId, memory: &WorkspaceMemory) {
        let row = WorkspaceMemoryRow {
            workspace_id: workspace.to_string(),
            last_page: memory.last_page.as_ref().map(ToString::to_string),
            selected_session: memory.selected_session.as_ref().map(ToString::to_string),
        };

        if let Err(error) = self.remember_workspace(row) {
            warn!(%workspace, %error, "could not remember a workspace");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::StorageWriter;

    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pushos-memory-{}.sqlite",
            pushos_domain::ids::ExecutionId::generate()
        ))
    }

    #[tokio::test]
    async fn what_a_project_was_doing_survives_being_written_and_read_back() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();
        let workspace = WorkspaceId::new("sydclaw");

        let memory = WorkspaceMemory {
            last_page: Some(PageId::new("development")),
            selected_session: Some(SessionId::new("term-abc")),
        };
        storage.remember(&workspace, &memory).await;

        assert_eq!(storage.recall(&workspace).await, Some(memory));
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn a_project_nothing_was_kept_for_recalls_nothing() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");

        assert_eq!(writer.handle().recall(&WorkspaceId::new("new")).await, None);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn remembering_twice_keeps_the_second() {
        let path = scratch();
        let writer = StorageWriter::open(&path).expect("a fresh database opens");
        let storage = writer.handle();
        let workspace = WorkspaceId::new("sydclaw");

        storage
            .remember(
                &workspace,
                &WorkspaceMemory {
                    last_page: Some(PageId::new("home")),
                    selected_session: None,
                },
            )
            .await;
        storage
            .remember(
                &workspace,
                &WorkspaceMemory {
                    last_page: Some(PageId::new("music")),
                    selected_session: None,
                },
            )
            .await;

        assert_eq!(
            storage
                .recall(&workspace)
                .await
                .and_then(|kept| kept.last_page),
            Some(PageId::new("music"))
        );
        std::fs::remove_file(&path).ok();
    }
}
