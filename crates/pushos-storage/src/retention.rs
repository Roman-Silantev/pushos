//! How much the database keeps, so it stops growing.
//!
//! Every press is recorded, and a PushOS used all day records thousands of
//! events a day. Kept for ever, that is a file that grows by a megabyte a day
//! for as long as PushOS runs. So the record is a window rather than a
//! history: the recent past, which is what anyone looks at, and nothing older.
//! Finished workflow runs are kept the same way; a run still going is never
//! touched.
//!
//! SQLite reuses the room a deleted row leaves, so once the window is full the
//! file stays the size it reached. A file that grew before there was a window
//! is compacted once, when it is mostly empty room.

use rusqlite::Connection;
use tracing::{debug, info};

use crate::error::StorageError;

/// The most events kept.
///
/// Around a week of busy use, and a dozen megabytes on disk.
pub(crate) const EVENTS_KEPT: i64 = 50_000;

/// The oldest an event may be, in seconds.
pub(crate) const EVENTS_FOR: i64 = 30 * 24 * 60 * 60;

/// The most finished workflow runs kept, with their steps.
pub(crate) const FINISHED_RUNS_KEPT: i64 = 500;

/// The oldest a finished workflow run may be, in seconds.
pub(crate) const FINISHED_RUNS_FOR: i64 = 90 * 24 * 60 * 60;

/// How many events are recorded between trims.
///
/// A trim is a few deletes over no more rows than the window holds. Often
/// enough that the window never overruns by more than this, and seldom enough
/// to cost nothing noticeable.
pub(crate) const TRIM_EVERY: u64 = 1_000;

/// What a trim removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trimmed {
    pub(crate) events: usize,
    pub(crate) runs: usize,
}

/// Removes what is outside the window, as of `now` in seconds since the epoch.
pub(crate) fn trim(connection: &Connection, now: i64) -> Result<Trimmed, StorageError> {
    let mut trimmed = Trimmed::default();

    trimmed.events += connection
        .execute(
            "DELETE FROM events WHERE recorded_at < ?1",
            [now - EVENTS_FOR],
        )
        .map_err(|source| StorageError::query("trimming old events", source))?;
    // By insertion order, which is the order events happened in.
    trimmed.events += connection
        .execute(
            "DELETE FROM events WHERE rowid <= (SELECT max(rowid) FROM events) - ?1",
            [EVENTS_KEPT],
        )
        .map_err(|source| StorageError::query("trimming surplus events", source))?;

    let finished = "SELECT id FROM workflow_runs WHERE state = 'finished' AND (updated_at < ?1 \
                    OR id NOT IN (SELECT id FROM workflow_runs WHERE state = 'finished' \
                    ORDER BY updated_at DESC LIMIT ?2))";
    connection
        .execute(
            &format!("DELETE FROM workflow_transitions WHERE run_id IN ({finished})"),
            rusqlite::params![now - FINISHED_RUNS_FOR, FINISHED_RUNS_KEPT],
        )
        .map_err(|source| StorageError::query("trimming old workflow steps", source))?;
    trimmed.runs += connection
        .execute(
            &format!("DELETE FROM workflow_runs WHERE id IN ({finished})"),
            rusqlite::params![now - FINISHED_RUNS_FOR, FINISHED_RUNS_KEPT],
        )
        .map_err(|source| StorageError::query("trimming old workflow runs", source))?;

    if trimmed != Trimmed::default() {
        debug!(
            events = trimmed.events,
            runs = trimmed.runs,
            "trimmed the record"
        );
    }
    Ok(trimmed)
}

