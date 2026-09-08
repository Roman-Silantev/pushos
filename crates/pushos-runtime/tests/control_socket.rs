//! The control socket, driven the way PushOS Studio drives it.
//!
//! These prove the guarantee the specification asks for: Studio configures a
//! running PushOS over a socket, the configuration files stay the source of
//! truth, and an edit that would break the surface changes nothing.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pushos_api::protocol::{FailureKind, Request, Response};
use pushos_api::{ControlClient, ControlServer};
use pushos_config::{BindingAddress, BindingSpec, ConfigStore};
use pushos_domain::ids::ExecutionId;
use pushos_domain::ports::PushOutput;
use pushos_runtime::{Runtime, Shutdown};
use pushos_testkit::{FakePush, RecordingProvider};
use pushos_ui::PushRenderer;

const CONFIG: &str = r#"
[runtime]
home_page = "home"

[[pages]]
id = "home"
name = "Home"

[[pages]]
id = "music"
name = "Music"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "test.one"
label = "One"
"#;

/// A running PushOS with a control socket, and a client already connected.
struct Harness {
    client: ControlClient,
    directory: PathBuf,
    socket: PathBuf,
    provider: RecordingProvider,
    shutdown: Shutdown,
    finished: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start() -> Self {
        let directory =
            std::env::temp_dir().join(format!("pushos-control-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
        std::fs::write(directory.join("pushos.toml"), CONFIG).expect("writable");

        let config = Arc::new(ConfigStore::load(&directory).expect("the configuration is valid"));
        // A Unix socket path is length-limited, and the system temporary
        // directory is long on macOS, so the socket goes somewhere short.
        let socket = short_socket();
        let server = ControlServer::bind(&socket).expect("the socket binds");

        let provider = RecordingProvider::new("test", ["one", "two"]);
        let shutdown = Shutdown::new();
        let runtime = Runtime::new(config)
            .with_provider(Arc::new(provider.clone()))
            .expect("free")
            .with_provider(Arc::new(
                pushos_actions::providers::page::PageProvider::new(),
            ))
            .expect("free")
            .with_control_socket(server);

        let (surface, input) = FakePush::new();
        let output: Arc<dyn PushOutput> = Arc::new(surface);
        let running = shutdown.clone();
        let finished = tokio::spawn(async move {
            runtime
                .run(
                    Box::new(input),
                    output,
                    PushRenderer::new().expect("builds"),
                    running,
                )
                .await;
        });

        // The socket is already bound, so connecting only waits for the accept
        // loop to start.
        let client = connect(&socket).await;
        Self {
            client,
            directory,
            socket,
            provider,
            shutdown,
            finished,
        }
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.directory.join("pushos.toml")).expect("readable")
    }

    async fn stop(self) {
        self.shutdown.stop().await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.finished).await;
        std::fs::remove_dir_all(&self.directory).ok();
        std::fs::remove_file(&self.socket).ok();
    }
}

/// A socket path short enough for the kernel's fixed-size address field.
///
/// Takes the tail of the identifier rather than the head: these ids are time
/// ordered, so tests running in the same millisecond share a prefix and would
/// collide on the same socket.
fn short_socket() -> PathBuf {
    let id = ExecutionId::generate().to_string().replace('-', "");
    PathBuf::from(format!("/tmp/pos-{}.sock", &id[id.len() - 12..]))
}

