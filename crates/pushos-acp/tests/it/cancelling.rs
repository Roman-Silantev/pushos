//! Stopping an agent that is working.
//!
//! Cancel is the one control an operator presses *because* the agent is busy.
//! Answering it only once the work has finished makes the pad do nothing at
//! the only moment it matters, so this drives a real agent process that
//! accepts a prompt and never finishes it.

use std::sync::Arc;
use std::time::Duration;

use pushos_acp::{AcpBackend, AgentCommand};
use pushos_domain::ids::{AgentId, SessionId};
use pushos_domain::ports::{AgentBackend, AgentEvent, AgentObserver, SessionRequest};

/// An agent that answers the handshake and then goes quiet.
///
/// Written out at test time rather than checked in as a fixture, so what it
/// does is readable beside the thing it is proving.
const NEVER_FINISHES: &str = r#"
import json, sys

def reply(ident, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method = message.get("method")
    ident = message.get("id")
    if method == "initialize":
        reply(ident, {"protocolVersion": 1, "agentCapabilities": {}})
    elif method == "session/new":
        reply(ident, {"sessionId": "never-finishes"})
    elif method == "session/cancel":
        # Noticed, and deliberately not answered: the point is that the
        # notification arrives at all while the turn is still running.
        sys.stderr.write("cancelled\n")
        sys.stderr.flush()
    # session/prompt is never answered.
"#;

#[derive(Debug)]
struct Quiet;

impl AgentObserver for Quiet {
    fn observe(&self, _session: &SessionId, _event: AgentEvent) {}
}

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn holding(script: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "pushos-acp-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        std::fs::write(directory.join("agent.py"), script).expect("writable");
        Self(directory)
    }

    fn script(&self) -> String {
        self.0.join("agent.py").to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[tokio::test]
async fn a_working_agent_can_still_be_cancelled() {
    let scratch = Scratch::holding(NEVER_FINISHES);
    let backend = AcpBackend::new(
        "stand-in",
        AgentCommand::new("/usr/bin/python3", [scratch.script()]),
        Arc::new(Quiet),
    );

    let handle = backend
        .start(SessionRequest {
            agent: AgentId::new("builder"),
            workspace: None,
            cwd: std::env::temp_dir(),
            objective: String::new(),
            permissions: None,
        })
        .await
        .expect("the stand-in agent starts");

    backend
        .prompt(&handle, "work on something for ever")
        .await
        .expect("the turn starts");

    // The turn is running and will never end on its own.
    let cancelled = tokio::time::timeout(Duration::from_secs(5), backend.cancel(&handle)).await;
    assert!(
        cancelled.is_ok(),
        "cancel must come back while the agent is working, not after"
    );
    cancelled.expect("answered").expect("the cancel was sent");

    // And stopping must not wait for the turn either.
    let stopped = tokio::time::timeout(Duration::from_secs(5), backend.stop(&handle)).await;
    assert!(stopped.is_ok(), "stop must come back too");
}

#[tokio::test]
async fn an_agent_that_dies_while_idle_says_so() {
    // The role is resolved to this session until something says otherwise, so
    // a death nobody reports wedges that role: every later prompt goes to a
    // process that is not there, and only stopping it by hand clears it.
    let scratch = Scratch::holding("import sys\nsys.exit(0)\n");
    let seen = Arc::new(Watching::default());
    let backend = AcpBackend::new(
        "stand-in",
        AgentCommand::new("/usr/bin/python3", [scratch.script()]),
        seen.clone(),
    );

    // Starting fails, because the agent is gone before it can answer — which
    // is exactly the shape of one that dies a moment later.
    let started = backend
        .start(SessionRequest {
            agent: AgentId::new("builder"),
            workspace: None,
            cwd: std::env::temp_dir(),
            objective: String::new(),
            permissions: None,
        })
        .await;
    assert!(
        started.is_err(),
        "an agent that exits cannot open a session"
    );

    // What matters is that PushOS was told, rather than left believing in it.
    for _ in 0..200 {
        if seen.went_offline() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("nothing reported the session offline; its role would stay wedged");
}

/// Remembers whether a session was ever reported offline.
#[derive(Debug, Default)]
struct Watching {
    offline: std::sync::atomic::AtomicBool,
}

impl Watching {
    fn went_offline(&self) -> bool {
        self.offline.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AgentObserver for Watching {
    fn observe(&self, _session: &SessionId, event: AgentEvent) {
        if matches!(
            event,
            AgentEvent::StateChanged {
                state: pushos_domain::agent::AgentState::Offline
            }
        ) {
            self.offline
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
