//! One agent process, driven for the life of one session.
//!
//! The task owns the connection outright: nothing else touches the process, and
//! every instruction arrives as a command with a reply channel. That is what
//! makes a cancel racing a completion a normal event rather than a bug.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use pushos_domain::agent::AgentState;
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{AgentEvent, AgentObserver, ApprovalId, ApprovalOption, StopReason};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::command::SessionCommand;
use crate::launcher::AgentCommand;
use crate::mapping;

/// How long a permission question waits for the operator before it is refused.
///
/// A question nobody answers wedges the agent forever. Refusing eventually is
/// worse than a prompt answer and far better than a session that never ends.
const APPROVAL_PATIENCE: Duration = Duration::from_mins(15);

/// How many commands may queue for one session.
const COMMAND_CAPACITY: usize = 32;

/// The answers a parked permission question is waiting for.
pub(crate) type PendingAnswers = Arc<Mutex<HashMap<String, oneshot::Sender<Option<String>>>>>;

/// A running session, as the backend holds it.
#[derive(Debug)]
pub(crate) struct RunningSession {
    commands: mpsc::Sender<SessionCommand>,
    pending: PendingAnswers,
}

impl RunningSession {
    /// Sends a command and waits for the session to say whether it took it.
    pub(crate) async fn send(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<(), String>>) -> SessionCommand,
    ) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| "the session has stopped".to_owned())?;
        answer
            .await
            .map_err(|_| "the session stopped before answering".to_owned())?
    }

    /// A sender for this session's commands.
    pub(crate) fn clone_sender(&self) -> mpsc::Sender<SessionCommand> {
        self.commands.clone()
    }

    /// The questions this session has parked.
    pub(crate) fn clone_pending(&self) -> PendingAnswers {
        Arc::clone(&self.pending)
    }
}

/// Hands an answer to a question the agent is waiting on.
///
/// Returns whether a question with that identity was actually parked; an answer
/// to nothing must not be reported as delivered.
pub(crate) fn deliver_answer(pending: &PendingAnswers, request: &ApprovalId, option: &str) -> bool {
    let Ok(mut parked) = pending.lock() else {
        return false;
    };
    match parked.remove(&request.0) {
        Some(sender) => sender.send(Some(option.to_owned())).is_ok(),
        None => false,
    }
}

/// Starts an agent process and drives it until it is told to stop.
///
/// Returns once the process is up and a session has been opened, so a caller
/// knows the agent is real before anything is bound to it.
pub(crate) async fn launch(
    command: AgentCommand,
    session: SessionId,
    cwd: std::path::PathBuf,
    objective: String,
    observer: Arc<dyn AgentObserver>,
) -> Result<RunningSession, String> {
    let (commands, inbox) = mpsc::channel(COMMAND_CAPACITY);
    let pending: PendingAnswers = Arc::new(Mutex::new(HashMap::new()));
    let (ready, opened) = oneshot::channel();

    let running = RunningSession {
        commands,
        pending: Arc::clone(&pending),
    };

    tokio::spawn(run(
        command, session, cwd, objective, observer, inbox, pending, ready,
    ));

    // Waiting for the session to open, rather than returning optimistically,
    // means a provider that is not installed fails where the operator can see
    // it instead of on the next pad press.
    opened
        .await
        .map_err(|_| "the agent stopped before opening a session".to_owned())??;
    Ok(running)
}

