//! The single owner of which project is in effect.
//!
//! Selecting a workspace is one gesture with several consequences: the surface
//! moves, what the previous project was doing is remembered, what this one was
//! doing is restored, and everything started afterwards happens in its
//! directory. All of that is sequenced here so that no two parts of PushOS can
//! disagree about where they are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::context::{SurfaceContext, WorkspaceMemory};
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::{AgentId, PageId, ProviderName, WorkspaceId};
use pushos_domain::ports::{
    ClaimError, Repository, WorkspaceContext, WorkspaceMemoryStore, Worktree,
};
use pushos_domain::workspace::{Workspace, WorkspaceTarget};
use tokio::sync::{Mutex, watch};
use tracing::{debug, info, warn};

use crate::leases::{Holder, WorktreeLeases};
use crate::registry::{UnknownWorkspace, WorkspaceRegistry};

/// The branch prefix PushOS puts its own working trees on.
///
/// Namespaced so that a tree PushOS made is never mistaken for one the operator
/// made, and so that `git branch --list pushos/*` says exactly what PushOS has
/// been doing.
const BRANCH_PREFIX: &str = "pushos";

/// Where the operator ends up, and what the surface should do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The workspace now in effect, or none if they left one.
    pub workspace: Option<WorkspaceId>,
    /// What it is called, for the display.
    pub name: Option<String>,
    /// The page it restored, when it had one to restore.
    pub page: Option<PageId>,
}

/// Owns the current project and everything that follows from it.
#[derive(Debug)]
pub struct WorkspaceManager {
    registry: WorkspaceRegistry,
    memory: Arc<dyn WorkspaceMemoryStore>,
    repository: Arc<dyn Repository>,
    /// Where work happens when no project is in effect.
    fallback_root: PathBuf,
    /// Where isolated working trees are put.
    ///
    /// Outside the operator's repository on purpose: PushOS does not leave
    /// directories inside a project the operator did not put there.
    worktree_root: PathBuf,
    state: Mutex<State>,
    /// What is in effect, published so the surface follows rather than keeping
    /// a second answer of its own. One owner, one value.
    announced: watch::Sender<Option<WorkspaceId>>,
}

#[derive(Debug, Default)]
struct State {
    current: Option<WorkspaceId>,
    leases: WorktreeLeases,
}

