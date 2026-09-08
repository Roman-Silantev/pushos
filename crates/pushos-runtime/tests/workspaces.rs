//! Projects, driven through the surface.
//!
//! These prove the claim Phase 7 makes: one pad represents an entire coding
//! project. Pressing it moves the surface, brings the project's bindings into
//! force, restores the page it was last on, and points what starts afterwards
//! at its directory.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_actions::providers::{page, terminal, workspace};
use pushos_config::{ConfigFile, ConfigStore, RuntimeConfig};
use pushos_domain::controls::PadIndex;
use pushos_domain::event::DomainEvent;
use pushos_domain::ids::WorkspaceId;
use pushos_domain::ports::{PushOutput, WorkspaceContext};
use pushos_runtime::{Runtime, Shutdown, TerminalReporter};
use pushos_terminal::TerminalSupervisor;
use pushos_testkit::{
    FakeApplications, FakePush, FakeRepository, FakeTerminal, FakeWorkspaceMemory,
};
use pushos_ui::PushRenderer;
use pushos_workspaces::{WorkspaceManager, WorkspaceRegistry};

const CONFIG: &str = r#"
[runtime]
home_page = "home"

[permissions]
granted = ["shell.execute"]

[[pages]]
id = "home"
name = "Home"

[[pages]]
id = "work"
name = "Work"

[[pages]]
id = "music"
name = "Music"

[[workspaces]]
id = "alpha"
name = "Alpha"
root = "/tmp/pushos-alpha"
home_page = "work"

[[workspaces]]
id = "beta"
name = "Beta"
root = "/tmp/pushos-beta"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "workspace.select"
target = "alpha"
label = "Alpha"

[[bindings]]
control = "pad.1"
gesture = "tap"
action = "workspace.select"
target = "beta"
label = "Beta"

[[bindings]]
control = "pad.2"
gesture = "tap"
action = "page.show"
target = "music"

[[bindings]]
control = "pad.3"
gesture = "tap"
workspace = "alpha"
action = "page.show"
target = "work"
label = "Alpha only"

[[bindings]]
control = "pad.4"
gesture = "tap"
action = "terminal.open"
label = "Shell"
[bindings.params]
name = "shell"
"#;

fn store(text: &str) -> Arc<ConfigStore> {
    let parsed: ConfigFile = toml::from_str(text).expect("the test configuration parses");
    RuntimeConfig::build(&parsed).expect("the test configuration is valid");

    let directory = std::env::temp_dir().join(format!(
        "pushos-ws-{}",
        pushos_domain::ids::ExecutionId::generate()
    ));
    std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
    std::fs::write(directory.join("pushos.toml"), text).expect("writable");
    Arc::new(ConfigStore::load(&directory).expect("the configuration is valid"))
}

fn pad(index: u8) -> PadIndex {
    PadIndex::new(index).expect("test pad index is in range")
}

/// A running PushOS with projects, backed by fakes throughout.
///
/// Where the operator is comes from the event bus, which is how PushOS itself
/// records it: the surface publishes a page change and a project change, and
/// the test follows the same trail an audit would.
struct Harness {
    surface: FakePush,
    manager: Arc<WorkspaceManager>,
    terminals: FakeTerminal,
    events: pushos_runtime::EventSubscription,
    shutdown: Shutdown,
    finished: tokio::task::JoinHandle<()>,
    page: Option<String>,
    workspace: Option<String>,
}