#[allow(clippy::too_many_arguments)]
async fn run(
    command: AgentCommand,
    session: SessionId,
    cwd: std::path::PathBuf,
    objective: String,
    observer: Arc<dyn AgentObserver>,
    mut inbox: mpsc::Receiver<SessionCommand>,
    pending: PendingAnswers,
    ready: oneshot::Sender<Result<(), String>>,
) {
    let agent = match build_agent(&command) {
        Ok(agent) => agent,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let notify = {
        let observer = Arc::clone(&observer);
        let session = session.clone();
        move |update: SessionNotification| {
            if let Some(event) = mapping::session_update(&update.update) {
                observer.observe(&session, event);
            }
        }
    };

    let permission = PermissionHandler {
        session: session.clone(),
        observer: Arc::clone(&observer),
        pending,
    };

    let outcome = Client
        .builder()
        .name("pushos")
        .on_receive_notification(
            {
                let notify = notify.clone();
                async move |notification: SessionNotification, _cx| {
                    notify(notification);
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let permission = permission.clone();
                async move |request: RequestPermissionRequest, responder, _cx| {
                    permission.handle(request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| {
            let session = session.clone();
            let observer = Arc::clone(&observer);
            async move {
                serve(
                    connection, session, cwd, objective, observer, &mut inbox, ready,
                )
                .await
            }
        })
        .await;

    if let Err(error) = outcome {
        warn!(%error, "the agent connection ended with an error");
    }
}

/// Opens the session, then carries out commands until told to stop.
async fn serve(
    connection: ConnectionTo<Agent>,
    session: SessionId,
    cwd: std::path::PathBuf,
    objective: String,
    observer: Arc<dyn AgentObserver>,
    inbox: &mut mpsc::Receiver<SessionCommand>,
    ready: oneshot::Sender<Result<(), String>>,
) -> agent_client_protocol::Result<()> {
    if let Err(error) = connection
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await
    {
        let _ = ready.send(Err(format!("the agent would not start: {error}")));
        return Err(error);
    }

    let opened = match connection
        .send_request(NewSessionRequest::new(cwd))
        .block_task()
        .await
    {
        Ok(opened) => opened,
        Err(error) => {
            let _ = ready.send(Err(format!("the agent would not open a session: {error}")));
            return Err(error);
        }
    };

    let wire_session = opened.session_id;
    info!(%session, "agent session opened");
    let _ = ready.send(Ok(()));
    observer.observe(
        &session,
        AgentEvent::StateChanged {
            state: AgentState::Sleeping,
        },
    );

    // The role's standing instruction goes first, so everything after it is the
    // operator's own words rather than setup.
    if !objective.trim().is_empty() {
        let _ = send_prompt(&connection, &wire_session, &objective).await;
    }

    while let Some(command) = inbox.recv().await {
        match command {
            SessionCommand::Prompt { text, reply } => {
                observer.observe(
                    &session,
                    AgentEvent::StateChanged {
                        state: AgentState::Working,
                    },
                );
                let _ = reply.send(Ok(()));

                let stop = send_prompt(&connection, &wire_session, &text).await;
                let reason = match stop {
                    Ok(reason) => mapping::stop_reason(&reason),
                    Err(error) => {
                        warn!(%error, %session, "the agent failed during a turn");
                        StopReason::Failed
                    }
                };
                observer.observe(&session, AgentEvent::Ended { reason });
            }

            SessionCommand::Cancel { reply } => {
                // The protocol answers a cancel with a stop reason on the turn
                // itself, so nothing is reported here beyond the send.
                let sent = connection
                    .send_notification(agent_client_protocol::schema::v1::CancelNotification::new(
                        wire_session.clone(),
                    ))
                    .map_err(|error| error.to_string());
                let _ = reply.send(sent);
            }

            SessionCommand::Stop { reply } => {
                let _ = reply.send(Ok(()));
                debug!(%session, "agent session stopping");
                break;
            }
        }
    }

    // Anything still queued is refused rather than dropped, so a caller waiting
    // on a reply learns the session has gone instead of waiting forever.
    inbox.close();
    while let Some(orphan) = inbox.recv().await {
        orphan.refuse("the session has ended");
    }

    observer.observe(
        &session,
        AgentEvent::StateChanged {
            state: AgentState::Offline,
        },
    );
    Ok(())
}

async fn send_prompt(
    connection: &ConnectionTo<Agent>,
    session: &agent_client_protocol::schema::v1::SessionId,
    text: &str,
) -> agent_client_protocol::Result<agent_client_protocol::schema::v1::StopReason> {
    let response = connection
        .send_request(PromptRequest::new(
            session.clone(),
            vec![ContentBlock::Text(TextContent::new(text.to_owned()))],
        ))
        .block_task()
        .await?;
    Ok(response.stop_reason)
}

/// Parks a permission question until the operator decides.
#[derive(Clone)]
struct PermissionHandler {
    session: SessionId,
    observer: Arc<dyn AgentObserver>,
    pending: PendingAnswers,
}

impl PermissionHandler {
    async fn handle(
        &self,
        request: RequestPermissionRequest,
        responder: agent_client_protocol::Responder<RequestPermissionResponse>,
    ) -> agent_client_protocol::Result<()> {
        let id = format!("{}-{}", self.session, request.tool_call.tool_call_id);
        let options: Vec<ApprovalOption> = request
            .options
            .iter()
            .map(|option| ApprovalOption {
                id: option.option_id.to_string(),
                label: option.name.clone(),
                // The protocol distinguishes allowing from rejecting by kind;
                // anything that is not an explicit rejection lets work proceed.
                allows: !format!("{:?}", option.kind)
                    .to_lowercase()
                    .contains("reject"),
            })
            .collect();

        let (answer, decided) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id.clone(), answer);
        }

        self.observer.observe(
            &self.session,
            AgentEvent::AskedPermission {
                request: ApprovalId(id.clone()),
                question: request
                    .tool_call
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| "allow this?".to_owned()),
                options: options.clone(),
            },
        );

        // A question nobody answers would wedge the agent forever.
        let chosen = match tokio::time::timeout(APPROVAL_PATIENCE, decided).await {
            Ok(Ok(chosen)) => chosen,
            Ok(Err(_)) | Err(_) => None,
        };

        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }

        match chosen.and_then(|choice| {
            request
                .options
                .iter()
                .find(|option| option.option_id.to_string() == choice)
                .map(|option| option.option_id.clone())
        }) {
            Some(option) => responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option)),
            )),
            None => responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            )),
        }
    }
}

fn build_agent(command: &AgentCommand) -> Result<AcpAgent, String> {
    if !command.is_runnable() {
        return Err("the provider has no program to run".to_owned());
    }

    let mut config = agent_client_protocol::AcpAgentConfig::new(&command.program);
    for argument in &command.args {
        config = config.arg(argument);
    }
    for (name, value) in &command.env {
        config = config.env(name, value);
    }
    Ok(AcpAgent::new(config))
}