impl WorkspaceManager {
    /// Builds a manager over the configured workspaces.
    pub fn new(
        registry: WorkspaceRegistry,
        memory: Arc<dyn WorkspaceMemoryStore>,
        repository: Arc<dyn Repository>,
        fallback_root: impl Into<PathBuf>,
        worktree_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            registry,
            memory,
            repository,
            fallback_root: fallback_root.into(),
            worktree_root: worktree_root.into(),
            state: Mutex::new(State::default()),
            announced: watch::channel(None).0,
        }
    }

    /// Follows the project in effect.
    ///
    /// The surface subscribes to this rather than tracking its own, so the two
    /// can never disagree about which project the operator is in.
    pub fn subscribe(&self) -> watch::Receiver<Option<WorkspaceId>> {
        self.announced.subscribe()
    }

    /// The configured projects.
    pub fn registry(&self) -> &WorkspaceRegistry {
        &self.registry
    }

    /// The project in effect.
    pub async fn current(&self) -> Option<WorkspaceId> {
        self.state.lock().await.current.clone()
    }

    /// The project in effect, in full.
    pub async fn current_workspace(&self) -> Option<&Workspace> {
        let current = self.current().await?;
        self.registry.get(&current)
    }

    /// Moves to a project.
    ///
    /// `leaving` is where the operator is now, which is what gets remembered
    /// for the project they are leaving.
    pub async fn select(
        &self,
        target: &WorkspaceTarget,
        leaving: &SurfaceContext,
    ) -> Result<Arrival, UnknownWorkspace> {
        let here = self.current().await;
        let chosen = self
            .registry
            .resolve(target, here.as_ref())?
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    workspace.name.clone(),
                    workspace.home_page.clone(),
                )
            });

        // Remembered before moving, so that what is kept is what the operator
        // was actually doing rather than where they went next.
        if let Some(previous) = &here
            && Some(previous) != chosen.as_ref().map(|(id, _, _)| id)
        {
            self.remember(previous, leaving).await;
        }

        let Some((id, name, home)) = chosen else {
            self.state.lock().await.current = None;
            info!("left the workspace");
            return Ok(Arrival {
                workspace: None,
                name: None,
                page: None,
            });
        };

        let restored = self
            .memory
            .recall(&id)
            .await
            .and_then(|kept| kept.last_page);
        // What it was doing wins over where it starts: an operator returning to
        // a project wants to be where they left off.
        let page = restored.or(home);

        self.state.lock().await.current = Some(id.clone());
        self.announced.send_replace(Some(id.clone()));
        info!(workspace = %id, page = ?page, "workspace selected");

        Ok(Arrival {
            workspace: Some(id),
            name: Some(name),
            page,
        })
    }

    /// Keeps what a project is doing now, so returning to it restores this.
    pub async fn remember(&self, workspace: &WorkspaceId, context: &SurfaceContext) {
        let memory = WorkspaceMemory {
            last_page: context.page.clone(),
            selected_session: context.session.clone(),
        };
        debug!(%workspace, page = ?memory.last_page, "remembering a workspace");
        self.memory.remember(workspace, &memory).await;
    }

    /// Keeps what the project in effect is doing, if one is.
    ///
    /// Called when PushOS stops, so a restart lands where the operator left.
    pub async fn remember_current(&self, context: &SurfaceContext) {
        if let Some(current) = self.current().await {
            self.remember(&current, context).await;
        }
    }

    /// How many trees are being worked in.
    pub async fn leased(&self) -> usize {
        self.state.lock().await.leases.len()
    }

    /// The workspace a target names, without moving to it.
    fn workspace_for(
        &self,
        named: Option<&WorkspaceId>,
        current: Option<&WorkspaceId>,
    ) -> Option<&Workspace> {
        named.or(current).and_then(|id| self.registry.get(id))
    }

    /// Finds or makes a tree for a role, and records that it has it.
    async fn isolate(&self, workspace: &Workspace, agent: &AgentId) -> Result<PathBuf, ClaimError> {
        let holder = Holder::new(&workspace.id, agent);

        if !self.repository.is_repository(&workspace.root).await {
            return Err(ClaimError::backend(
                format!(
                    "`{}` is not a repository, so `{}` cannot give its agents separate trees",
                    workspace.root.display(),
                    workspace.id
                ),
                ErrorClass::Validation,
                std::io::Error::other("not a repository"),
            ));
        }

        // Listed outside the lock, because it runs a subprocess.
        let existing = self
            .repository
            .worktrees(&workspace.root)
            .await
            .map_err(|error| {
                let class = error.class();
                ClaimError::backend(error.to_string(), class, error)
            })?;

        if let Some(reused) = self.reuse(&existing, &holder).await {
            debug!(%holder, path = %reused.display(), "working in a tree that was free");
            return Ok(reused);
        }

        let path = self
            .worktree_root
            .join(workspace.id.as_str())
            .join(agent.as_str());
        let branch = format!("{BRANCH_PREFIX}/{holder}");

        let tree = self
            .repository
            .add_worktree(&workspace.root, &path, &branch)
            .await
            .map_err(|error| {
                let class = error.class();
                ClaimError::backend(error.to_string(), class, error)
            })?;

        let mut state = self.state.lock().await;
        state.leases.take(&holder, &tree.path);
        info!(%holder, path = %tree.path.display(), "gave a role its own tree");
        Ok(tree.path)
    }

    /// Takes a tree PushOS made earlier that nobody is working in.
    ///
    /// Reused rather than accumulated: every role in every project would
    /// otherwise leave a checkout and a branch behind for ever.
    async fn reuse(&self, existing: &[Worktree], holder: &Holder) -> Option<PathBuf> {
        let mut state = self.state.lock().await;

        // Whatever this role already had, first: asking twice is not a reason
        // to move it.
        if let Some(held) = state.leases.held_by(holder) {
            return Some(held.to_path_buf());
        }

        let free = existing.iter().find(|tree| {
            !tree.is_main
                && tree
                    .branch
                    .as_ref()
                    .is_some_and(|branch| branch.starts_with(BRANCH_PREFIX))
                && state.leases.is_free(&tree.path)
        })?;

        state.leases.take(holder, &free.path);
        Some(free.path.clone())
    }
}

