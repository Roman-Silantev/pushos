//! PushOS Studio's backend.
//!
//! Thin on purpose. Every command opens a connection to a running PushOS, asks
//! one question and closes it again, so Studio holds no state that could drift
//! from what the runtime actually has, and nothing here can keep PushOS alive
//! or hold its configuration open.

use pushos_api::protocol::{
    BindingList, EditReport, Request, Response, StatusReport, TestReport, Vocabulary,
};
use pushos_api::{ClientError, ControlClient};
use pushos_config::{BindingAddress, BindingSpec};
use serde::Serialize;

/// What a command reports when it cannot reach PushOS or is refused.
#[derive(Debug, Serialize)]
pub struct StudioError {
    /// What went wrong, for a person.
    message: String,
    /// The individual problems, when a configuration was rejected.
    problems: Vec<String>,
    /// Whether PushOS simply is not running.
    not_running: bool,
}

impl From<ClientError> for StudioError {
    fn from(error: ClientError) -> Self {
        Self {
            message: error.to_string(),
            problems: error.problems().to_vec(),
            not_running: error.is_not_running(),
        }
    }
}

impl StudioError {
    fn unexpected(response: &Response) -> Self {
        Self {
            message: format!("PushOS answered unexpectedly: {response:?}"),
            problems: Vec::new(),
            not_running: false,
        }
    }
}

/// Opens a connection, asks one thing, and lets the connection go.
async fn ask(request: Request) -> Result<Response, StudioError> {
    let mut client = ControlClient::connect_default().await?;
    // Refuse to talk to a runtime speaking a protocol this build does not
    // understand, rather than sending edits it might read differently.
    client.handshake().await?;
    Ok(client.send(&request).await?)
}

/// How the runtime is doing.
#[tauri::command]
async fn status() -> Result<StatusReport, StudioError> {
    let mut client = ControlClient::connect_default().await?;
    Ok(client.handshake().await?)
}

/// Every control, gesture, page and action available.
#[tauri::command]
async fn describe() -> Result<Vocabulary, StudioError> {
    match ask(Request::Describe).await? {
        Response::Vocabulary(vocabulary) => Ok(*vocabulary),
        Response::Failed(failure) => Err(ClientError::Refused(failure).into()),
        other => Err(StudioError::unexpected(&other)),
    }
}

/// Every binding currently configured.
#[tauri::command]
async fn bindings() -> Result<BindingList, StudioError> {
    match ask(Request::Bindings).await? {
        Response::Bindings(list) => Ok(list),
        Response::Failed(failure) => Err(ClientError::Refused(failure).into()),
        other => Err(StudioError::unexpected(&other)),
    }
}

/// Writes a binding and makes it take effect.
#[tauri::command]
async fn bind(spec: BindingSpec) -> Result<EditReport, StudioError> {
    match ask(Request::Bind {
        spec: Box::new(spec),
    })
    .await?
    {
        Response::Edited(report) => Ok(report),
        Response::Failed(failure) => Err(ClientError::Refused(failure).into()),
        other => Err(StudioError::unexpected(&other)),
    }
}

/// Removes a binding.
#[tauri::command]
async fn unbind(address: BindingAddress) -> Result<EditReport, StudioError> {
    match ask(Request::Unbind { address }).await? {
        Response::Edited(report) => Ok(report),
        Response::Failed(failure) => Err(ClientError::Refused(failure).into()),
        other => Err(StudioError::unexpected(&other)),
    }
}

/// Runs a configured binding once, so its effect can be seen before committing
/// to it on the hardware.
#[tauri::command]
async fn test(address: BindingAddress) -> Result<TestReport, StudioError> {
    match ask(Request::Test { address }).await? {
        Response::Tested(report) => Ok(report),
        Response::Failed(failure) => Err(ClientError::Refused(failure).into()),
        other => Err(StudioError::unexpected(&other)),
    }
}

/// Starts Studio.
///
/// # Panics
///
/// Panics if the window cannot be created, which means the graphical
/// environment is unusable and there is nothing sensible to fall back to.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            status, describe, bindings, bind, unbind, test
        ])
        // A window that shows nothing is the hardest kind of failure to report,
        // because there is nowhere on screen to report it. Recording what the
        // webview actually loaded means the answer is in the log instead.
        .on_page_load(|window, payload| {
            eprintln!(
                "studio: webview {:?} {} in {}",
                payload.event(),
                payload.url(),
                window.label()
            );
        })
        .run(tauri::generate_context!())
        .expect("PushOS Studio could not open a window");
}
