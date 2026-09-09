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
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
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
    },
    Migration {
        version: 2,
        name: "workflow runs",
        sql: "
        -- Where each run of a workflow has got to. Read at startup: what is
        -- here and unfinished is what PushOS was in the middle of.
        CREATE TABLE workflow_runs (
            id              TEXT PRIMARY KEY NOT NULL,
            workflow_id     TEXT NOT NULL,
            workspace_id    TEXT,
            node_id         TEXT NOT NULL,
            state           TEXT NOT NULL,
            waiting         TEXT,
            outcome         TEXT,
            steps           INTEGER NOT NULL,
            last_succeeded  INTEGER NOT NULL,
            note            TEXT,
            started_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        ) STRICT;

        CREATE INDEX workflow_runs_unfinished ON workflow_runs (state);

        -- Every step every run took, written before the step after it runs.
        CREATE TABLE workflow_transitions (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id       TEXT NOT NULL,
            from_node    TEXT,
            to_node      TEXT NOT NULL,
            note         TEXT,
            recorded_at  INTEGER NOT NULL
        ) STRICT;

        CREATE INDEX workflow_transitions_by_run ON workflow_transitions (run_id, id);
    ",
    },
    Migration {
        version: 3,
        name: "note index",
        sql: "
        -- Where notes are, so they can be found without reading every file.
        -- Everything here is derived from files on disk: deleting this table
        -- costs a re-read and nothing else, which is what keeps notes the
        -- operator\'s rather than PushOS\'s.
        CREATE TABLE notes (
            id           TEXT PRIMARY KEY NOT NULL,
            source_id    TEXT NOT NULL,
            title        TEXT NOT NULL,
            body         TEXT NOT NULL,
            path         TEXT,
            workspace_id TEXT,
            tags         TEXT NOT NULL,
            written_at   INTEGER NOT NULL
        ) STRICT;

        CREATE INDEX notes_by_source ON notes (source_id);
        CREATE INDEX notes_by_time ON notes (written_at DESC);

        -- The words, for searching. External content: the row above is the
        -- only copy of the text, and this refers to it rather than repeating
        -- it, so the two cannot drift apart.
        CREATE VIRTUAL TABLE note_words USING fts5(
            title,
            body,
            tags,
            content = \'notes\',
            content_rowid = \'rowid\',
            tokenize = \'unicode61 remove_diacritics 2\'
        );

        CREATE TRIGGER notes_indexed AFTER INSERT ON notes BEGIN
            INSERT INTO note_words (rowid, title, body, tags)
            VALUES (new.rowid, new.title, new.body, new.tags);
        END;

        CREATE TRIGGER notes_unindexed AFTER DELETE ON notes BEGIN
            INSERT INTO note_words (note_words, rowid, title, body, tags)
            VALUES (\'delete\', old.rowid, old.title, old.body, old.tags);
        END;

        CREATE TRIGGER notes_reindexed AFTER UPDATE ON notes BEGIN
            INSERT INTO note_words (note_words, rowid, title, body, tags)
            VALUES (\'delete\', old.rowid, old.title, old.body, old.tags);
            INSERT INTO note_words (rowid, title, body, tags)
            VALUES (new.rowid, new.title, new.body, new.tags);
        END;
    ",
    },
];

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