#[async_trait]
impl WorkspaceContext for WorkspaceManager {
    async fn root(&self, workspace: Option<&WorkspaceId>) -> PathBuf {
        let current = self.current().await;
        self.workspace_for(workspace, current.as_ref())
            .map_or_else(|| self.fallback_root.clone(), |found| found.root.clone())
    }

    async fn claim(
        &self,
        workspace: Option<&WorkspaceId>,
        agent: &AgentId,
    ) -> Result<PathBuf, ClaimError> {
        let current = self.current().await;
        let Some(found) = self.workspace_for(workspace, current.as_ref()) else {
            // No project, no isolation to arrange: everything shares the one
            // directory PushOS was started in.
            return Ok(self.fallback_root.clone());
        };

        if !found.isolate_agents {
            // Not asking for separate trees is a decision, and it means the
            // agents share this one.
            return Ok(found.root.clone());
        }

        self.isolate(found, agent).await
    }

    async fn release(&self, workspace: Option<&WorkspaceId>, agent: &AgentId) {
        let current = self.current().await;
        let Some(found) = self.workspace_for(workspace, current.as_ref()) else {
            return;
        };

        let holder = Holder::new(&found.id, agent);
        let mut state = self.state.lock().await;
        if let Some(path) = state.leases.release(&holder) {
            // The tree is left where it is. It may hold work nobody has
            // committed, and removing that to tidy up would be the worst thing
            // PushOS could do.
            debug!(%holder, path = %path.display(), "gave back a working tree");
        }
    }

    async fn provider_for(
        &self,
        workspace: Option<&WorkspaceId>,
        agent: &AgentId,
    ) -> Option<ProviderName> {
        let current = self.current().await;
        self.workspace_for(workspace, current.as_ref())
            .and_then(|found| found.provider_for(agent))
            .cloned()
    }

    async fn environment(&self, workspace: Option<&WorkspaceId>) -> BTreeMap<String, String> {
        let current = self.current().await;
        self.workspace_for(workspace, current.as_ref())
            .map(|found| found.env.clone())
            .unwrap_or_default()
    }
}

/// Reports a project that could not be opened in an application.
pub fn describe_missing_app(workspace: &Workspace, app: &str) -> String {
    let known: Vec<&str> = workspace.apps.keys().map(String::as_str).collect();
    if known.is_empty() {
        format!("`{}` has no applications configured", workspace.id)
    } else {
        format!(
            "`{}` has no application called `{app}`; it has {}",
            workspace.id,
            known.join(", ")
        )
    }
}

/// Warns once about a directory a project points at that is not there.
pub fn warn_if_missing(workspace: &Workspace) {
    if !Path::new(&workspace.root).is_dir() {
        warn!(
            workspace = %workspace.id,
            root = %workspace.root.display(),
            "the project directory is not there; agents and terminals will refuse to start"
        );
    }
}

#[cfg(test)]
mod tests {
    use pushos_testkit::{FakeRepository, FakeWorkspaceMemory};

    use super::*;

    struct Fixture {
        manager: WorkspaceManager,
        memory: FakeWorkspaceMemory,
        repository: FakeRepository,
    }