impl Harness {
    fn start() -> Self {
        let config = store(CONFIG);
        let current = config.current();

        let manager = Arc::new(WorkspaceManager::new(
            WorkspaceRegistry::new(current.workspaces.clone()),
            Arc::new(FakeWorkspaceMemory::new()),
            Arc::new(FakeRepository::new("/tmp/pushos-alpha")),
            "/tmp/pushos-nowhere",
            "/tmp/pushos-trees",
        ));

        let (reporter, terminal_updates) = TerminalReporter::new();
        let terminals = FakeTerminal::new(Arc::new(reporter));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(terminals.clone()),
            Arc::clone(&manager) as Arc<dyn WorkspaceContext>,
        ));

        let (surface, input) = FakePush::new();
        let shutdown = Shutdown::new();

        let runtime = Runtime::new(Arc::clone(&config))
            .with_workspaces(Arc::clone(&manager))
            .with_terminals(Arc::clone(&supervisor), terminal_updates)
            .with_provider(Arc::new(page::PageProvider::new()))
            .expect("the namespace is free")
            .with_provider(Arc::new(workspace::WorkspaceProvider::new(
                Arc::clone(&manager),
                Arc::new(FakeApplications::new()),
            )))
            .expect("the namespace is free")
            .with_provider(Arc::new(terminal::TerminalProvider::new(supervisor)))
            .expect("the namespace is free");

        // The view is taken from a second runtime construction path: the
        // runtime publishes it internally, so the test reads the surface
        // through the same channel the renderer does.
        let events = runtime.bus().subscribe();
        let output: Arc<dyn PushOutput> = Arc::new(surface.clone());
        let renderer = PushRenderer::new().expect("the renderer builds");
        let running = shutdown.clone();
        let finished = tokio::spawn(async move {
            runtime
                .run(Box::new(input), output, renderer, running)
                .await;
        });

        Self {
            surface,
            manager,
            terminals,
            events,
            shutdown,
            finished,
            page: Some("home".to_owned()),
            workspace: None,
        }
    }

    async fn tap(&self, index: u8) {
        let at = Instant::now();
        self.surface.press_pad(pad(index), at).await;
        self.surface.release_pad(pad(index), at).await;
    }

    /// Follows the trail until the surface is where the test expects.
    async fn settles_on(&mut self, workspace: Option<&str>, page: &str) {
        let wanted = workspace.map(ToOwned::to_owned);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        while self.workspace != wanted || self.page.as_deref() != Some(page) {
            let next = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "the surface settled on {:?} / {:?}, not {wanted:?} / {page}",
                        self.workspace, self.page
                    )
                });

            let Some(envelope) = next else {
                panic!("the runtime stopped reporting");
            };

            self.note(&envelope.payload);
        }
    }

    /// Drains what has arrived so far, without waiting for more.
    async fn catch_up(&mut self) {
        // Everything already queued, and nothing that has not arrived yet.
        while let Ok(Some(envelope)) =
            tokio::time::timeout(Duration::from_millis(20), self.events.recv()).await
        {
            self.note(&envelope.payload);
        }
    }

    fn note(&mut self, event: &DomainEvent) {
        match event {
            DomainEvent::PageChanged { page } => self.page = Some(page.to_string()),
            DomainEvent::WorkspaceChanged { workspace } => {
                self.workspace = workspace.as_ref().map(ToString::to_string);
            }
            _ => {}
        }
    }

    async fn stop(self) {
        self.shutdown.stop().await;
        let _ = self.finished.await;
    }
}

#[tokio::test]
async fn one_pad_moves_the_surface_into_a_project_and_onto_its_page() {
    let mut harness = Harness::start();

    harness.tap(0).await;
    harness.settles_on(Some("alpha"), "work").await;

    assert_eq!(
        harness.manager.current().await,
        Some(WorkspaceId::new("alpha")),
        "the manager and the surface agree"
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_binding_scoped_to_a_project_comes_into_force_with_it() {
    let mut harness = Harness::start();

    // Outside the project, the pad means nothing and the surface stays put.
    harness.tap(2).await;
    harness.settles_on(None, "music").await;
    harness.tap(3).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    harness.catch_up().await;
    assert_eq!(
        harness.page.as_deref(),
        Some("music"),
        "a binding scoped to a project must not fire outside it"
    );

    // Inside it, the same pad works.
    harness.tap(0).await;
    harness.settles_on(Some("alpha"), "work").await;
    harness.tap(2).await;
    harness.settles_on(Some("alpha"), "music").await;
    harness.tap(3).await;
    harness.settles_on(Some("alpha"), "work").await;

    harness.stop().await;
}

#[tokio::test]
async fn returning_to_a_project_puts_the_operator_back_where_they_were() {
    // The whole point of remembering: a pad puts them back where they left
    // off, not back at the beginning.
    let mut harness = Harness::start();

    harness.tap(0).await;
    harness.settles_on(Some("alpha"), "work").await;
    harness.tap(2).await;
    harness.settles_on(Some("alpha"), "music").await;

    harness.tap(1).await;
    harness.settles_on(Some("beta"), "music").await;

    harness.tap(0).await;
    harness.settles_on(Some("alpha"), "music").await;

    harness.stop().await;
}

#[tokio::test]
async fn a_terminal_opened_in_a_project_starts_in_its_directory() {
    // What makes one pad mean the whole thing: everything started afterwards
    // happens in the project.
    let mut harness = Harness::start();

    harness.tap(1).await;
    harness.settles_on(Some("beta"), "home").await;
    harness.tap(4).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let opened = harness
            .terminals
            .open_ids()
            .into_iter()
            .find_map(|id| harness.terminals.spec(&id));

        if let Some(spec) = opened {
            assert_eq!(spec.cwd, std::path::PathBuf::from("/tmp/pushos-beta"));
            break;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "no terminal was opened"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    harness.stop().await;
}
