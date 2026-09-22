//! A whole fleet of threads against a real `codex app-server`.
//!
//! Ignored by default: these need Codex installed and signed in, and they take
//! seconds rather than milliseconds. They are the only thing that proves the
//! claim this adapter exists for — that sixty-four sessions fit in one process
//! — because every other test here talks to a stand-in that would agree to
//! anything.
//!
//! Run them with:
//!
//! ```text
//! cargo test -p pushos-codex -- --ignored --nocapture
//! ```

use std::sync::{Arc, Mutex};
use std::time::Instant;

use pushos_codex::CodexBackend;
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{
    AgentBackend, AgentEvent, AgentObserver, SessionHandle, SessionRequest,
};

/// How many threads the fleet test opens. The number on the front of the box.
const FLEET: usize = 64;

/// What the whole fleet may cost, resident, across the whole process tree.
///
/// A trip wire rather than a target: the day a thread starts costing tens of
/// megabytes, sixty-four pads quietly become a dozen again, and a number that
/// only ever gets looked at by hand would not catch it.
const FLEET_CEILING_MB: f64 = 2_500.0;

/// What a process and everything it started are holding, in megabytes.
///
/// The tree, not the process. `codex` on this machine is a small Node wrapper
/// that spawns the real binary, so measuring the process PushOS started
/// reports the wrapper and misses everything that matters — which is exactly
/// the mistake the first version of this test made.
fn resident_mb(pid: u32) -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,rss="])
        .output()
        .expect("ps runs");
    let listing = String::from_utf8_lossy(&out.stdout);

    let rows: Vec<(u32, u32, f64)> = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let rss: f64 = fields.next()?.parse().ok()?;
            Some((pid, parent, rss))
        })
        .collect();

    // Walk down from the root, so a wrapper's children are counted whatever
    // shape the installation happens to have.
    let mut tree = vec![pid];
    let mut held = 0.0;
    while let Some(at) = tree.pop() {
        for (child, parent, rss) in &rows {
            if *parent == at {
                tree.push(*child);
            }
            if *child == at {
                held += rss / 1024.0;
            }
        }
    }
    held
}

/// Collects what the backend reported, so a test can look at it afterwards.
#[derive(Debug, Default)]
struct Heard(Mutex<Vec<(SessionId, AgentEvent)>>);

impl AgentObserver for Heard {
    fn observe(&self, session: &SessionId, event: AgentEvent) {
        if let Ok(mut heard) = self.0.lock() {
            heard.push((session.clone(), event));
        }
    }
}

fn wanted(name: &str) -> SessionRequest {
    SessionRequest {
        agent: name.into(),
        workspace: None,
        cwd: std::env::temp_dir(),
        objective: "Answer briefly.".to_owned(),
        permissions: None,
    }
}

fn backend() -> CodexBackend {
    CodexBackend::new(
        "codex",
        "codex",
        vec!["app-server".to_owned()],
        Arc::new(Heard::default()),
    )
}

