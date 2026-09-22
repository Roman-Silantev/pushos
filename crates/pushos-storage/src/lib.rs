//! Local persistence.
//!
//! SQLite is the runtime's local truth. One task owns the connection and every
//! write goes through it, so there is a single, obvious answer to who may write
//! and in what order.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod connection;
mod error;
mod memory;
mod notes;
mod records;
mod retention;
mod runs;
mod schema;
mod writer;

pub use error::StorageError;
pub use notes::NoteRow;
pub use records::{EventRecord, WorkspaceMemoryRow};
pub use runs::{RunRow, TransitionRow};
pub use writer::{Storage, StorageWriter};

/// Whether the database is there and readable, without writing to it.
///
/// For a diagnostic. Opening it the way PushOS does is not a read: that trims
/// what has expired and may vacuum the file, which makes `doctor` a second
/// writer against a database the running PushOS owns — and, on a healthy
/// machine, deletes rows from the operator's own audit trail for the sake of
/// looking at it.
///
/// `Ok(false)` means there is no database yet, which is the ordinary state of
/// a machine that has not run PushOS and not a fault.
pub fn readable(path: &std::path::Path) -> Result<bool, StorageError> {
    if !path.exists() {
        return Ok(false);
    }

    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(StorageError::open)?;

    // Enough to prove the file is a database this PushOS understands, and
    // nothing that writes.
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StorageError::open)?;
    if version > schema::target_version() {
        return Err(StorageError::open(rusqlite::Error::InvalidQuery));
    }
    Ok(true)
}
