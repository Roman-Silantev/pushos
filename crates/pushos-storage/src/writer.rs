//! The single owner of the database connection.
//!
//! Every read and write goes through one task, so ordering is obvious and there
//! is never a question of who holds a lock. SQLite is synchronous, so the task
//! is a dedicated thread and callers hand over work without blocking.

use std::path::Path;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use rusqlite::Connection;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::connection;
use crate::error::StorageError;
use crate::records::{EventRecord, WorkspaceMemoryRow};

/// How much work may queue before callers are told the writer is behind.
///
/// Sized so a burst of activity is absorbed. A full queue means something is
/// wrong, and dropping an audit record silently would be worse than saying so.
const QUEUE_CAPACITY: usize = 1_024;

/// A request for the storage task.
enum Request {
    RecordEvent(Box<EventRecord>),
    RememberWorkspace(Box<WorkspaceMemoryRow>),
    RecallWorkspace(String, oneshot::Sender<Option<WorkspaceMemoryRow>>),
    PutSetting(String, String),
    GetSetting(String, oneshot::Sender<Option<String>>),
    RecentEvents(usize, oneshot::Sender<Vec<EventRecord>>),
    /// Stop after everything already queued has been committed.
    Shutdown,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::RecordEvent(_) => "RecordEvent",
            Self::RememberWorkspace(_) => "RememberWorkspace",
            Self::RecallWorkspace(..) => "RecallWorkspace",
            Self::PutSetting(..) => "PutSetting",
            Self::GetSetting(..) => "GetSetting",
            Self::RecentEvents(..) => "RecentEvents",
            Self::Shutdown => "Shutdown",
        };
        f.write_str(name)
    }
}

/// A handle to the storage task.
///
/// Cloning shares one task and one connection.
#[derive(Debug, Clone)]
pub struct Storage {
    requests: SyncSender<Request>,
}

/// Owns the storage task and stops it when dropped.
#[derive(Debug)]
pub struct StorageWriter {
    storage: Storage,
    worker: Option<JoinHandle<()>>,
}

impl StorageWriter {
    /// Opens the database at `path` and starts the task.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        Self::start(connection::open(path)?)
    }

    /// Opens a database that exists only for the lifetime of this writer.
    pub fn in_memory() -> Result<Self, StorageError> {
        Self::start(connection::open_in_memory()?)
    }

    /// A handle for issuing work.
    pub fn handle(&self) -> Storage {
        self.storage.clone()
    }

    fn start(connection: Connection) -> Result<Self, StorageError> {
        let (requests, inbox) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker = thread::Builder::new()
            .name("pushos-storage".to_owned())
            .spawn(move || serve(&connection, &inbox))
            .map_err(|source| StorageError::Unwritable {
                path: std::path::PathBuf::from("<storage thread>"),
                source,
            })?;

        Ok(Self {
            storage: Storage { requests },
            worker: Some(worker),
        })
    }
}

impl Drop for StorageWriter {
    fn drop(&mut self) {
        // Shutdown is an explicit message rather than a dropped sender: handles
        // are cloneable and may outlive the writer, so waiting for the channel
        // to disconnect would wait forever. Sending blocks only until the queue
        // has room, and the message is served in order, so everything already
        // queued is committed first.
        if self.storage.requests.send(Request::Shutdown).is_err() {
            debug!("the storage task had already stopped");
        }

        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            warn!("the storage thread ended abnormally");
        }
    }
}

impl Storage {
    /// Appends one row to the audit trail.
    pub fn record_event(&self, record: EventRecord) -> Result<(), StorageError> {
        self.send(Request::RecordEvent(Box::new(record)))
    }

    /// Saves what a workspace should restore.
    pub fn remember_workspace(&self, row: WorkspaceMemoryRow) -> Result<(), StorageError> {
        self.send(Request::RememberWorkspace(Box::new(row)))
    }

    /// Reads what a workspace should restore.
    pub async fn recall_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceMemoryRow>, StorageError> {
        self.ask(|reply| Request::RecallWorkspace(workspace_id.to_owned(), reply))
            .await
    }