#[tokio::test]
#[ignore = "needs Codex installed"]
async fn sixty_four_threads_live_in_one_process() {
    let backend = backend();

    // One thread first, so the fixed cost of the process can be told apart
    // from what a thread actually costs. Quoting the total as though it were
    // per-thread flatters the number by an order of magnitude.
    let first = backend.start(wanted("first")).await.expect("one opens");
    let pid = backend.pid().await.expect("the server is running");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let alone = resident_mb(pid);

    let began = Instant::now();
    let mut opened: Vec<SessionHandle> = vec![first];
    for i in 1..FLEET {
        opened.push(
            backend
                .start(wanted(&format!("worker-{i}")))
                .await
                .unwrap_or_else(|error| panic!("thread {i} should open: {error}")),
        );
    }
    let took = began.elapsed();

    let mut threads: Vec<&str> = opened
        .iter()
        .map(|handle| {
            handle
                .provider_session
                .as_deref()
                .expect("a Codex session carries its thread id")
        })
        .collect();
    let total = threads.len();
    threads.sort_unstable();
    threads.dedup();

    assert_eq!(total, FLEET, "every thread should have opened");
    assert_eq!(threads.len(), FLEET, "every thread should be its own");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let held = resident_mb(pid);
    let each = (held - alone) / f64::from(u32::try_from(FLEET - 1).unwrap_or(1));

    println!(
        "one thread: {alone:.0} MB. {FLEET} threads: {held:.0} MB \
         ({each:.1} MB each after the first), opened in {:?} ({:.0} ms each)",
        took,
        took.as_secs_f64() * 1000.0 / f64::from(u32::try_from(FLEET).unwrap_or(u32::MAX))
    );

    assert!(
        held < FLEET_CEILING_MB,
        "a fleet of {FLEET} threads should fit in one process: {held:.1} MB is past the \
         {FLEET_CEILING_MB:.0} MB this adapter exists to stay under"
    );

    for handle in &opened {
        backend.stop(handle).await.expect("a thread closes");
    }
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "needs Codex installed"]
async fn a_thread_opens_and_carries_an_id_that_could_be_resumed() {
    let backend = backend();
    let handle = backend.start(wanted("solo")).await.expect("a thread opens");

    assert_eq!(handle.provider.as_str(), "codex");
    let thread = handle
        .provider_session
        .clone()
        .expect("the thread id is what makes a parked session free");
    assert!(!thread.is_empty());

    backend.stop(&handle).await.expect("it closes");
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "needs Codex installed"]
async fn the_provider_says_what_it_can_do() {
    let backend = backend();
    let can = backend.capabilities().await.expect("capabilities are read");

    assert!(
        can.resume,
        "a thread id outlives the process that opened it"
    );
    assert!(can.cancel, "a turn can be interrupted");
    assert!(can.permissions, "Codex asks before it acts");
    assert!(
        !can.models.is_empty(),
        "the app-server should list the models it offers"
    );
    println!("models: {}", can.models.join(", "));

    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "needs Codex installed"]
async fn a_session_that_was_never_opened_is_refused_rather_than_guessed_at() {
    let backend = backend();
    let stranger = SessionHandle {
        id: SessionId::new("never-opened"),
        provider_session: None,
        provider: "codex".into(),
    };

    let refused = backend.prompt(&stranger, "hello").await;
    assert!(refused.is_err(), "an unknown session has nothing to prompt");

    backend.shutdown().await;
}

#[tokio::test]
async fn nothing_is_started_until_something_is_asked_of_it() {
    // Configuration is read at startup and by `pushos check`. Neither is a
    // reason to have a Codex running, and an operator who has Codex installed
    // but never uses it should never see one.
    let backend = backend();
    assert!(
        backend.pid().await.is_none(),
        "describing a provider must not start it"
    );
    backend.shutdown().await;
}

#[tokio::test]
async fn a_provider_whose_program_does_not_exist_reports_itself_unavailable() {
    let backend = CodexBackend::new(
        "codex",
        "definitely-not-a-program-on-this-machine",
        Vec::new(),
        Arc::new(Heard::default()),
    );

    let refused = backend
        .start(wanted("nobody"))
        .await
        .expect_err("no program");
    assert!(
        matches!(
            refused,
            pushos_domain::ports::AgentError::Unavailable { .. }
        ),
        "a missing program is the provider being unavailable, got {refused}"
    );
    backend.shutdown().await;
}

#[tokio::test]
#[ignore = "needs Codex installed"]
async fn a_read_only_role_opens_a_thread_the_app_server_accepts() {
    // The sandbox is sent when the thread opens. If the app-server did not
    // recognise the mode, `thread/start` would refuse rather than quietly
    // opening a thread with the run of the machine.
    use pushos_domain::permissions::{Permission, PermissionSet};

    let backend = backend();
    let mut reader = PermissionSet::empty();
    reader.grant(Permission::FilesystemRead);

    let mut request = wanted("scout");
    request.permissions = Some(reader);

    let handle = backend
        .start(request)
        .await
        .expect("a read-only thread opens");
    assert!(handle.provider_session.is_some());

    backend.stop(&handle).await.expect("it closes");
    backend.shutdown().await;
}
