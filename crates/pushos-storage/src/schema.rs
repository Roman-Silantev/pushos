//! The database schema, as an ordered list of migrations.
//!
//! Migrations are append-only and applied inside one transaction each. A
//! partially applied schema is never left behind, and a database from an older
//! PushOS is brought forward on startup without operator involvement.

/// One schema step.
pub(crate) struct Migration {
    /// Monotonic version this step produces.
    pub(crate) version: i64,
    /// What the step is for, used in logs.
    pub(crate) name: &'static str,
    /// The statements to run.
    pub(crate) sql: &'static str,
}

/// Every migration, in the order they must be applied.
pub(crate) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sql: "
        CREATE TABLE settings (
            key         TEXT PRIMARY KEY NOT NULL,
            value       TEXT NOT NULL,
            updated_at  INTEGER NOT NULL
        ) STRICT;

        -- What a workspace restores when the operator returns to it.
        CREATE TABLE workspace_memory (
            workspace_id      TEXT PRIMARY KEY NOT NULL,
            last_page         TEXT,
            selected_session  TEXT,
            updated_at        INTEGER NOT NULL
        ) STRICT;

        -- The audit trail. Correlation ties every row produced by one gesture
        -- together, and causation records what led to what.
        CREATE TABLE events (
            id              TEXT PRIMARY KEY NOT NULL,
            correlation_id  TEXT NOT NULL,
            causation_id    TEXT,
            recorded_at     INTEGER NOT NULL,
            source          TEXT NOT NULL,
            kind            TEXT NOT NULL,
            detail          TEXT
        ) STRICT;

        CREATE INDEX events_by_correlation ON events (correlation_id);
        CREATE INDEX events_by_time ON events (recorded_at DESC);
    ",
}];

/// The schema version this build expects.
pub(crate) fn target_version() -> i64 {
    MIGRATIONS.last().map_or(0, |migration| migration.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_from_one_without_gaps() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            let expected = i64::try_from(index + 1).expect("the migration list is small");
            assert_eq!(
                migration.version, expected,
                "`{}` is out of order; migrations are append-only",
                migration.name
            );
        }
    }

    #[test]
    fn the_target_version_is_the_last_migration() {
        assert_eq!(
            target_version(),
            i64::try_from(MIGRATIONS.len()).expect("the list is small")
        );
    }
}
