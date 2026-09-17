//! Talking to one `codex app-server`, which holds every thread PushOS uses.
//!
//! This is the whole reason Codex threads are affordable on a surface with
//! sixty-four pads: the server is one process, and a thread inside it costs a
//! few tens of megabytes rather than a few hundred. So PushOS keeps one
//! connection and speaks the app server's protocol over it, rather than
//! starting a program for every pad.
//!
//! The protocol is JSON-RPC with one message per line. Nothing here knows what
//! the methods mean; that is [`super`]'s business.
//!
//! The server is started when it is first needed and killed when PushOS stops.
//! That loses nothing: threads live on disk, and one whose server has gone is
//! listed and resumed exactly as one that was merely unloaded.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pushos_domain::ports::AttachError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, warn};

use crate::sessions::programs;

/// How long one request may take.
///
/// Listing and unloading are quick. Starting a thread waits for the server to
/// write it down, not for any work to be done.
const PATIENCE: Duration = Duration::from_secs(30);

/// The longest answer PushOS will read.
///
/// A thread listing can be large, and this is well past any of them; it exists
/// so a program printing something else cannot be kept whole in memory.
const LONGEST_LINE: u64 = 32 * 1024 * 1024;

/// What PushOS calls itself to the server.
const CLIENT: &str = "pushos";

/// What the server is told not to load.
///
/// Every one of these is something a coding thread on a pad does not use, and
/// each costs memory in the server and, for some, a program of its own for
/// every thread. Measured on an M4 with sixty-four threads: 1.1 GB without
/// them against 1.5 GB with them, which is 16 MB a thread rather than 22.
/// An operator who wants them can say so; see `codex_features`.
const NOT_LOADED: [&str; 6] = [
    "apps",
    "browser_use",
    "computer_use",
    "code_mode_host",
    "goals",
    "guardian_approval",
];

/// Everyone waiting for an answer, by the number they asked under.
type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// What the server has said without being asked.
///
/// The server says when a thread starts working and when it stops, which is
/// both sooner and cheaper than asking it every few seconds. Kept here as it
/// arrives; [`super`] decides what it means for a pad.
#[derive(Debug, Default)]
pub(super) struct Heard {
    /// The last status each thread was reported to be in.
    statuses: HashMap<String, String>,
}

impl Heard {
    /// What a thread was last reported to be doing.
    pub(super) fn status_of(&self, thread: &str) -> Option<&str> {
        self.statuses.get(thread).map(String::as_str)
    }

    /// Notes a status as though the server had said it, for tests.
    #[cfg(test)]
    pub(super) fn note_for_test(&mut self, thread: &str, status: &str) {
        self.note(
            "thread/status/changed",
            &json!({"threadId": thread, "status": {"type": status}}),
        );
    }

    /// Forgets threads nobody is showing, so this cannot grow for ever.
    pub(super) fn keep_only(&mut self, threads: &[String]) {
        self.statuses
            .retain(|thread, _| threads.iter().any(|kept| kept == thread));
    }

    /// Takes in one thing the server said.
    fn note(&mut self, method: &str, params: &Value) {
        // A turn starting and finishing is a status change too, and the status
        // is what a pad shows, so this is the only thing worth listening for.
        if method != "thread/status/changed" {
            return;
        }
        let (Some(thread), Some(status)) = (
            params.get("threadId").and_then(Value::as_str),
            params
                .get("status")
                .and_then(|status| status.get("type"))
                .and_then(Value::as_str),
        ) else {
            return;
        };
        self.statuses.insert(thread.to_owned(), status.to_owned());
    }
}

/// A connection to an app server, started when it is first wanted.
#[derive(Debug)]
pub(super) struct AppServer {
    /// What to run, when it is not the `codex` on the path.
    program: Option<PathBuf>,
    /// The arguments that start the server, for a test that starts something
    /// else that speaks the same protocol.
    arguments: Vec<String>,
    live: Mutex<Option<Live>>,
    /// Held while saying hello, so two callers at once cannot both do it: a
    /// server greeted twice refuses the second.
    greeting: Mutex<()>,
    /// What the server has said of its own accord.
    heard: Arc<Mutex<Heard>>,
    next: AtomicU64,
}

/// A server that is running, and what has been asked of it.
#[derive(Debug)]
struct Live {
    child: tokio::process::Child,
    writing: tokio::process::ChildStdin,
    /// Whether this server has been greeted. Once per connection: saying it
    /// twice is an error, and a server started again has not heard it at all.
    greeted: bool,
    /// Who is waiting for which answer. Emptied when the server goes.
    waiting: Waiting,
}

