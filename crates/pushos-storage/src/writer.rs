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
use crate::notes::{self, NoteRow, SearchRequest};
use crate::records::{EventRecord, WorkspaceMemoryRow};
use crate::runs::{RunRow, TransitionRow};

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
    /// Write a workflow run and say when it is safe to lose the process.
    ///
    /// Acknowledged rather than queued, because the engine runs the next step
    /// only once this one is on disk.
    SaveRun(Box<RunRow>, oneshot::Sender<Result<(), StorageError>>),
    AppendTransition(
        Box<TransitionRow>,
        oneshot::Sender<Result<(), StorageError>>,
    ),
    UnfinishedRuns(oneshot::Sender<Result<Vec<RunRow>, StorageError>>),
    ForgetRun(String, oneshot::Sender<Result<(), StorageError>>),
    /// Index one note, replacing whatever was there under the same identity.
    SaveNote(Box<NoteRow>, oneshot::Sender<Result<(), StorageError>>),
    SearchNotes(
        Box<SearchRequest>,
        oneshot::Sender<Result<Vec<pushos_domain::memory::Excerpt>, StorageError>>,
    ),
    ForgetNote(String, oneshot::Sender<Result<(), StorageError>>),
    EmptySource(String, oneshot::Sender<Result<(), StorageError>>),
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
            Self::SaveRun(..) => "SaveRun",
            Self::AppendTransition(..) => "AppendTransition",
            Self::UnfinishedRuns(..) => "UnfinishedRuns",
            Self::ForgetRun(..) => "ForgetRun",
            Self::SaveNote(..) => "SaveNote",
            Self::SearchNotes(..) => "SearchNotes",
            Self::ForgetNote(..) => "ForgetNote",
            Self::EmptySource(..) => "EmptySource",
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

    /// Writes a workflow run, returning once it is committed.
    ///
    /// Waited on rather than queued: the engine runs the next step only when
    /// this one is safe to lose the process over.
    pub(crate) async fn save_run(&self, row: RunRow) -> Result<(), StorageError> {
        self.ask(|reply| Request::SaveRun(Box::new(row), reply))
            .await?
    }

    /// Appends one step to a run's history, returning once it is committed.
    pub(crate) async fn append_transition(&self, row: TransitionRow) -> Result<(), StorageError> {
        self.ask(|reply| Request::AppendTransition(Box::new(row), reply))
            .await?
    }

    /// Every run that had not finished.
    pub(crate) async fn unfinished_runs(&self) -> Result<Vec<RunRow>, StorageError> {
        self.ask(Request::UnfinishedRuns).await?
    }

    /// Forgets a run and its history.
    pub(crate) async fn forget_run(&self, id: String) -> Result<(), StorageError> {
        self.ask(|reply| Request::ForgetRun(id, reply)).await?
    }

    /// Indexes one note, returning once it is committed.
    ///
    /// Waited on rather than queued: a note written and immediately searched
    /// for should be found, which is exactly what a capture pad followed by a
    /// search pad does.
    pub(crate) async fn save_note(&self, row: NoteRow) -> Result<(), StorageError> {
        self.ask(|reply| Request::SaveNote(Box::new(row), reply))
            .await?
    }

    /// Finds notes, best first.
    pub(crate) async fn search_notes(
        &self,
        request: SearchRequest,
    ) -> Result<Vec<pushos_domain::memory::Excerpt>, StorageError> {
        self.ask(|reply| Request::SearchNotes(Box::new(request), reply))
            .await?
    }

    /// Forgets one note.
    pub(crate) async fn forget_note(&self, id: String) -> Result<(), StorageError> {
        self.ask(|reply| Request::ForgetNote(id, reply)).await?
    }

    /// Forgets everything indexed from one source.
    pub(crate) async fn empty_source(&self, source: String) -> Result<(), StorageError> {
        self.ask(|reply| Request::EmptySource(source, reply))
            .await?
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

        Request::SaveRun(row, reply) => {
            let _ = reply.send(save_run(connection, &row));
        }

        Request::AppendTransition(row, reply) => {
            let _ = reply.send(append_transition(connection, &row));
        }

        Request::UnfinishedRuns(reply) => {
            let _ = reply.send(unfinished_runs(connection));
        }

        Request::ForgetRun(id, reply) => {
            let _ = reply.send(forget_run(connection, &id));
        }

        Request::SaveNote(row, reply) => {
            let _ = reply.send(save_note(connection, &row));
        }

        Request::SearchNotes(request, reply) => {
            let _ = reply.send(search_notes(connection, &request));
        }

        Request::ForgetNote(id, reply) => {
            let _ = reply.send(
                connection
                    .execute("DELETE FROM notes WHERE id = ?1", [&id])
                    .map(|_| ())
                    .map_err(|source| notes::failed("forgetting a note", source)),
            );
        }

        Request::EmptySource(source, reply) => {
            let _ = reply.send(
                connection
                    .execute("DELETE FROM notes WHERE source_id = ?1", [&source])
                    .map(|_| ())
                    .map_err(|source| notes::failed("emptying a note source", source)),
            );
        }

        // Handled by the serve loop, which stops rather than dispatching it.
        Request::Shutdown => {}
    }
    Ok(())
}

fn save_note(connection: &Connection, row: &NoteRow) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO notes
               (id, source_id, title, body, path, workspace_id, tags, written_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               source_id = excluded.source_id,
               title = excluded.title,
               body = excluded.body,
               path = excluded.path,
               workspace_id = excluded.workspace_id,
               tags = excluded.tags,
               written_at = excluded.written_at",
            rusqlite::params![
                row.id,
                row.source_id,
                row.title,
                row.body,
                row.path,
                row.workspace_id,
                row.tags,
                row.written_at,
            ],
        )
        .map(|_| ())
        .map_err(|source| notes::failed("recording a note", source))
}

