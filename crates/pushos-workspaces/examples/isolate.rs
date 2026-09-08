//! Gives two roles their own working trees in a real repository.
//!
//! ```text
//! cargo run -p pushos-workspaces --example isolate -- <repository> <where to put trees>
//! ```
//!
//! Exists because worktree isolation cannot be proved against a fake: the
//! question is whether the operator's own git does what PushOS asks of it.
#![allow(clippy::expect_used, clippy::print_stdout)]

use std::sync::Arc;

use pushos_domain::context::SurfaceContext;
use pushos_domain::ids::{AgentId, WorkspaceId};
use pushos_domain::ports::{Repository, WorkspaceContext};
use pushos_domain::workspace::{Workspace, WorkspaceTarget};
use pushos_macos::SystemProcessRunner;
use pushos_testkit::FakeWorkspaceMemory;
use pushos_workspaces::{GitRepository, WorkspaceManager, WorkspaceRegistry};

#[tokio::main]
async fn main() {
    let root = std::env::args().nth(1).expect("a repository to work in");
    let trees = std::env::args().nth(2).expect("somewhere to put the trees");

    let processes = Arc::new(SystemProcessRunner::new());
    let git = Arc::new(GitRepository::new(processes));
    println!(
        "repository: {}",
        git.is_repository(std::path::Path::new(&root)).await
    );

    let mut project = Workspace::new("demo", "Demo", &root);
    project.isolate_agents = true;

    let manager = WorkspaceManager::new(
        WorkspaceRegistry::new([project]),
        Arc::new(FakeWorkspaceMemory::new()),
        Arc::clone(&git) as Arc<dyn Repository>,
        &root,
        &trees,
    );
    manager
        .select(
            &WorkspaceTarget::Named(WorkspaceId::new("demo")),
            &SurfaceContext::empty(),
        )
        .await
        .expect("it is configured");

    for role in ["builder", "reviewer"] {
        let agent = AgentId::new(role);
        match manager.claim(None, &agent).await {
            Ok(path) => println!("{role}: {}", path.display()),
            Err(error) => println!("{role}: FAILED {error}"),
        }
    }

    println!("leased: {}", manager.leased().await);
    for tree in git
        .worktrees(std::path::Path::new(&root))
        .await
        .expect("the repository lists its trees")
    {
        println!(
            "  tree {} on {} (main: {})",
            tree.path.display(),
            tree.branch.as_deref().unwrap_or("(detached)"),
            tree.is_main
        );
    }
}