impl AppServer {
    /// The `codex` installed for this user.
    pub(super) fn new(lean: bool) -> Self {
        let mut arguments = Vec::new();
        if lean {
            for feature in NOT_LOADED {
                arguments.extend(["--disable".to_owned(), feature.to_owned()]);
            }
        }
        arguments.push("app-server".to_owned());
        Self {
            program: None,
            arguments,
            live: Mutex::new(None),
            greeting: Mutex::new(()),
            heard: Arc::new(Mutex::new(Heard::default())),
            next: AtomicU64::new(0),
        }
    }

    /// A particular program that speaks the same protocol.
    #[cfg(test)]
    pub(super) fn running(program: impl Into<PathBuf>, arguments: &[&str]) -> Self {
        Self {
            program: Some(program.into()),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            live: Mutex::new(None),
            greeting: Mutex::new(()),
            heard: Arc::new(Mutex::new(Heard::default())),
            next: AtomicU64::new(0),
        }
    }

    /// What the server has said without being asked.
    pub(super) fn heard(&self) -> &Arc<Mutex<Heard>> {
        &self.heard
    }

    /// Where Codex is installed, if it is.
    pub(super) fn program(&self) -> Option<PathBuf> {
        self.program.clone().or_else(|| programs::find("codex"))
    }

    /// Asks the server something, starting it if it is not running.
    ///
    /// A server that has gone is started again by the next call: what it held
    /// was on disk all along.
    pub(super) async fn call(&self, method: &str, params: Value) -> Result<Value, AttachError> {
        match self.attempt(method, params.clone()).await {
            Err(gone) if gone.retry => {
                debug!(method, "the app server went away; starting another");
                *self.live.lock().await = None;
                self.attempt(method, params)
                    .await
                    .map_err(|failed| failed.error)
            }
            Err(failed) => Err(failed.error),
            Ok(answer) => Ok(answer),
        }
    }

    /// One go at asking, over a server that has been started and greeted.
    async fn attempt(&self, method: &str, params: Value) -> Result<Value, Failed> {
        self.say_hello().await?;
        self.ask(method, params).await
    }

    /// Says hello to the server, once per connection.
    ///
    /// The server expects it before anything else, and refuses to hear it
    /// twice, so this is where it belongs rather than at each call.
    async fn say_hello(&self) -> Result<(), Failed> {
        let _saying = self.greeting.lock().await;
        let unheard = {
            let mut held = self.live.lock().await;
            if held.is_none() {
                *held = Some(self.start().map_err(Failed::final_answer)?);
            }
            held.as_ref().is_some_and(|live| !live.greeted)
        };
        if !unheard {
            return Ok(());
        }

        self.ask(
            "initialize",
            json!({"clientInfo": {"name": CLIENT, "version": env!("CARGO_PKG_VERSION"), "title": "PushOS"}}),
        )
        .await?;
        if let Some(live) = self.live.lock().await.as_mut() {
            live.greeted = true;
        }
        Ok(())
    }

