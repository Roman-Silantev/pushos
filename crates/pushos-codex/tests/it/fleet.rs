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

/// What the whole fleet may cost, resident, in one process.
///
/// Measured at around 30 MB on an M4 in September 2026. The ceiling is
/// deliberately far above that: it is not a target to creep towards but a trip
/// wire for the day a thread starts costing megabytes, which would quietly
/// turn sixty-four pads back into a dozen.
const FLEET_CEILING_MB: f64 = 250.0;

/// What a process is holding, in megabytes.
fn resident_mb(pid: u32) -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
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

    let began = Instant::now();
    let mut opened: Vec<SessionHandle> = Vec::with_capacity(FLEET);
    for i in 0..FLEET {
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

    let pid = backend.pid().await.expect("the server is still running");
    let held = resident_mb(pid);

    println!(
        "opened {FLEET} threads in {:?} ({:.0} ms each), all in pid {pid} holding {held:.1} MB",
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
