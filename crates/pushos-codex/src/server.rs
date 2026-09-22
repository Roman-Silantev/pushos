//! One `codex app-server` process, shared by every thread PushOS opens.
//!
//! This is the whole point of the adapter. The Agent Client Protocol path
//! starts one agent process per session, which is what a Claude Code session
//! costs and why a dozen of them fill a laptop. Codex's app-server instead
//! hosts many threads in a single process: measured on an M4, sixty-four idle
//! threads live in about 30 MB of resident memory in one process, and a thread
//! opens in roughly a tenth of a second.
//!
//! The protocol is JSON-RPC in newline-delimited JSON over the child's stdio.
//! Three kinds of message come back and each is handled differently:
//!
//! - a **response**, matched to the call waiting on it by id;
//! - a **notification**, which is something a thread did;
//! - a **request from the server**, which is the agent asking permission and
//!   which nothing may drop, because the thread stops until it is answered.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::CodexError;

/// How many calls may be in flight before the adapter refuses more.
///
/// A bound rather than a hope: a server that stopped answering would otherwise
/// grow this map for as long as PushOS ran. Far above what sixty-four threads
/// generate, and small enough to notice.
const MOST_IN_FLIGHT: usize = 256;

/// How long a call waits before giving up on the server.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How many messages from the server may queue before the reader slows down.
///
/// Back-pressure rather than an unbounded queue: a burst of output from a busy
/// fleet must not become memory PushOS never gets back.
const INCOMING_QUEUE: usize = 512;

/// The calls still waiting for an answer, by the id each was sent under.
type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, CodexError>>>>>;

/// Something the server said that was not an answer to a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// A thread did something.
    Notification {
        /// The method, such as `item/agentMessage/delta`.
        method: String,
        /// Whatever came with it.
        params: Value,
    },
    /// The agent is asking to be allowed to do something.
    ///
    /// Carries the id the answer must quote. Until [`AppServer::respond`] is
    /// called with it, the thread that asked is stopped.
    Request {
        /// The id to answer with.
        id: Value,
        /// The method, such as `execCommandApproval`.
        method: String,
        /// Whatever came with it.
        params: Value,
    },
}

/// A running `codex app-server`.
#[derive(Debug)]
pub struct AppServer {
    stdin: Mutex<ChildStdin>,
    pending: Waiting,
    next_id: AtomicU64,
    child: Mutex<Child>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl AppServer {
    /// Starts the server and negotiates the connection.
    ///
    /// Returns the server and the stream of everything it says that is not an
    /// answer. The caller owns that receiver; dropping it stops the adapter
    /// hearing about permission questions, so it must be held for as long as
    /// any thread is open.
    pub async fn start(
        program: &str,
        args: &[String],
    ) -> Result<(Arc<Self>, mpsc::Receiver<Incoming>), CodexError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| CodexError::CouldNotStart {
                program: program.to_owned(),
                source: Arc::new(source),
            })?;

        let stdin = child.stdin.take().ok_or(CodexError::NoPipe)?;
        let stdout = child.stdout.take().ok_or(CodexError::NoPipe)?;

        let pending: Waiting = Arc::new(Mutex::new(HashMap::new()));
        let (sender, incoming) = mpsc::channel(INCOMING_QUEUE);

        let reader = tokio::spawn(read_from(stdout, Arc::clone(&pending), sender));

        let server = Arc::new(Self {
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            reader: Mutex::new(Some(reader)),
        });

        server
            .call(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "pushos",
                        "title": "PushOS",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;

        Ok((server, incoming))
    }

    /// Calls a method and waits for its answer.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CodexError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= MOST_IN_FLIGHT {
                return Err(CodexError::TooBusy {
                    in_flight: pending.len(),
                });
            }
            pending.insert(id, reply);
        }

        let written = self
            .write(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
            .await;

        if let Err(error) = written {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }

        match tokio::time::timeout(CALL_TIMEOUT, answer).await {
            Ok(Ok(result)) => result,
            // The reader dropped the sender, which only happens when the
            // server's output ended: the process is gone.
            Ok(Err(_)) => Err(CodexError::Stopped),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(CodexError::TimedOut {
                    method: method.to_owned(),
                })
            }
        }
    }

    /// Answers a request the server made.
    ///
    /// The thread that asked is stopped until this arrives, so a failure here
    /// is reported rather than logged and forgotten.
    pub async fn respond(&self, id: Value, result: Value) -> Result<(), CodexError> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await
    }

    async fn write(&self, message: &Value) -> Result<(), CodexError> {
        let mut line = serde_json::to_string(message).map_err(|source| CodexError::Malformed {
            detail: source.to_string(),
        })?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|_| CodexError::Stopped)?;
        stdin.flush().await.map_err(|_| CodexError::Stopped)
    }

    /// The operating system's id for the server process.
    ///
    /// `None` once it has been reaped. Wanted for diagnostics: what a fleet
    /// costs is a property of this one process, so it has to be nameable.
    pub async fn pid(&self) -> Option<u32> {
        self.child.lock().await.id()
    }

    /// Stops the server and the task reading from it.
    ///
    /// Idempotent: called on shutdown and again when the last thread closes.
    pub async fn shutdown(&self) {
        if let Some(reader) = self.reader.lock().await.take() {
            reader.abort();
        }
        let mut child = self.child.lock().await;
        if let Err(error) = child.kill().await {
            debug!(%error, "the app-server had already gone");
        }
    }
}

