//! Opening the database.
//!
//! Every connection is configured the same way, once, here: write-ahead
//! logging so a reader is never blocked by the writer, foreign keys enforced
//! rather than decorative, and a busy timeout so a brief overlap waits instead
//! of failing.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use tracing::{debug, info};

use crate::error::StorageError;
use crate::schema::{MIGRATIONS, target_version};

/// How long a statement waits for a lock before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens the database at `path`, creating and migrating it as needed.
pub(crate) fn open(path: &Path) -> Result<Connection, StorageError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| StorageError::Unwritable {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let connection = Connection::open(path).map_err(StorageError::open)?;
    configure(&connection)?;
    migrate(&connection)?;
    Ok(connection)
}

/// Opens a database that exists only in memory, for tests.
pub(crate) fn open_in_memory() -> Result<Connection, StorageError> {
    let connection = Connection::open_in_memory().map_err(StorageError::open)?;
    configure(&connection)?;
    migrate(&connection)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> Result<(), StorageError> {
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(StorageError::configure)?;
    // An in-memory database has no journal to write ahead of, and asking for
    // WAL there fails; everywhere else it is what keeps reads non-blocking.
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .or_else(|_| connection.pragma_update(None, "journal_mode", "MEMORY"))
        .map_err(StorageError::configure)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(StorageError::configure)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(StorageError::configure)?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<(), StorageError> {
    let current: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StorageError::migrate)?;

    if current > target_version() {
        return Err(StorageError::FromTheFuture {
            found: current,
            expected: target_version(),
        });
    }

    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current)
    {
        debug!(
            version = migration.version,
            name = migration.name,
            "applying migration"
        );
        connection
            .execute_batch(&format!(
                "BEGIN; {} PRAGMA user_version = {}; COMMIT;",
                migration.sql, migration.version
            ))
            .map_err(StorageError::migrate)?;
    }

    if current < target_version() {
        info!(
            from = current,
            to = target_version(),
            "database schema brought forward"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_database_is_migrated_to_the_current_schema() {
        let connection = open_in_memory().expect("an in-memory database always opens");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("the version is readable");
        assert_eq!(version, target_version());
    }

    #[test]
    fn foreign_keys_are_enforced_rather_than_decorative() {
        let connection = open_in_memory().expect("opens");
        let enabled: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("readable");
        assert_eq!(enabled, 1);
    }

    #[test]
    fn migrating_twice_is_harmless() {
        let connection = open_in_memory().expect("opens");
        migrate(&connection).expect("a second migration finds nothing to do");
        assert_eq!(
            connection
                .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .expect("readable"),
            target_version()
        );
    }

    #[test]
    fn a_database_from_a_newer_pushos_is_refused_rather_than_downgraded() {
        let connection = open_in_memory().expect("opens");
        connection
            .pragma_update(None, "user_version", target_version() + 10)
            .expect("writable");

        let error = migrate(&connection).expect_err("the schema is from the future");
        assert!(matches!(error, StorageError::FromTheFuture { .. }));
    }

    #[test]
    fn the_expected_tables_exist() {
        let connection = open_in_memory().expect("opens");
        for table in ["settings", "workspace_memory", "events"] {
            let count: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("the query runs");
            assert_eq!(count, 1, "`{table}` is missing");
        }
    }
}