fn search_notes(
    connection: &Connection,
    request: &SearchRequest,
) -> Result<Vec<pushos_domain::memory::Excerpt>, StorageError> {
    // Two queries rather than one with a branch in it. The words query has to
    // join the index and order by how well each note matched; the open one has
    // no scores to order by and asks for the latest instead.
    if request.words.is_empty() {
        return recent_notes(connection, request);
    }
    matching_notes(connection, request)
}

/// The notes that matched, best first.
fn matching_notes(
    connection: &Connection,
    request: &SearchRequest,
) -> Result<Vec<pushos_domain::memory::Excerpt>, StorageError> {
    let sql = format!(
        "SELECT {columns},
                snippet(note_words, 1, '', '', '…', 12)
           FROM note_words
           JOIN notes ON notes.rowid = note_words.rowid
          WHERE note_words MATCH ?1
            AND (?2 IS NULL OR notes.workspace_id = ?2)
            AND (?3 IS NULL OR instr(' ' || notes.tags || ' ', ' ' || ?3 || ' ') > 0)
          ORDER BY bm25(note_words, 4.0, 1.0, 2.0)
          LIMIT ?4",
        columns = prefixed(notes::COLUMNS)
    );

    let mut statement = connection
        .prepare(&sql)
        .map_err(|source| notes::failed("preparing a note search", source))?;

    let found = statement
        .query_map(
            rusqlite::params![
                request.words,
                request.workspace_id,
                request.tag,
                request.limit
            ],
            |row| {
                let note = notes::read_row(row)?;
                let snippet: Option<String> = row.get(8)?;
                Ok(notes::excerpt_of(&note, snippet))
            },
        )
        .map_err(|source| notes::failed("searching notes", source))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| notes::failed("reading a note", source))?;

    Ok(found)
}

/// The latest notes, for a search with no words in it.
fn recent_notes(
    connection: &Connection,
    request: &SearchRequest,
) -> Result<Vec<pushos_domain::memory::Excerpt>, StorageError> {
    let sql = format!(
        "SELECT {columns}
           FROM notes
          WHERE (?1 IS NULL OR workspace_id = ?1)
            AND (?2 IS NULL OR instr(' ' || tags || ' ', ' ' || ?2 || ' ') > 0)
          ORDER BY written_at DESC
          LIMIT ?3",
        columns = notes::COLUMNS
    );

    let mut statement = connection
        .prepare(&sql)
        .map_err(|source| notes::failed("preparing a note listing", source))?;

    let found = statement
        .query_map(
            rusqlite::params![request.workspace_id, request.tag, request.limit],
            |row| Ok(notes::excerpt_of(&notes::read_row(row)?, None)),
        )
        .map_err(|source| notes::failed("listing notes", source))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| notes::failed("reading a note", source))?;

    Ok(found)
}

/// Qualifies the note columns, for a query that joins another table.
fn prefixed(columns: &str) -> String {
    columns
        .split(", ")
        .map(|column| format!("notes.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn save_run(connection: &Connection, row: &RunRow) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO workflow_runs
               (id, workflow_id, workspace_id, node_id, state, waiting, outcome,
                steps, last_succeeded, note, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
               node_id = excluded.node_id,
               state = excluded.state,
               waiting = excluded.waiting,
               outcome = excluded.outcome,
               steps = excluded.steps,
               last_succeeded = excluded.last_succeeded,
               note = excluded.note,
               updated_at = excluded.updated_at",
            rusqlite::params![
                row.id,
                row.workflow_id,
                row.workspace_id,
                row.node_id,
                row.state,
                row.waiting,
                row.outcome,
                row.steps,
                i64::from(row.last_succeeded),
                row.note,
                row.started_at,
                row.updated_at,
            ],
        )
        .map(|_| ())
        .map_err(|source| StorageError::query("saving a workflow run", source))
}

fn append_transition(connection: &Connection, row: &TransitionRow) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO workflow_transitions (run_id, from_node, to_node, note, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                row.run_id,
                row.from_node,
                row.to_node,
                row.note,
                row.recorded_at
            ],
        )
        .map(|_| ())
        .map_err(|source| StorageError::query("recording a workflow step", source))
}

fn unfinished_runs(connection: &Connection) -> Result<Vec<RunRow>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, workflow_id, workspace_id, node_id, state, waiting, outcome,
                    steps, last_succeeded, note, started_at, updated_at
               FROM workflow_runs
              WHERE state <> 'finished'
              ORDER BY started_at",
        )
        .map_err(|source| StorageError::query("reading what was running", source))?;

    let rows = statement
        .query_map([], |row| {
            Ok(RunRow {
                id: row.get(0)?,
                workflow_id: row.get(1)?,
                workspace_id: row.get(2)?,
                node_id: row.get(3)?,
                state: row.get(4)?,
                waiting: row.get(5)?,
                outcome: row.get(6)?,
                steps: row.get(7)?,
                last_succeeded: row.get::<_, i64>(8)? != 0,
                note: row.get(9)?,
                started_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .map_err(|source| StorageError::query("reading what was running", source))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| StorageError::query("reading what was running", source))
}

fn forget_run(connection: &Connection, id: &str) -> Result<(), StorageError> {
    connection
        .execute("DELETE FROM workflow_transitions WHERE run_id = ?1", [id])
        .map_err(|source| StorageError::query("forgetting a workflow's steps", source))?;
    connection
        .execute("DELETE FROM workflow_runs WHERE id = ?1", [id])
        .map(|_| ())
        .map_err(|source| StorageError::query("forgetting a workflow run", source))
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