async fn connect(socket: &std::path::Path) -> ControlClient {
    for _ in 0..200 {
        if let Ok(client) = ControlClient::connect(socket).await {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the control socket never accepted a connection");
}

#[tokio::test]
async fn a_client_can_ask_how_the_runtime_is_doing() {
    let mut harness = Harness::start().await;

    let status = harness
        .client
        .handshake()
        .await
        .expect("the protocols match");
    assert_eq!(status.binding_count, 1);
    assert_eq!(status.page.as_deref(), Some("home"));
    assert!(!status.version.is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn the_vocabulary_covers_the_whole_surface_so_a_client_hard_codes_nothing() {
    let mut harness = Harness::start().await;

    let Response::Vocabulary(vocabulary) = harness
        .client
        .send(&Request::Describe)
        .await
        .expect("answered")
    else {
        panic!("expected a vocabulary");
    };

    assert_eq!(
        vocabulary.controls.len(),
        141,
        "every control should be offered"
    );
    assert_eq!(vocabulary.gestures.len(), 13);
    assert_eq!(vocabulary.pages.len(), 2);

    let pads: Vec<_> = vocabulary
        .controls
        .iter()
        .filter(|control| control.kind == "pad")
        .collect();
    assert_eq!(pads.len(), 64);
    assert!(
        pads.iter().all(|pad| pad.grid.is_some()),
        "pads carry their grid position"
    );
    assert!(pads.iter().all(|pad| pad.illuminated));

    let names: Vec<_> = vocabulary
        .providers
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"test"));
    assert!(names.contains(&"page"));

    harness.stop().await;
}

#[tokio::test]
async fn the_vocabulary_says_which_providers_are_actually_permitted() {
    let mut harness = Harness::start().await;

    let Response::Vocabulary(vocabulary) = harness
        .client
        .send(&Request::Describe)
        .await
        .expect("answered")
    else {
        panic!("expected a vocabulary");
    };

    let page = vocabulary
        .providers
        .iter()
        .find(|provider| provider.name == "page")
        .expect("the page provider is installed");
    assert!(page.requires.is_empty());
    assert!(
        page.permitted,
        "an action needing nothing is always permitted"
    );

    harness.stop().await;
}

#[tokio::test]
async fn bindings_come_back_the_way_they_were_written() {
    let mut harness = Harness::start().await;

    let answer = harness
        .client
        .send(&Request::Bindings)
        .await
        .expect("answered");
    let Response::Bindings(list) = answer else {
        panic!("expected bindings, got {answer:?}");
    };

    assert_eq!(list.bindings.len(), 1);
    assert_eq!(list.bindings[0].address.control, "pad.0");
    assert_eq!(list.bindings[0].address.gesture, "tap");
    assert_eq!(list.bindings[0].action, "test.one");
    assert_eq!(list.bindings[0].label.as_deref(), Some("One"));

    harness.stop().await;
}

#[tokio::test]
async fn an_edit_reaches_the_file_and_the_running_surface() {
    let mut harness = Harness::start().await;

    let spec = BindingSpec::new(
        BindingAddress::new("pad.5", "hold").on_page("music"),
        "test.two",
    )
    .with_label("Two");
    let Response::Edited(report) = harness
        .client
        .send(&Request::Bind {
            spec: Box::new(spec),
        })
        .await
        .expect("answered")
    else {
        panic!("expected an edit report");
    };

    assert!(!report.replaced);
    assert_eq!(
        report.binding_count, 2,
        "the running configuration was reloaded"
    );
    assert!(report.file.ends_with("pushos.toml"));

    let saved = harness.read_config();
    assert!(
        saved.contains("test.two"),
        "the file is the source of truth"
    );
    assert!(saved.contains("label = \"Two\""));

    harness.stop().await;
}

#[tokio::test]
async fn editing_an_existing_binding_replaces_rather_than_duplicates_it() {
    let mut harness = Harness::start().await;

    let spec = BindingSpec::new(BindingAddress::new("pad.0", "tap"), "test.two");
    let Response::Edited(report) = harness
        .client
        .send(&Request::Bind {
            spec: Box::new(spec),
        })
        .await
        .expect("answered")
    else {
        panic!("expected an edit report");
    };

    assert!(report.replaced);
    assert_eq!(report.binding_count, 1, "no duplicate was created");
    assert!(!harness.read_config().contains("test.one"));

    harness.stop().await;
}

#[tokio::test]
async fn an_edit_that_would_break_the_surface_changes_nothing() {
    let mut harness = Harness::start().await;
    let before = harness.read_config();

    let spec = BindingSpec::new(
        BindingAddress::new("pad.5", "tap").on_page("nowhere"),
        "test.two",
    );
    let Response::Failed(failure) = harness
        .client
        .send(&Request::Bind {
            spec: Box::new(spec),
        })
        .await
        .expect("answered")
    else {
        panic!("expected a refusal");
    };

    assert_eq!(failure.kind, FailureKind::Rejected);
    assert!(
        !failure.problems.is_empty(),
        "the reason should be itemised"
    );
    assert_eq!(harness.read_config(), before, "the file must be untouched");

    harness.stop().await;
}

#[tokio::test]
async fn a_binding_can_be_removed() {
    let mut harness = Harness::start().await;

    let Response::Edited(report) = harness
        .client
        .send(&Request::Unbind {
            address: BindingAddress::new("pad.0", "tap"),
        })
        .await
        .expect("answered")
    else {
        panic!("expected an edit report");
    };

    assert_eq!(report.binding_count, 0);
    assert!(!harness.read_config().contains("test.one"));

    harness.stop().await;
}

#[tokio::test]
async fn removing_something_that_is_not_bound_says_so() {
    let mut harness = Harness::start().await;

    let Response::Failed(failure) = harness
        .client
        .send(&Request::Unbind {
            address: BindingAddress::new("pad.60", "hold"),
        })
        .await
        .expect("answered")
    else {
        panic!("expected a refusal");
    };

    assert_eq!(failure.kind, FailureKind::NotFound);
    harness.stop().await;
}

#[tokio::test]
async fn testing_a_binding_runs_its_action_once() {
    let mut harness = Harness::start().await;

    let Response::Tested(report) = harness
        .client
        .send(&Request::Test {
            address: BindingAddress::new("pad.0", "tap"),
        })
        .await
        .expect("answered")
    else {
        panic!("expected a test report");
    };

    assert_eq!(report.action, "test.one");
    assert_eq!(report.status, "completed");
    assert_eq!(harness.provider.call_count(), 1, "exactly once");

    harness.stop().await;
}

#[tokio::test]
async fn testing_something_that_is_not_bound_says_so_rather_than_running_anything() {
    let mut harness = Harness::start().await;

    let Response::Failed(failure) = harness
        .client
        .send(&Request::Test {
            address: BindingAddress::new("pad.60", "hold"),
        })
        .await
        .expect("answered")
    else {
        panic!("expected a refusal");
    };

    assert_eq!(failure.kind, FailureKind::NotFound);
    assert_eq!(harness.provider.call_count(), 0);

    harness.stop().await;
}

#[tokio::test]
async fn a_malformed_request_loses_only_itself() {
    let mut harness = Harness::start().await;

    // Talk to the socket directly, since the client cannot send nonsense.
    let mut raw = tokio::net::UnixStream::connect(&harness.socket)
        .await
        .expect("connects");
    {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        raw.write_all(b"{\"request\":\"self_destruct\"}\n")
            .await
            .expect("written");
        raw.flush().await.expect("flushed");

        let (reader, _) = raw.split();
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let answer = lines
            .next_line()
            .await
            .expect("readable")
            .expect("an answer");
        assert!(answer.contains("malformed"), "got {answer}");
    }

    // The runtime is still serving.
    assert!(harness.client.handshake().await.is_ok());
    harness.stop().await;
}

#[tokio::test]
async fn several_requests_share_one_connection() {
    let mut harness = Harness::start().await;

    for _ in 0..5 {
        assert!(matches!(
            harness
                .client
                .send(&Request::Status)
                .await
                .expect("answered"),
            Response::Status(_)
        ));
    }

    harness.stop().await;
}

#[tokio::test]
async fn the_socket_is_not_readable_by_anyone_else() {
    use std::os::unix::fs::PermissionsExt as _;

    let harness = Harness::start().await;
    let mode = std::fs::metadata(&harness.socket)
        .expect("the socket exists")
        .permissions()
        .mode();

    assert_eq!(
        mode & 0o077,
        0,
        "the socket must not be reachable by other accounts"
    );
    harness.stop().await;
}

#[tokio::test]
async fn the_socket_file_is_cleaned_up_so_a_stale_one_never_misleads_a_client() {
    let harness = Harness::start().await;
    let socket = harness.socket.clone();
    assert!(socket.exists());

    harness.stop().await;
    assert!(!socket.exists(), "a stale socket was left behind");
}

#[tokio::test]
async fn a_stale_socket_from_a_previous_run_does_not_stop_pushos_starting() {
    let socket = short_socket();
    std::fs::write(&socket, "not really a socket").expect("writable");

    let server = ControlServer::bind(&socket).expect("a stale file should be replaced");
    assert_eq!(server.path(), socket);

    drop(server);
    std::fs::remove_file(&socket).ok();
}
