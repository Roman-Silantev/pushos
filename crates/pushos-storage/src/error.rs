//! Storage failures.

use std::path::PathBuf;

/// Why a storage operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// The directory the database lives in could not be created.
    #[error("could not create `{path}`")]
    Unwritable {
        /// The directory.
        path: PathBuf,
        /// What the file system reported.
        #[source]
        source: std::io::Error,
    },

    /// The database could not be opened.
    #[error("could not open the PushOS database")]
    Open(#[source] rusqlite::Error),

    /// The connection could not be configured.
    #[error("could not configure the PushOS database")]
    Configure(#[source] rusqlite::Error),

    /// The schema could not be brought up to date.
    #[error("could not migrate the PushOS database")]
    Migrate(#[source] rusqlite::Error),

    /// The database was written by a newer PushOS.
    #[error(
        "the database is at schema version {found} but this build expects {expected}; \
         it was written by a newer PushOS"
    )]
    FromTheFuture {
        /// The version on disk.
        found: i64,
        /// The version this build understands.
        expected: i64,
    },

    /// A statement failed.
    #[error("{context}")]
    Query {
        /// What PushOS was doing.
        context: String,
        /// What the database reported.
        #[source]
        source: rusqlite::Error,
    },

    /// The writer has stopped and will accept no further work.
    #[error("the storage writer has stopped")]
    Stopped,
}

impl StorageError {
    pub(crate) fn open(source: rusqlite::Error) -> Self {
        Self::Open(source)
    }

    pub(crate) fn configure(source: rusqlite::Error) -> Self {
        Self::Configure(source)
    }

    pub(crate) fn migrate(source: rusqlite::Error) -> Self {
        Self::Migrate(source)
    }

    pub(crate) fn query(context: impl Into<String>, source: rusqlite::Error) -> Self {
        Self::Query {
            context: context.into(),
            source,
        }
    }

    /// Whether the failure means the database is unusable, as opposed to one
    /// statement having failed.
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Unwritable { .. }
                | Self::Open(_)
                | Self::Configure(_)
                | Self::Migrate(_)
                | Self::FromTheFuture { .. }
                | Self::Stopped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_statement_is_not_fatal_but_a_failed_migration_is() {
        let statement =
            StorageError::query("recording an event", rusqlite::Error::QueryReturnedNoRows);
        assert!(!statement.is_fatal());

        let migration = StorageError::migrate(rusqlite::Error::QueryReturnedNoRows);
        assert!(migration.is_fatal());
    }

    #[test]
    fn a_future_schema_explains_itself() {
        let error = StorageError::FromTheFuture {
            found: 9,
            expected: 1,
        };
        let message = error.to_string();
        assert!(message.contains('9'));
        assert!(message.contains("newer PushOS"));
    }
}
