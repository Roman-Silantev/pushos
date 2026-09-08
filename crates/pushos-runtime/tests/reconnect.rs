//! What survives the Push 2 being unplugged.
//!
//! The architecture claims the runtime outlives the hardware. These check that
//! the claim is about state and not only about the process staying alive: a
//! surface that comes back must find its terminals still running, its project
//! still selected, and Studio must have been able to reach PushOS the whole
//! time it was gone.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_actions::providers::{terminal, workspace};
use pushos_api::protocol::{Request, Response};
use pushos_api::{ControlClient, ControlServer};
use pushos_config::{ConfigFile, ConfigStore, RuntimeConfig};
use pushos_domain::controls::PadIndex;
use pushos_domain::ids::{ExecutionId, WorkspaceId};
use pushos_domain::ports::{PushOutput, WorkspaceContext};
use pushos_runtime::{RunningRuntime, Runtime, Shutdown, TerminalReporter};
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

[[workspaces]]
id = "alpha"
name = "Alpha"
root = "/tmp/pushos-alpha"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "workspace.select"
target = "alpha"

[[bindings]]
control = "pad.1"
gesture = "tap"
action = "terminal.open"
[bindings.params]
name = "shell"
"#;

fn store() -> Arc<ConfigStore> {
    let parsed: ConfigFile = toml::from_str(CONFIG).expect("the test configuration parses");
    RuntimeConfig::build(&parsed).expect("the test configuration is valid");

    let directory = std::env::temp_dir().join(format!("pushos-rc-{}", ExecutionId::generate()));
    std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
    std::fs::write(directory.join("pushos.toml"), CONFIG).expect("writable");
    Arc::new(ConfigStore::load(&directory).expect("the configuration is valid"))
}

fn socket() -> std::path::PathBuf {
    // Short, because a Unix socket path is far shorter than a file path.
    let tail = ExecutionId::generate().to_string();
    std::path::PathBuf::from(format!("/tmp/pos-{}.sock", &tail[tail.len() - 12..]))
}

struct Harness {
    running: RunningRuntime,
    manager: Arc<WorkspaceManager>,
    terminals: FakeTerminal,
    socket: std::path::PathBuf,
    shutdown: Shutdown,
}

impl Harness {
    async fn start() -> Self {
        let config = store();
        let current = config.current();

        let manager = Arc::new(WorkspaceManager::new(
            WorkspaceRegistry::new(current.workspaces.clone()),
            Arc::new(FakeWorkspaceMemory::new()),
            Arc::new(FakeRepository::new("/tmp/pushos-alpha")),
            "/tmp/pushos-nowhere",
            "/tmp/pushos-trees",
        ));

        let (reporter, updates) = TerminalReporter::new();
        let terminals = FakeTerminal::new(Arc::new(reporter));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(terminals.clone()),
            Arc::clone(&manager) as Arc<dyn WorkspaceContext>,
        ));

        let path = socket();
        let server = ControlServer::bind(&path).expect("the socket binds");
        let shutdown = Shutdown::new();

        let runtime = Runtime::new(config)
            .with_workspaces(Arc::clone(&manager))
            .with_terminals(Arc::clone(&supervisor), updates)
            .with_control_socket(server)
            .with_provider(Arc::new(workspace::WorkspaceProvider::new(
                Arc::clone(&manager),
                Arc::new(FakeApplications::new()),
            )))
            .expect("the namespace is free")
            .with_provider(Arc::new(terminal::TerminalProvider::new(supervisor)))
            .expect("the namespace is free");

        Self {
            running: runtime.start(&shutdown).await,
            manager,
            terminals,
            socket: path,
            shutdown,
        }
    }

    /// Attaches a surface, runs the closure against it, then unplugs it.
    async fn with_surface<F, Fut>(&self, use_it: F)
    where
        F: FnOnce(FakePush) -> Fut,
        Fut: Future<Output = ()>,
    {
        let (surface, input) = FakePush::new();
        let output: Arc<dyn PushOutput> = Arc::new(surface.clone());
        let renderer = PushRenderer::new().expect("the renderer builds");

        let serving = self
            .running
            .serve(Box::new(input), output, renderer, &self.shutdown);

        let driving = async {
            use_it(surface.clone()).await;
            surface.disconnect().await;
        };

        tokio::join!(serving, driving);
    }

    async fn ask(&self, request: &Request) -> Response {
        let mut client = ControlClient::connect(&self.socket)
            .await
            .expect("the socket is there");
        client.handshake().await.expect("it answers");
        client.send(request).await.expect("it answers")
    }

    async fn stop(self) {
        self.running.stop().await;
        self.shutdown.stop().await;
        std::fs::remove_file(&self.socket).ok();
    }
}

async fn tap(surface: &FakePush, index: u8) {
    let at = Instant::now();
    let pad = PadIndex::new(index).expect("test pad index is in range");
    surface.press_pad(pad, at).await;
    surface.release_pad(pad, at).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
}

#[tokio::test]
async fn work_started_on_one_surface_is_still_running_on_the_next() {
    // Unplugging a cable is not a reason to lose a terminal or forget which
    // project the operator is in.
    let harness = Harness::start().await;

    harness
        .with_surface(|surface| async move {
            tap(&surface, 0).await;
            tap(&surface, 1).await;
        })
        .await;

    assert_eq!(
        harness.manager.current().await,
        Some(WorkspaceId::new("alpha")),
        "the project outlives the surface"
    );
    assert_eq!(
        harness.terminals.open_count(),
        1,
        "and so does the terminal"
    );

    // The surface comes back and finds everything where it was.
    harness
        .with_surface(|surface| async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = surface.state().await;
            assert!(state.frame_count() > 0, "the display was drawn again");
        })
        .await;

    assert_eq!(
        harness.terminals.open_count(),
        1,
        "not started a second time"
    );
    harness.stop().await;
}

#[tokio::test]
async fn studio_can_reach_pushos_while_no_surface_is_attached() {
    // A Push 2 in a bag is not a reason for the configuration application to
    // say the machine is not running.
    let harness = Harness::start().await;

    let Response::Status(status) = harness.ask(&Request::Status).await else {
        panic!("expected a status report");
    };
    assert_eq!(status.binding_count, 2);
    assert_eq!(
        status.surface,
        pushos_api::protocol::SurfaceReport::Absent,
        "and it says plainly that nothing is attached"
    );

    let Response::Workspaces(list) = harness.ask(&Request::Workspaces).await else {
        panic!("expected the projects");
    };
    assert_eq!(list.workspaces.len(), 1);

    harness.stop().await;
}

#[tokio::test]
async fn the_socket_keeps_answering_across_a_surface_coming_and_going() {
    // The control socket is bound once. Binding it per connection would fail
    // the second time, and Studio would go quiet after the first unplug.
    let harness = Harness::start().await;

    harness
        .with_surface(|surface| async move {
            tap(&surface, 0).await;
        })
        .await;

    let Response::Status(status) = harness.ask(&Request::Status).await else {
        panic!("expected a status report");
    };
    assert_eq!(status.workspace.as_deref(), Some("alpha"));

    harness
        .with_surface(|surface| async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(surface);
        })
        .await;

    let Response::Sessions(sessions) = harness.ask(&Request::Sessions).await else {
        panic!("expected the sessions");
    };
    assert!(sessions.sessions.is_empty(), "none were started");

    harness.stop().await;
}
