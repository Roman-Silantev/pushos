//! The workspace ports.
//!
//! Everything downstream of "which project am I in" reaches it through here.
//! The agents crate does not know what a workspace is; it knows it must ask
//! where to work, and something else answers.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::context::WorkspaceMemory;
use crate::ids::{AgentId, ProviderName, WorkspaceId};

/// What the project in effect means for the rest of the system.
#[async_trait]
pub trait WorkspaceContext: Send + Sync + std::fmt::Debug {
    /// Where work happens: for the workspace named, or the one in effect.
    async fn root(&self, workspace: Option<&WorkspaceId>) -> PathBuf;

    /// Where a role should work.
    ///
    /// May hand back an isolated copy of the tree, so that two coding agents
    /// never edit the same working tree at once. What it hands out is given
    /// back by [`Self::release`].
    ///
    /// Claimed by role rather than by session, because a role has one live
    /// session at a time and the tree should outlive any one of them: a builder
    /// restarted tomorrow wants the branch it was working on, not a new one.
    ///
    /// Fails only when a project asked for isolation and PushOS cannot provide
    /// it. A project that did not ask shares its one tree, which is what not
    /// asking means.
    async fn claim(
        &self,
        workspace: Option<&WorkspaceId>,
        agent: &AgentId,
    ) -> Result<PathBuf, ClaimError> {
        let _ = agent;
        Ok(self.root(workspace).await)
    }

    /// Gives back what [`Self::claim`] handed out.
    ///
    /// The tree itself is not destroyed: it may hold work nobody has committed.
    async fn release(&self, workspace: Option<&WorkspaceId>, agent: &AgentId) {
        let _ = (workspace, agent);
    }

    /// The provider a workspace wants for a role, when it names one.
    ///
    /// A role is filled by whichever agent the project prefers, which is why a
    /// binding names the role.
    async fn provider_for(
        &self,
        workspace: Option<&WorkspaceId>,
        agent: &AgentId,
    ) -> Option<ProviderName> {
        let _ = (workspace, agent);
        None
    }

    /// Environment given to everything started in a workspace.
    async fn environment(
        &self,
        workspace: Option<&WorkspaceId>,
    ) -> std::collections::BTreeMap<String, String> {
        let _ = workspace;
        std::collections::BTreeMap::new()
    }
}

/// Why a session could not be given somewhere to work.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClaimError {
    /// Another role is already working there.
    #[error("`{root}` is already being worked in by `{holder}`")]
    InUse {
        /// The tree that is taken.
        root: PathBuf,
        /// Who has it.
        holder: AgentId,
    },

    /// Isolation was asked for and could not be arranged.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ClaimError {
    /// Classifies the failure.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::InUse { .. } => ErrorClass::Validation,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Wraps a fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}

/// Where a workspace's remembered context is kept.
///
/// Separate from the workspace itself: what a project *is* comes from
/// configuration the operator wrote, and what it *was doing* is state PushOS
/// accumulated. Losing the second is an inconvenience; losing the first is not
/// PushOS's to lose.
#[async_trait]
pub trait WorkspaceMemoryStore: Send + Sync + std::fmt::Debug {
    /// What a workspace should restore, if anything was kept.
    async fn recall(&self, workspace: &WorkspaceId) -> Option<WorkspaceMemory>;

    /// Keeps what a workspace should restore next time.
    ///
    /// Failure is not reported: forgetting which page a project was on is not
    /// a reason to refuse to switch to it.
    async fn remember(&self, workspace: &WorkspaceId, memory: &WorkspaceMemory);
}

/// One working tree of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Where it is on disk.
    pub path: PathBuf,
    /// The branch checked out in it, when it is on one.
    pub branch: Option<String>,
    /// Whether this is the repository's own directory rather than an added one.
    pub is_main: bool,
}

/// A version control system, as far as PushOS needs one.
///
/// Only what workspace isolation requires. PushOS does not commit, push, merge
/// or resolve anything: those are decisions, and decisions belong to the
/// operator or to an agent they are watching.
#[async_trait]
pub trait Repository: Send + Sync + std::fmt::Debug {
    /// Whether a directory is a repository at all.
    async fn is_repository(&self, root: &std::path::Path) -> bool;

    /// Every working tree the repository has.
    async fn worktrees(&self, root: &std::path::Path) -> Result<Vec<Worktree>, RepositoryError>;

    /// Adds a working tree on a new branch.
    async fn add_worktree(
        &self,
        root: &std::path::Path,
        path: &std::path::Path,
        branch: &str,
    ) -> Result<Worktree, RepositoryError>;
}

/// Why a repository operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepositoryError {
    /// The directory is not a repository.
    #[error("`{root}` is not a repository")]
    NotARepository {
        /// The directory that was named.
        root: PathBuf,
    },

    /// The version control system reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl RepositoryError {
    /// Classifies the failure.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::NotARepository { .. } => ErrorClass::Validation,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Wraps a backend fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest possible implementation: one directory, no isolation.
    #[derive(Debug)]
    struct OneRoot(PathBuf);

    #[async_trait]
    impl WorkspaceContext for OneRoot {
        async fn root(&self, _workspace: Option<&WorkspaceId>) -> PathBuf {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn a_context_that_only_says_where_still_answers_everything() {
        // The defaults exist so that a PushOS with no workspaces configured
        // needs no special case anywhere downstream.
        let context = OneRoot(PathBuf::from("/tmp/project"));
        let builder = AgentId::new("builder");

        assert_eq!(
            context
                .claim(None, &builder)
                .await
                .expect("nothing is isolated"),
            PathBuf::from("/tmp/project")
        );
        context.release(None, &builder).await;
        assert_eq!(context.provider_for(None, &builder).await, None);
        assert!(context.environment(None).await.is_empty());
    }

    #[test]
    fn a_missing_repository_is_a_validation_error() {
        let error = RepositoryError::NotARepository {
            root: PathBuf::from("/tmp/nope"),
        };
        assert_eq!(error.class(), crate::error::ErrorClass::Validation);
        assert!(error.to_string().contains("/tmp/nope"));
    }

    #[test]
    fn a_worktree_knows_whether_it_is_the_repository_itself() {
        let main = Worktree {
            path: PathBuf::from("/tmp/project"),
            branch: Some("main".to_owned()),
            is_main: true,
        };
        assert!(main.is_main);
    }
}
