//! The agent port, implemented over one shared app-server.
//!
//! Every session here is a thread in the same process, so opening the
//! sixty-fourth costs what the first did. The backend owns two things beyond
//! that: which PushOS session is which Codex thread, and which permission
//! questions are still unanswered — because a thread that has asked one is
//! stopped until it hears back.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::agent::AgentState;
use pushos_domain::ids::{ProviderName, SessionId};
use pushos_domain::ports::{
    AgentBackend, AgentCapabilities, AgentError, AgentEvent, AgentObserver, ApprovalId,
    SessionHandle, SessionRequest,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::error::CodexError;
use crate::mapping::{self, Translated};
use crate::server::{AppServer, Incoming};

/// How many unanswered permission questions are kept.
///
/// A question nobody answers holds its thread for ever, so this is a bound on
/// a leak rather than on a workload: sixty-four threads cannot each have more
/// than a few outstanding, and far past that something is wrong.
const MOST_UNANSWERED: usize = 512;

/// Codex, reached through one app-server process.
///
/// The process is not started until something is actually asked of it.
/// Configuration is read at startup and by `pushos check`, and neither of
/// those is a reason to have a Codex running.
#[derive(Debug)]
pub struct CodexBackend {
    provider: ProviderName,
    program: String,
    args: Vec<String>,
    observer: Arc<dyn AgentObserver>,
    threads: Arc<Mutex<HashMap<SessionId, String>>>,
    running: Mutex<Option<Running>>,
}

/// The process, once there is one.
#[derive(Debug)]
struct Running {
    server: Arc<AppServer>,
    listener: JoinHandle<()>,
}

impl CodexBackend {
    /// Describes a Codex the adapter can start when it is needed.
    ///
    /// One of these serves every Codex session PushOS runs. A second would be
    /// a second process, which is the thing this adapter exists to avoid.
    pub fn new(
        provider: impl Into<ProviderName>,
        program: impl Into<String>,
        args: Vec<String>,
        observer: Arc<dyn AgentObserver>,
    ) -> Self {
        Self {
            provider: provider.into(),
            program: program.into(),
            args,
            observer,
            threads: Arc::new(Mutex::new(HashMap::new())),
            running: Mutex::new(None),
        }
    }

    /// The server, started if this is the first thing to want it.
    ///
    /// The lock is held across the start so two sessions opening at once
    /// cannot each get a process of their own.
    async fn server(&self) -> Result<Arc<AppServer>, CodexError> {
        let mut running = self.running.lock().await;
        if let Some(already) = running.as_ref() {
            return Ok(Arc::clone(&already.server));
        }

        let (server, incoming) = AppServer::start(&self.program, &self.args).await?;
        let listener = tokio::spawn(listen(
            Arc::clone(&server),
            Arc::clone(&self.threads),
            Arc::clone(&self.observer),
            incoming,
        ));

        info!(
            provider = %self.provider,
            program = %self.program,
            "one app-server now serves every Codex thread"
        );

        *running = Some(Running {
            server: Arc::clone(&server),
            listener,
        });
        Ok(server)
    }

    /// The Codex thread a session is running as.
    async fn thread(&self, session: &SessionId) -> Result<String, AgentError> {
        self.threads
            .lock()
            .await
            .get(session)
            .cloned()
            .ok_or_else(|| AgentError::NoSuchSession {
                session: session.clone(),
            })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, AgentError> {
        let server = self
            .server()
            .await
            .map_err(|error| error.into_agent_error(&self.provider))?;
        server
            .call(method, params)
            .await
            .map_err(|error| error.into_agent_error(&self.provider))
    }

    /// The process every thread of this backend lives in.
    ///
    /// `None` until something has asked Codex for anything.
    pub async fn pid(&self) -> Option<u32> {
        let running = self.running.lock().await;
        let server = Arc::clone(&running.as_ref()?.server);
        drop(running);
        server.pid().await
    }

    /// Stops the server and the task listening to it.
    pub async fn shutdown(&self) {
        let Some(running) = self.running.lock().await.take() else {
            return;
        };
        running.listener.abort();
        running.server.shutdown().await;
    }
}

/// Carries everything the server says to the observer, for as long as it runs.
async fn listen(
    server: Arc<AppServer>,
    threads: Arc<Mutex<HashMap<SessionId, String>>>,
    observer: Arc<dyn AgentObserver>,
    mut incoming: tokio::sync::mpsc::Receiver<Incoming>,
) {
    let mut unanswered: HashMap<ApprovalId, Value> = HashMap::new();

    while let Some(message) = incoming.recv().await {
        let Some(Translated { thread, event }) = mapping::translate(&message) else {
            continue;
        };

        // A question must be answerable later, so the id it has to quote is
        // kept before the operator is ever told about it.
        if let AgentEvent::AskedPermission { request, .. } = &event {
            if unanswered.len() >= MOST_UNANSWERED {
                warn!(
                    held = unanswered.len(),
                    "refusing a permission question: too many are already unanswered"
                );
                deny(&server, &message).await;
                continue;
            }
            if let Incoming::Request { id, .. } = &message {
                unanswered.insert(request.clone(), id.clone());
            }
        }

        let Some(session) = session_of(&threads, &thread).await else {
            debug!(thread, "a message about a thread PushOS did not open");
            continue;
        };

        if let AgentEvent::AskedPermission { .. } = &event {
            observer.observe(
                &session,
                AgentEvent::StateChanged {
                    state: AgentState::WaitingApproval,
                },
            );
        }
        observer.observe(&session, event);
    }

    debug!("the app-server stopped talking");
}

/// Refuses a question rather than leaving its thread stopped for ever.
async fn deny(server: &AppServer, message: &Incoming) {
    if let Incoming::Request { id, .. } = message
        && let Err(error) = server
            .respond(id.clone(), json!({"decision": "denied"}))
            .await
    {
        warn!(%error, "could not refuse a permission question");
    }
}

async fn session_of(
    threads: &Mutex<HashMap<SessionId, String>>,
    thread: &str,
) -> Option<SessionId> {
    threads
        .lock()
        .await
        .iter()
        .find(|(_, open)| open.as_str() == thread)
        .map(|(session, _)| session.clone())
}

#[async_trait]
impl AgentBackend for CodexBackend {
    fn provider(&self) -> ProviderName {
        self.provider.clone()
    }

    async fn capabilities(&self) -> Result<AgentCapabilities, AgentError> {
        let models = self
            .call("model/list", json!({}))
            .await
            .ok()
            .and_then(|answer| {
                let listed = answer.get("data")?.as_array()?;
                Some(
                    listed
                        .iter()
                        .filter_map(|model| model.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect(),
                )
            })
            .unwrap_or_default();

        Ok(AgentCapabilities {
            // A thread outlives the process that opened it: its id is enough
            // to pick it up again, which is what `thread/resume` is for.
            resume: true,
            cancel: true,
            permissions: true,
            terminal: true,
            worktrees: false,
            models,
        })
    }

    async fn start(&self, request: SessionRequest) -> Result<SessionHandle, AgentError> {
        let started = self
            .call(
                "thread/start",
                json!({
                    "cwd": request.cwd.to_string_lossy(),
                    "developerInstructions": request.objective,
                }),
            )
            .await?;

        let thread = started
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CodexError::Malformed {
                    detail: "thread/start answered without a thread id".to_owned(),
                }
                .into_agent_error(&self.provider)
            })?
            .to_owned();

        // Named the way the other adapter names its sessions, so a session id
        // reads the same in the log whichever protocol opened it.
        let session = SessionId::new(format!(
            "{}-{}-{}",
            self.provider,
            request.agent,
            pushos_domain::ids::ExecutionId::generate()
        ));
        self.threads
            .lock()
            .await
            .insert(session.clone(), thread.clone());

        info!(%session, thread, agent = %request.agent, "opened a Codex thread");

        Ok(SessionHandle {
            id: session,
            provider_session: Some(thread),
            provider: self.provider.clone(),
        })
    }

    async fn resume(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let thread = handle
            .provider_session
            .as_ref()
            .ok_or_else(|| AgentError::NoSuchSession {
                session: handle.id.clone(),
            })?;

        self.call("thread/resume", json!({ "threadId": thread }))
            .await?;
        self.threads
            .lock()
            .await
            .insert(handle.id.clone(), thread.clone());
        Ok(())
    }

    async fn prompt(&self, handle: &SessionHandle, text: &str) -> Result<(), AgentError> {
        let thread = self.thread(&handle.id).await?;
        self.call(
            "turn/start",
            json!({
                "threadId": thread,
                "input": [{"type": "text", "text": text}],
            }),
        )
        .await
        .map(drop)
    }

    async fn cancel(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let thread = self.thread(&handle.id).await?;
        self.call("turn/interrupt", json!({ "threadId": thread }))
            .await
            .map(drop)
    }

    async fn answer(
        &self,
        handle: &SessionHandle,
        request: &ApprovalId,
        option: &str,
    ) -> Result<(), AgentError> {
        // The thread is stopped until this lands, so the id has to be the one
        // the server used, written back exactly as it arrived.
        let id = mapping::request_id(&request.0).ok_or_else(|| {
            CodexError::Malformed {
                detail: format!("`{request}` is not an id the app-server gave"),
            }
            .into_agent_error(&self.provider)
        })?;

        let server = self
            .server()
            .await
            .map_err(|error| error.into_agent_error(&self.provider))?;
        server
            .respond(id, json!({ "decision": option }))
            .await
            .map_err(|error| error.into_agent_error(&self.provider))?;

        debug!(session = %handle.id, %request, option, "answered a permission question");
        Ok(())
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), AgentError> {
        let thread = self.thread(&handle.id).await?;
        // Unsubscribing lets the app-server unload the thread; the
        // conversation stays on disk and `resume` picks it up again. This is
        // what makes a parked session cost nothing.
        let outcome = self
            .call("thread/unsubscribe", json!({ "threadId": thread }))
            .await;
        self.threads.lock().await.remove(&handle.id);
        outcome.map(drop)
    }
}