    async fn ask(&self, method: &str, params: Value) -> Result<Value, Failed> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (reply, answer) = oneshot::channel();
        let line = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        );

        {
            let mut held = self.live.lock().await;
            if held.is_none() {
                *held = Some(self.start().map_err(Failed::final_answer)?);
            }
            let Some(running) = held.as_mut() else {
                return Err(Failed::final_answer(AttachError::unavailable(
                    "the Codex app server could not be started",
                )));
            };
            running.waiting.lock().await.insert(id, reply);
            if running.writing.write_all(line.as_bytes()).await.is_err() {
                running.waiting.lock().await.remove(&id);
                return Err(Failed::worth_another_go());
            }
            let _ = running.writing.flush().await;
        }

        match tokio::time::timeout(PATIENCE, answer).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(said))) => Err(Failed::final_answer(AttachError::unavailable(said))),
            // The reader dropped the sender: the server ended mid-question.
            Ok(Err(_)) => Err(Failed::worth_another_go()),
            Err(_) => {
                self.forget(id).await;
                Err(Failed::final_answer(AttachError::unavailable(format!(
                    "the Codex app server did not answer `{method}` within {} seconds",
                    PATIENCE.as_secs()
                ))))
            }
        }
    }

    /// Stops waiting for an answer that never came.
    async fn forget(&self, id: u64) {
        if let Some(running) = self.live.lock().await.as_ref() {
            running.waiting.lock().await.remove(&id);
        }
    }

    /// Starts a server and says hello to it.
    fn start(&self) -> Result<Live, AttachError> {
        let program = self.program().ok_or_else(|| {
            AttachError::unavailable("Codex is not installed, so it can keep no threads")
        })?;

        let mut child = tokio::process::Command::new(&program)
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                AttachError::unavailable(format!("could not start the Codex app server: {error}"))
            })?;

        let (Some(writing), Some(reading)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(AttachError::unavailable(
                "the Codex app server gave PushOS nothing to talk over",
            ));
        };

        let asked: Waiting = Arc::new(Mutex::new(HashMap::new()));
        let answering = Arc::clone(&asked);
        let noting = Arc::clone(&self.heard);
        tokio::spawn(async move {
            let mut reading = BufReader::new(reading);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reading.read_until(b'\n', &mut line).await {
                    // The server ended, or said something PushOS cannot read.
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // A line longer than any answer could be is dropped rather
                // than parsed, so nothing the server says can be kept forever.
                if line.len() as u64 > LONGEST_LINE {
                    warn!("ignoring an answer from the Codex app server that was absurdly long");
                    continue;
                }
                if let Ok(text) = std::str::from_utf8(&line) {
                    deliver(text, &answering, &noting).await;
                }
            }
            // Nobody will answer what is still outstanding, and a caller left
            // waiting for an answer that cannot come is worse than an error.
            answering.lock().await.clear();
        });

        let live = Live {
            child,
            writing,
            greeted: false,
            waiting: asked,
        };
        debug!(program = %program.display(), "started the Codex app server");
        Ok(live)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // The threads it held are on disk; the process is not worth keeping.
        if let Err(error) = self.child.start_kill() {
            warn!(%error, "the Codex app server would not stop");
        }
    }
}

/// Hands one line of the server's output to whoever asked for it.
async fn deliver(line: &str, waiting: &Waiting, heard: &Mutex<Heard>) {
    let Some((id, answer)) = read_answer(line) else {
        // Not an answer to anything: the server saying what has changed, which
        // is how PushOS knows what threads are doing without asking.
        if let Some((method, params)) = read_notification(line) {
            heard.lock().await.note(&method, &params);
        }
        return;
    };
    if let Some(reply) = waiting.lock().await.remove(&id) {
        let _ = reply.send(answer);
    }
}

/// What one line of the server's output means, when it is an answer at all.
fn read_answer(line: &str) -> Option<(u64, Result<Value, String>)> {
    let message: Value = serde_json::from_str(line.trim()).ok()?;
    let id = message.get("id")?.as_u64()?;
    if let Some(failed) = message.get("error") {
        let said = failed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the Codex app server refused");
        return Some((id, Err(said.to_owned())));
    }
    Some((
        id,
        Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    ))
}

/// What the server said of its own accord, when a line is that.
fn read_notification(line: &str) -> Option<(String, Value)> {
    let message: Value = serde_json::from_str(line.trim()).ok()?;
    if message.get("id").is_some() {
        return None;
    }
    let method = message.get("method")?.as_str()?.to_owned();
    Some((
        method,
        message.get("params").cloned().unwrap_or(Value::Null),
    ))
}

/// A request that did not work, and whether starting the server again is worth
/// trying.
#[derive(Debug)]
struct Failed {
    error: AttachError,
    retry: bool,
}

impl Failed {
    const fn final_answer(error: AttachError) -> Self {
        Self {
            error,
            retry: false,
        }
    }