    fn workspaces() -> Vec<Workspace> {
        let mut sydclaw = Workspace::new("sydclaw", "Sydclaw", "/tmp/sydclaw");
        sydclaw.home_page = Some(PageId::new("development"));
        sydclaw
            .roles
            .insert(AgentId::new("builder"), ProviderName::new("claude"));
        sydclaw
            .env
            .insert("PROJECT".to_owned(), "sydclaw".to_owned());

        let mut pushos = Workspace::new("pushos", "PushOS", "/tmp/pushos");
        pushos.isolate_agents = true;

        vec![sydclaw, pushos]
    }

    impl Fixture {
        fn new() -> Self {
            let memory = FakeWorkspaceMemory::new();
            let repository = FakeRepository::new("/tmp/pushos");
            Self {
                manager: WorkspaceManager::new(
                    WorkspaceRegistry::new(workspaces()),
                    Arc::new(memory.clone()),
                    Arc::new(repository.clone()),
                    "/tmp/nowhere",
                    "/tmp/trees",
                ),
                memory,
                repository,
            }
        }

        async fn select(&self, name: &str, from: SurfaceContext) -> Arrival {
            self.manager
                .select(&WorkspaceTarget::Named(WorkspaceId::new(name)), &from)
                .await
                .expect("the workspace is configured")
        }
    }

    fn on(page: &str) -> SurfaceContext {
        SurfaceContext::empty().on_page(page)
    }

    #[tokio::test]
    async fn selecting_a_project_makes_it_the_one_in_effect() {
        let fixture = Fixture::new();
        let arrival = fixture.select("sydclaw", SurfaceContext::empty()).await;

        assert_eq!(arrival.workspace, Some(WorkspaceId::new("sydclaw")));
        assert_eq!(arrival.name.as_deref(), Some("Sydclaw"));
        assert_eq!(
            fixture.manager.current().await,
            Some(WorkspaceId::new("sydclaw"))
        );
    }

    #[tokio::test]
    async fn a_project_arrives_on_its_home_page_the_first_time() {
        let fixture = Fixture::new();
        let arrival = fixture.select("sydclaw", SurfaceContext::empty()).await;
        assert_eq!(arrival.page, Some(PageId::new("development")));
    }

    #[tokio::test]
    async fn returning_to_a_project_restores_where_it_was_left() {
        // The whole point of remembering: a pad puts the operator back where
        // they were, not back at the beginning.
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        // They wandered to another page, then left for another project.
        fixture
            .select("pushos", on("music").in_workspace("sydclaw"))
            .await;
        let back = fixture
            .select("sydclaw", on("home").in_workspace("pushos"))
            .await;

        assert_eq!(back.page, Some(PageId::new("music")));
    }

    #[tokio::test]
    async fn what_was_remembered_beats_where_the_project_starts() {
        let fixture = Fixture::new();
        fixture.memory.preload(
            &WorkspaceId::new("sydclaw"),
            WorkspaceMemory {
                last_page: Some(PageId::new("music")),
                selected_session: None,
            },
        );

        let arrival = fixture.select("sydclaw", SurfaceContext::empty()).await;
        assert_eq!(
            arrival.page,
            Some(PageId::new("music")),
            "the home page is where it starts, not where it was"
        );
    }

    #[tokio::test]
    async fn selecting_the_project_already_in_effect_does_not_overwrite_its_memory() {
        // Pressing the pad twice must not record the page as wherever the
        // second press happened to find them.
        let fixture = Fixture::new();
        fixture.memory.preload(
            &WorkspaceId::new("sydclaw"),
            WorkspaceMemory {
                last_page: Some(PageId::new("music")),
                selected_session: None,
            },
        );
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        let again = fixture
            .select("sydclaw", on("development").in_workspace("sydclaw"))
            .await;
        assert_eq!(again.page, Some(PageId::new("music")));
    }

    #[tokio::test]
    async fn leaving_a_project_keeps_what_it_was_doing() {
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        let left = fixture
            .manager
            .select(&WorkspaceTarget::None, &on("music").in_workspace("sydclaw"))
            .await
            .expect("leaving is always possible");

        assert_eq!(left.workspace, None);
        assert_eq!(fixture.manager.current().await, None);
        assert_eq!(
            fixture
                .memory
                .recall(&WorkspaceId::new("sydclaw"))
                .await
                .and_then(|kept| kept.last_page),
            Some(PageId::new("music"))
        );
    }