/// Reads the server's output until it ends, routing each message.
async fn read_from(
    stdout: tokio::process::ChildStdout,
    pending: Waiting,
    incoming: mpsc::Sender<Incoming>,
) {
    let mut lines = BufReader::new(stdout).lines();

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                warn!(%error, "the app-server's output could not be read");
                break;
            }
        };

        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            // The server prints the odd non-JSON line; it is not ours to
            // interpret, and guessing would be worse than ignoring.
            debug!(line = %truncate(&line), "ignoring a line that is not JSON-RPC");
            continue;
        };

        match classify(&message) {
            Some(Routed::Answer { id, result }) => {
                if let Some(reply) = pending.lock().await.remove(&id) {
                    // The caller may have timed out and gone; that is not an
                    // error, only a late answer nobody is waiting for.
                    drop(reply.send(result));
                }
            }
            Some(Routed::Said(message)) => {
                if incoming.send(message).await.is_err() {
                    break;
                }
            }
            None => debug!(line = %truncate(&line), "ignoring an unrecognised message"),
        }
    }

    // The server has gone. Everything still waiting is told, rather than left
    // to time out one call at a time.
    for (_, reply) in pending.lock().await.drain() {
        drop(reply.send(Err(CodexError::Stopped)));
    }
}

/// What one message from the server turned out to be.
enum Routed {
    /// An answer to a call PushOS made.
    Answer {
        id: u64,
        result: Result<Value, CodexError>,
    },
    /// Anything the server said of its own accord.
    Said(Incoming),
}

/// Sorts one message by shape.
///
/// A response carries an id and a result or an error. A request carries an id
/// and a method. A notification carries a method and no id. Anything else is
/// not something this protocol defines.
fn classify(message: &Value) -> Option<Routed> {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id");

    match (method, id) {
        (Some(method), Some(id)) => Some(Routed::Said(Incoming::Request {
            id: id.clone(),
            method: method.to_owned(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        })),
        (Some(method), None) => Some(Routed::Said(Incoming::Notification {
            method: method.to_owned(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        })),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let result = message.get("error").map_or_else(
                || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                |failure| {
                    Err(CodexError::Refused {
                        detail: failure
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("the server gave no reason")
                            .to_owned(),
                    })
                },
            );
            Some(Routed::Answer { id, result })
        }
        (None, None) => None,
    }
}

/// Keeps a log line short enough to read.
fn truncate(line: &str) -> String {
    const MOST: usize = 200;
    if line.len() <= MOST {
        return line.to_owned();
    }
    let mut cut = MOST;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &line[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_matched_to_the_call_that_is_waiting() {
        let message = json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": true}});
        match classify(&message) {
            Some(Routed::Answer { id, result }) => {
                assert_eq!(id, 7);
                assert_eq!(result.expect("succeeded"), json!({"ok": true}));
            }
            _ => panic!("a response should be an answer"),
        }
    }

    #[test]
    fn a_refusal_keeps_the_reason_the_server_gave() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "error": {"code": -32601, "message": "thread/items/list is not supported yet"},
        });
        match classify(&message) {
            Some(Routed::Answer { result, .. }) => {
                let error = result.expect_err("an error response is a failure");
                assert!(error.to_string().contains("not supported yet"));
            }
            _ => panic!("an error response should be an answer"),
        }
    }

    #[test]
    fn a_notification_has_a_method_and_no_id() {
        let message = json!({
            "jsonrpc": "2.0",
            "method": "item/agentMessage/delta",
            "params": {"threadId": "t1", "delta": "hello"},
        });
        match classify(&message) {
            Some(Routed::Said(Incoming::Notification { method, params })) => {
                assert_eq!(method, "item/agentMessage/delta");
                assert_eq!(params["threadId"], "t1");
            }
            _ => panic!("a notification should be recognised"),
        }
    }

    #[test]
    fn a_question_from_the_server_keeps_the_id_its_answer_must_quote() {
        // Losing this id would leave the thread that asked stopped for good.
        let message = json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "execCommandApproval",
            "params": {"conversationId": "t1", "command": ["rm", "-rf", "/"]},
        });
        match classify(&message) {
            Some(Routed::Said(Incoming::Request { id, method, .. })) => {
                assert_eq!(id, json!("req-1"));
                assert_eq!(method, "execCommandApproval");
            }
            _ => panic!("a server request should keep its id"),
        }
    }

    #[test]
    fn a_message_that_is_neither_is_ignored_rather_than_guessed_at() {
        assert!(classify(&json!({"jsonrpc": "2.0"})).is_none());
        assert!(classify(&json!({"id": "not a number", "result": 1})).is_none());
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let line = "é".repeat(400);
        let cut = truncate(&line);
        assert!(cut.len() <= 203, "{} bytes", cut.len());
        assert!(cut.ends_with('…'));
    }
}