    fn worth_another_go() -> Self {
        Self {
            error: AttachError::unavailable("the Codex app server stopped answering"),
            retry: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_goes_to_whoever_asked_for_it() {
        let (id, answer) =
            read_answer(r#"{"jsonrpc":"2.0","id":7,"result":{"data":[]}}"#).expect("an answer");
        assert_eq!(id, 7);
        assert_eq!(answer.expect("a result"), json!({"data": []}));
    }

    #[test]
    fn a_refusal_says_what_the_server_said() {
        let (id, answer) = read_answer(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"no such thread"}}"#,
        )
        .expect("an answer");
        assert_eq!(id, 3);
        assert_eq!(answer.expect_err("refused"), "no such thread");
    }

    #[test]
    fn what_a_thread_is_doing_is_taken_from_what_the_server_says() {
        let mut heard = Heard::default();
        heard.note(
            "thread/status/changed",
            &json!({"threadId": "one", "status": {"type": "active"}}),
        );
        assert_eq!(heard.status_of("one"), Some("active"));

        heard.note(
            "thread/status/changed",
            &json!({"threadId": "one", "status": {"type": "idle"}}),
        );
        heard.note("turn/completed", &json!({"threadId": "one"}));
        assert_eq!(heard.status_of("one"), Some("idle"));
        assert_eq!(heard.status_of("another"), None);
    }

    #[test]
    fn what_the_server_said_about_threads_nobody_shows_is_forgotten() {
        let mut heard = Heard::default();
        for thread in ["one", "two"] {
            heard.note(
                "thread/status/changed",
                &json!({"threadId": thread, "status": {"type": "idle"}}),
            );
        }

        heard.keep_only(&["two".to_owned()]);

        assert_eq!(heard.status_of("one"), None);
        assert_eq!(heard.status_of("two"), Some("idle"));
    }

    #[test]
    fn something_the_server_says_that_pushos_does_not_know_changes_nothing() {
        let mut heard = Heard::default();
        heard.note("mcpServer/startupStatus/updated", &json!({"name": "x"}));
        heard.note("thread/status/changed", &json!({"threadId": "one"}));
        assert_eq!(heard.status_of("one"), None);
    }

    #[test]
    fn a_notification_is_not_an_answer_to_anything() {
        assert!(read_answer(r#"{"jsonrpc":"2.0","method":"thread/status/changed"}"#).is_none());
        assert!(read_answer("not json at all").is_none());
        assert!(read_answer("").is_none());
    }

    /// A server of sorts: it answers every request with its own method name,
    /// which is enough to show that a question and its answer find each other.
    fn stub(label: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "pushos-app-server-{label}-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        let script = directory.join("server.py");
        std::fs::write(
            &script,
            "import json,sys\n\
             for line in sys.stdin:\n\
             \x20   asked = json.loads(line)\n\
             \x20   print(json.dumps({'jsonrpc':'2.0','method':'thread/status/changed'}), flush=True)\n\
             \x20   print(json.dumps({'jsonrpc':'2.0','id':asked['id'],'result':{'said':asked['method']}}), flush=True)\n",
        )
        .expect("writable");
        (directory, script)
    }

    #[tokio::test]
    async fn a_question_is_answered_over_one_connection() {
        let (directory, script) = stub("answers");
        let server = AppServer::running("/usr/bin/python3", &[&script.to_string_lossy()]);

        let first = server
            .call("thread/list", json!({"limit": 1}))
            .await
            .expect("answered");
        assert_eq!(first, json!({"said": "thread/list"}));

        // The same server answers the next one: it is one process, not one per
        // question, which is the whole point of it.
        let second = server
            .call("thread/loaded/list", json!({}))
            .await
            .expect("answered");
        assert_eq!(second, json!({"said": "thread/loaded/list"}));

        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn a_server_that_will_not_start_says_so_rather_than_hanging() {
        let server = AppServer::running("/definitely/not/a/program", &[]);
        let refused = server
            .call("thread/list", json!({}))
            .await
            .expect_err("nothing to talk to");
        assert!(refused.to_string().contains("could not start"), "{refused}");
    }

    #[tokio::test]
    async fn a_server_that_goes_away_mid_question_is_started_again() {
        let (directory, script) = stub("restart");
        // Answers the greeting and one question, then ends, as a server that
        // crashed would.
        std::fs::write(
            &script,
            "import json,sys\n\
             for _ in range(2):\n\
             \x20   line = sys.stdin.readline()\n\
             \x20   asked = json.loads(line)\n\
             \x20   print(json.dumps({'jsonrpc':'2.0','id':asked['id'],'result':{'said':'once'}}), flush=True)\n",
        )
        .expect("writable");
        let server = AppServer::running("/usr/bin/python3", &[&script.to_string_lossy()]);

        assert_eq!(
            server.call("thread/list", json!({})).await.expect("first"),
            json!({"said": "once"})
        );
        assert_eq!(
            server.call("thread/list", json!({})).await.expect("second"),
            json!({"said": "once"}),
            "a second server answered where the first had gone"
        );

        std::fs::remove_dir_all(directory).ok();
    }
}