    #[tokio::test]
    async fn naming_a_project_nothing_configures_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .manager
            .select(
                &WorkspaceTarget::Named(WorkspaceId::new("nowhere")),
                &SurfaceContext::empty(),
            )
            .await
            .expect_err("nothing configures it");
        assert!(error.to_string().contains("nowhere"));
    }

    #[tokio::test]
    async fn work_happens_in_the_project_in_effect() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.manager.root(None).await,
            PathBuf::from("/tmp/nowhere"),
            "with no project, work happens where PushOS was started"
        );

        fixture.select("sydclaw", SurfaceContext::empty()).await;
        assert_eq!(
            fixture.manager.root(None).await,
            PathBuf::from("/tmp/sydclaw")
        );
    }

    #[tokio::test]
    async fn naming_a_project_beats_the_one_in_effect() {
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        assert_eq!(
            fixture
                .manager
                .root(Some(&WorkspaceId::new("pushos")))
                .await,
            PathBuf::from("/tmp/pushos")
        );
    }

    #[tokio::test]
    async fn a_project_that_shares_its_tree_gives_every_agent_the_same_one() {
        // Not asking for separate trees is a decision, and this is what it
        // means.
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        let first = fixture
            .manager
            .claim(None, &AgentId::new("builder"))
            .await
            .expect("shared");
        let second = fixture
            .manager
            .claim(None, &AgentId::new("reviewer"))
            .await
            .expect("shared");
        assert_eq!(first, second);
        assert_eq!(fixture.repository.added(), 0);
    }

    #[tokio::test]
    async fn a_project_that_isolates_gives_every_agent_its_own_tree() {
        let fixture = Fixture::new();
        fixture.select("pushos", SurfaceContext::empty()).await;

        let first = fixture
            .manager
            .claim(None, &AgentId::new("builder"))
            .await
            .expect("isolated");
        let second = fixture
            .manager
            .claim(None, &AgentId::new("reviewer"))
            .await
            .expect("isolated");

        assert_ne!(first, second, "two agents must never share a tree");
        assert_ne!(
            first,
            PathBuf::from("/tmp/pushos"),
            "nor use the repository itself"
        );
        assert_eq!(fixture.repository.added(), 2);
        assert_eq!(fixture.manager.leased().await, 2);
    }

    #[tokio::test]
    async fn a_role_asking_twice_gets_the_tree_it_already_has() {
        // Restarting the builder tomorrow should give it back the branch it
        // was working on, not a new one.
        let fixture = Fixture::new();
        fixture.select("pushos", SurfaceContext::empty()).await;

        let builder = AgentId::new("builder");
        let first = fixture
            .manager
            .claim(None, &builder)
            .await
            .expect("isolated");
        let again = fixture
            .manager
            .claim(None, &builder)
            .await
            .expect("isolated");

        assert_eq!(first, again);
        assert_eq!(fixture.repository.added(), 1, "no second tree was made");
    }

    #[tokio::test]
    async fn a_tree_given_back_is_used_again_rather_than_left_to_accumulate() {
        let fixture = Fixture::new();
        fixture.select("pushos", SurfaceContext::empty()).await;

        let builder = AgentId::new("builder");
        let first = fixture
            .manager
            .claim(None, &builder)
            .await
            .expect("isolated");
        fixture.manager.release(None, &builder).await;
        let second = fixture
            .manager
            .claim(None, &AgentId::new("reviewer"))
            .await
            .expect("isolated");

        assert_eq!(first, second, "the free tree was taken again");
        assert_eq!(fixture.repository.added(), 1);
    }

    #[tokio::test]
    async fn a_tree_given_back_is_not_deleted() {
        // It may hold work nobody has committed.
        let fixture = Fixture::new();
        fixture.select("pushos", SurfaceContext::empty()).await;

        fixture
            .manager
            .claim(None, &AgentId::new("builder"))
            .await
            .expect("isolated");
        fixture
            .manager
            .release(None, &AgentId::new("builder"))
            .await;

        assert_eq!(fixture.repository.added(), 1, "the tree is still there");
        assert_eq!(fixture.manager.leased().await, 0);
    }

    #[tokio::test]
    async fn a_project_that_wants_isolation_from_something_that_is_not_a_repository_refuses() {
        // Handing back the shared tree instead would silently give up the one
        // guarantee the project asked for.
        let mut isolated = Workspace::new("loose", "Loose", "/tmp/loose");
        isolated.isolate_agents = true;

        let manager = WorkspaceManager::new(
            WorkspaceRegistry::new([isolated]),
            Arc::new(FakeWorkspaceMemory::new()),
            Arc::new(FakeRepository::absent()),
            "/tmp/nowhere",
            "/tmp/trees",
        );
        manager
            .select(
                &WorkspaceTarget::Named(WorkspaceId::new("loose")),
                &SurfaceContext::empty(),
            )
            .await
            .expect("it is configured");

        let error = manager
            .claim(None, &AgentId::new("builder"))
            .await
            .expect_err("it is not a repository");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(error.to_string().contains("/tmp/loose"), "{error}");
    }

    #[tokio::test]
    async fn a_git_failure_while_isolating_is_reported_rather_than_papered_over() {
        let fixture = Fixture::new();
        fixture.select("pushos", SurfaceContext::empty()).await;
        fixture.repository.fail_with(ErrorClass::ComponentFailure);

        let error = fixture
            .manager
            .claim(None, &AgentId::new("builder"))
            .await
            .expect_err("git failed");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }

    #[tokio::test]
    async fn a_project_names_the_provider_it_wants_for_a_role() {
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        assert_eq!(
            fixture
                .manager
                .provider_for(None, &AgentId::new("builder"))
                .await,
            Some(ProviderName::new("claude"))
        );
        assert_eq!(
            fixture
                .manager
                .provider_for(None, &AgentId::new("reviewer"))
                .await,
            None,
            "a role the project says nothing about falls back to the global preference"
        );
    }

    #[tokio::test]
    async fn a_project_hands_its_environment_to_what_it_starts() {
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;

        let env = fixture.manager.environment(None).await;
        assert_eq!(env.get("PROJECT").map(String::as_str), Some("sydclaw"));
        assert!(
            fixture
                .manager
                .environment(Some(&WorkspaceId::new("pushos")))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stepping_moves_through_the_configured_order() {
        let fixture = Fixture::new();
        let first = fixture
            .manager
            .select(&WorkspaceTarget::Next, &SurfaceContext::empty())
            .await
            .expect("something is configured");
        assert_eq!(first.workspace, Some(WorkspaceId::new("sydclaw")));

        let second = fixture
            .manager
            .select(&WorkspaceTarget::Next, &first_context(&first))
            .await
            .expect("something is configured");
        assert_eq!(second.workspace, Some(WorkspaceId::new("pushos")));
    }

    fn first_context(arrival: &Arrival) -> SurfaceContext {
        let mut context = SurfaceContext::empty();
        context.workspace = arrival.workspace.clone();
        context.page = arrival.page.clone();
        context
    }

    #[tokio::test]
    async fn what_the_current_project_is_doing_is_kept_when_pushos_stops() {
        let fixture = Fixture::new();
        fixture.select("sydclaw", SurfaceContext::empty()).await;
        fixture
            .manager
            .remember_current(&on("music").in_workspace("sydclaw"))
            .await;

        assert_eq!(
            fixture
                .memory
                .recall(&WorkspaceId::new("sydclaw"))
                .await
                .and_then(|kept| kept.last_page),
            Some(PageId::new("music"))
        );
    }

    #[tokio::test]
    async fn with_nothing_in_effect_there_is_nothing_to_keep() {
        let fixture = Fixture::new();
        fixture.manager.remember_current(&on("music")).await;
        assert!(fixture.memory.is_empty());
    }
}