    /// Stores a setting.
    pub fn put_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.send(Request::PutSetting(key.to_owned(), value.to_owned()))
    }

    /// Reads a setting.
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        self.ask(|reply| Request::GetSetting(key.to_owned(), reply))
            .await
    }

    /// Reads the most recent events, newest first.
    pub async fn recent_events(&self, limit: usize) -> Result<Vec<EventRecord>, StorageError> {
        self.ask(|reply| Request::RecentEvents(limit, reply)).await
    }

    fn send(&self, request: Request) -> Result<(), StorageError> {
        self.requests
            .try_send(request)
            .map_err(|error| match error {
                TrySendError::Full(request) => {
                    warn!(?request, "storage queue is full");
                    StorageError::query(
                        "the storage queue is full",
                        rusqlite::Error::QueryReturnedNoRows,
                    )
                }
                TrySendError::Disconnected(_) => StorageError::Stopped,
            })
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> Result<T, StorageError> {
        let (reply, answer) = oneshot::channel();
        self.send(build(reply))?;
        answer.await.map_err(|_| StorageError::Stopped)
    }
}

fn serve(connection: &Connection, inbox: &mpsc::Receiver<Request>) {
    while let Ok(request) = inbox.recv() {
        if matches!(request, Request::Shutdown) {
            break;
        }
        // One failed statement is not a reason to stop serving: the audit trail
        // matters, but so does the surface staying up.
        if let Err(error) = handle(connection, request) {
            warn!(%error, "storage request failed");
        }
    }
    debug!("storage task stopped");
}

fn handle(connection: &Connection, request: Request) -> Result<(), StorageError> {
    match request {
        Request::RecordEvent(record) => {
            connection
                .execute(
                    "INSERT OR IGNORE INTO events
                       (id, correlation_id, causation_id, recorded_at, source, kind, detail)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        record.id,
                        record.correlation_id,
                        record.causation_id,
                        record.recorded_at,
                        record.source,
                        record.kind,
                        record.detail,
                    ],
                )
                .map_err(|source| StorageError::query("recording an event", source))?;
        }

        Request::RememberWorkspace(row) => {
            connection
                .execute(
                    "INSERT INTO workspace_memory
                       (workspace_id, last_page, selected_session, updated_at)
                     VALUES (?1, ?2, ?3, unixepoch())
                     ON CONFLICT(workspace_id) DO UPDATE SET
                       last_page = excluded.last_page,
                       selected_session = excluded.selected_session,
                       updated_at = excluded.updated_at",
                    rusqlite::params![row.workspace_id, row.last_page, row.selected_session],
                )
                .map_err(|source| StorageError::query("remembering a workspace", source))?;
        }

        Request::RecallWorkspace(workspace_id, reply) => {
            let row = connection
                .query_row(
                    "SELECT workspace_id, last_page, selected_session
                       FROM workspace_memory WHERE workspace_id = ?1",
                    [&workspace_id],
                    |row| {
                        Ok(WorkspaceMemoryRow {
                            workspace_id: row.get(0)?,
                            last_page: row.get(1)?,
                            selected_session: row.get(2)?,
                        })
                    },
                )
                .ok();
            let _ = reply.send(row);
        }

        Request::PutSetting(key, value) => {
            connection
                .execute(
                    "INSERT INTO settings (key, value, updated_at)
                     VALUES (?1, ?2, unixepoch())
                     ON CONFLICT(key) DO UPDATE SET
                       value = excluded.value, updated_at = excluded.updated_at",
                    rusqlite::params![key, value],
                )
                .map_err(|source| StorageError::query("storing a setting", source))?;
        }

        Request::GetSetting(key, reply) => {
            let value = connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })
                .ok();
            let _ = reply.send(value);
        }

        Request::RecentEvents(limit, reply) => {
            let events = read_recent(connection, limit).unwrap_or_default();
            let _ = reply.send(events);
        }

        // Handled by the serve loop, which stops rather than dispatching it.
        Request::Shutdown => {}
    }
    Ok(())
}

fn read_recent(connection: &Connection, limit: usize) -> Result<Vec<EventRecord>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, correlation_id, causation_id, recorded_at, source, kind, detail
               FROM events ORDER BY recorded_at DESC, rowid DESC LIMIT ?1",
        )
        .map_err(|source| StorageError::query("reading recent events", source))?;

    let rows = statement
        .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(EventRecord {
                id: row.get(0)?,
                correlation_id: row.get(1)?,
                causation_id: row.get(2)?,
                recorded_at: row.get(3)?,
                source: row.get(4)?,
                kind: row.get(5)?,
                detail: row.get(6)?,
            })
        })
        .map_err(|source| StorageError::query("reading recent events", source))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| StorageError::query("reading recent events", source))
}