/// Compacts a file that is mostly empty room, once it is worth doing.
///
/// Only for a file that grew before its record had a window: in steady use the
/// room a trim frees is taken again by the next events, and compacting would
/// only rewrite a file that is about to fill the same space.
pub(crate) fn compact_if_mostly_empty(connection: &Connection) -> Result<(), StorageError> {
    let pages: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(|source| StorageError::query("measuring the database", source))?;
    let free: i64 = connection
        .pragma_query_value(None, "freelist_count", |row| row.get(0))
        .map_err(|source| StorageError::query("measuring the database", source))?;

    // Half empty, and more than a thousand pages of nothing: about four
    // megabytes at SQLite's default page size.
    if free * 2 > pages && free > 1_000 {
        connection
            .execute_batch("VACUUM")
            .map_err(|source| StorageError::query("compacting the database", source))?;
        info!(freed_pages = free, "compacted the database");
    }
    Ok(())
}

/// Seconds since the epoch, now.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        crate::connection::open_in_memory().expect("opens")
    }

    fn record(connection: &Connection, id: usize, at: i64) {
        connection
            .execute(
                "INSERT INTO events (id, correlation_id, recorded_at, source, kind) \
                 VALUES (?1, 'c', ?2, 'push', 'gesture')",
                rusqlite::params![format!("e{id}"), at],
            )
            .expect("inserts");
    }

    fn events(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("counts")
    }

    #[test]
    fn events_older_than_the_window_are_removed() {
        let connection = database();
        let now = 2_000_000_000;
        record(&connection, 1, now - EVENTS_FOR - 1);
        record(&connection, 2, now - 60);

        let trimmed = trim(&connection, now).expect("trims");
        assert_eq!(trimmed.events, 1);
        assert_eq!(events(&connection), 1);
    }

    #[test]
    fn no_more_than_the_window_of_events_is_kept_and_the_newest_stay() {
        let connection = database();
        let now = 2_000_000_000;
        let total = usize::try_from(EVENTS_KEPT).expect("fits") + 250;
        connection.execute_batch("BEGIN").expect("begins");
        for id in 0..total {
            record(&connection, id, now);
        }
        connection.execute_batch("COMMIT").expect("commits");

        trim(&connection, now).expect("trims");
        assert_eq!(events(&connection), EVENTS_KEPT);
        let oldest_kept: String = connection
            .query_row("SELECT id FROM events ORDER BY rowid LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("one");
        assert_eq!(oldest_kept, "e250", "the oldest went, the newest stayed");
    }

    fn run(connection: &Connection, id: &str, state: &str, updated_at: i64) {
        connection
            .execute(
                "INSERT INTO workflow_runs (id, workflow_id, node_id, state, steps, last_succeeded, \
                 started_at, updated_at) VALUES (?1, 'w', 'n', ?2, 1, 1, ?3, ?3)",
                rusqlite::params![id, state, updated_at],
            )
            .expect("inserts a run");
        connection
            .execute(
                "INSERT INTO workflow_transitions (run_id, to_node, recorded_at) VALUES (?1, 'n', ?2)",
                rusqlite::params![id, updated_at],
            )
            .expect("inserts a step");
    }

    #[test]
    fn an_old_finished_run_goes_with_its_steps_and_a_run_still_going_never_does() {
        let connection = database();
        let now = 2_000_000_000;
        let long_ago = now - FINISHED_RUNS_FOR - 1;
        run(&connection, "old-finished", "finished", long_ago);
        run(&connection, "old-waiting", "waiting", long_ago);
        run(&connection, "recent", "finished", now);

        let trimmed = trim(&connection, now).expect("trims");
        assert_eq!(trimmed.runs, 1);

        let left: Vec<String> = connection
            .prepare("SELECT id FROM workflow_runs ORDER BY id")
            .expect("prepares")
            .query_map([], |row| row.get(0))
            .expect("queries")
            .map(|id| id.expect("an id"))
            .collect();
        assert_eq!(left, ["old-waiting", "recent"]);
        let orphaned: i64 = connection
            .query_row(
                "SELECT count(*) FROM workflow_transitions WHERE run_id = 'old-finished'",
                [],
                |row| row.get(0),
            )
            .expect("counts");
        assert_eq!(orphaned, 0, "its steps went with it");
    }
}
