//! Fakes for the workspace ports.
//!
//! Let the manager, the actions above it and the agents below it be exercised
//! with no repository on disk and no database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use pushos_domain::context::WorkspaceMemory;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::WorkspaceId;
use pushos_domain::ports::{Repository, RepositoryError, WorkspaceMemoryStore, Worktree};

/// The simplest possible workspace context: one directory, no isolation.
///
/// What PushOS behaves like when no projects are configured, which is what
/// most of the system should be tested against.
#[derive(Debug, Clone)]
pub struct FixedRoot(PathBuf);

impl FixedRoot {
    /// Builds a context where all work happens in one directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self(root.into())
    }
}

#[async_trait]
impl pushos_domain::ports::WorkspaceContext for FixedRoot {
    async fn root(&self, _workspace: Option<&WorkspaceId>) -> PathBuf {
        self.0.clone()
    }
}

/// Workspace memory that lives only as long as the test.
#[derive(Debug, Clone, Default)]
pub struct FakeWorkspaceMemory {
    kept: Arc<Mutex<HashMap<WorkspaceId, WorkspaceMemory>>>,
}

impl FakeWorkspaceMemory {
    /// Builds a store that has kept nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts something in the store without going through the port.
    pub fn preload(&self, workspace: &WorkspaceId, memory: WorkspaceMemory) {
        lock(&self.kept).insert(workspace.clone(), memory);
    }

    /// How many workspaces have been remembered.
    pub fn len(&self) -> usize {
        lock(&self.kept).len()
    }

    /// Whether anything has been remembered.
    pub fn is_empty(&self) -> bool {
        lock(&self.kept).is_empty()
    }
}

#[async_trait]
impl WorkspaceMemoryStore for FakeWorkspaceMemory {
    async fn recall(&self, workspace: &WorkspaceId) -> Option<WorkspaceMemory> {
        lock(&self.kept).get(workspace).cloned()
    }

    async fn remember(&self, workspace: &WorkspaceId, memory: &WorkspaceMemory) {
        lock(&self.kept).insert(workspace.clone(), memory.clone());
    }
}

/// A repository that exists only in memory.
#[derive(Debug, Clone)]
pub struct FakeRepository {
    trees: Arc<Mutex<Vec<Worktree>>>,
    is_repository: Arc<Mutex<bool>>,
    fail_with: Arc<Mutex<Option<ErrorClass>>>,
}

impl FakeRepository {
    /// Builds a repository whose only tree is its own directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            trees: Arc::new(Mutex::new(vec![Worktree {
                path: root.into(),
                branch: Some("main".to_owned()),
                is_main: true,
            }])),
            is_repository: Arc::new(Mutex::new(true)),
            fail_with: Arc::new(Mutex::new(None)),
        }
    }

    /// Builds something that is not a repository at all.
    pub fn absent() -> Self {
        let fake = Self::new("/tmp/not-a-repository");
        *lock(&fake.is_repository) = false;
        fake
    }

    /// Adds a tree without going through the port.
    pub fn preload(&self, path: impl Into<PathBuf>, branch: &str) {
        lock(&self.trees).push(Worktree {
            path: path.into(),
            branch: Some(branch.to_owned()),
            is_main: false,
        });
    }

    /// Every tree, in the order they were added.
    pub fn worktrees_now(&self) -> Vec<Worktree> {
        lock(&self.trees).clone()
    }

    /// How many trees have been added beyond the repository itself.
    pub fn added(&self) -> usize {
        lock(&self.trees)
            .iter()
            .filter(|tree| !tree.is_main)
            .count()
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        *lock(&self.fail_with) = Some(class);
    }

    fn check(&self) -> Result<(), RepositoryError> {
        match *lock(&self.fail_with) {
            None => Ok(()),
            Some(class) => Err(RepositoryError::backend(
                "the fake was told to fail",
                class,
                std::io::Error::other("injected failure"),
            )),
        }
    }
}

#[async_trait]
impl Repository for FakeRepository {
    async fn is_repository(&self, _root: &Path) -> bool {
        *lock(&self.is_repository)
    }

    async fn worktrees(&self, _root: &Path) -> Result<Vec<Worktree>, RepositoryError> {
        self.check()?;
        Ok(lock(&self.trees).clone())
    }

    async fn add_worktree(
        &self,
        _root: &Path,
        path: &Path,
        branch: &str,
    ) -> Result<Worktree, RepositoryError> {
        self.check()?;
        let tree = Worktree {
            path: path.to_path_buf(),
            branch: Some(branch.to_owned()),
            is_main: false,
        };
        lock(&self.trees).push(tree.clone());
        Ok(tree)
    }
}

/// Takes a lock without turning a panic elsewhere into a panic here.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::PageId;

    use super::*;

    #[tokio::test]
    async fn what_is_remembered_comes_back() {
        let store = FakeWorkspaceMemory::new();
        let workspace = WorkspaceId::new("sydclaw");
        let memory = WorkspaceMemory {
            last_page: Some(PageId::new("development")),
            selected_session: None,
        };

        store.remember(&workspace, &memory).await;
        assert_eq!(store.recall(&workspace).await, Some(memory));
        assert_eq!(store.recall(&WorkspaceId::new("other")).await, None);
    }

    #[tokio::test]
    async fn a_new_repository_has_only_its_own_tree() {
        let repository = FakeRepository::new("/tmp/project");
        let trees = repository
            .worktrees(Path::new("/tmp/project"))
            .await
            .expect("the fake succeeds by default");

        assert_eq!(trees.len(), 1);
        assert!(trees[0].is_main);
        assert_eq!(repository.added(), 0);
    }

    #[tokio::test]
    async fn an_added_tree_appears_in_the_listing() {
        let repository = FakeRepository::new("/tmp/project");
        repository
            .add_worktree(
                Path::new("/tmp/project"),
                Path::new("/tmp/tree"),
                "pushos/one",
            )
            .await
            .expect("the fake succeeds by default");

        assert_eq!(repository.added(), 1);
        assert_eq!(repository.worktrees_now().len(), 2);
    }

    #[tokio::test]
    async fn an_injected_failure_keeps_its_classification() {
        let repository = FakeRepository::new("/tmp/project");
        repository.fail_with(ErrorClass::ComponentFailure);

        let error = repository
            .worktrees(Path::new("/tmp/project"))
            .await
            .expect_err("the fake was told to fail");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }
}
