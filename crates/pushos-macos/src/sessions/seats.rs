//! Which session each named seat is holding.
//!
//! A pad that starts a session needs to find the same one again tomorrow. A
//! session in tmux is found by the name tmux keeps it under, but a session a
//! coding agent keeps has only the identifier the agent chose, and nothing
//! PushOS can ask it to be called instead. So PushOS writes down which
//! identifier each seat took, and that is all this is.
//!
//! One line per seat, which is at most one per pad, and entries for sessions
//! the agent no longer knows about are dropped whenever the book is read. It
//! cannot grow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;
use tracing::{debug, warn};

/// What each seat is holding, kept between runs.
#[derive(Debug)]
pub(super) struct Seats {
    /// Where the book is written, when it is written anywhere.
    path: Option<PathBuf>,
    held: Mutex<Option<BTreeMap<String, String>>>,
}

impl Seats {
    /// A book kept in a file, read the first time it is needed.
    pub(super) fn kept_in(path: Option<PathBuf>) -> Self {
        Self {
            path,
            held: Mutex::new(None),
        }
    }

    /// What a seat is holding, if it holds anything.
    pub(super) async fn holding(&self, seat: &str) -> Option<String> {
        self.read().await.get(seat).cloned()
    }

    /// The seat a session is held by, if any seat holds it.
    pub(super) async fn seat_of(&self, session: &str) -> Option<String> {
        self.read()
            .await
            .iter()
            .find(|(_, held)| held.as_str() == session)
            .map(|(seat, _)| seat.clone())
    }

    /// Records that a seat took a session, replacing whatever it had.
    pub(super) async fn took(&self, seat: &str, session: &str) {
        let mut held = self.held.lock().await;
        let book = held.get_or_insert_with(|| self.load());
        book.insert(seat.to_owned(), session.to_owned());
        debug!(seat, session, "a seat took a session");
        self.save(book);
    }

    /// Drops what a seat holds, and anything held that no longer exists.
    ///
    /// Given every session the agent still knows about, so a seat whose
    /// session was removed elsewhere stops pointing at nothing.
    pub(super) async fn keep_only(&self, existing: &[String]) {
        let mut held = self.held.lock().await;
        let book = held.get_or_insert_with(|| self.load());
        let before = book.len();
        book.retain(|_, session| existing.iter().any(|kept| kept == session));
        if book.len() != before {
            self.save(book);
        }
    }

    /// The book, read from the file the first time.
    async fn read(&self) -> BTreeMap<String, String> {
        let mut held = self.held.lock().await;
        held.get_or_insert_with(|| self.load()).clone()
    }

    fn load(&self) -> BTreeMap<String, String> {
        let Some(path) = &self.path else {
            return BTreeMap::new();
        };
        let Ok(written) = std::fs::read_to_string(path) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&written).unwrap_or_else(|error| {
            warn!(%error, path = %path.display(), "the seat book could not be read; starting a new one");
            BTreeMap::new()
        })
    }

    fn save(&self, book: &BTreeMap<String, String>) {
        let Some(path) = &self.path else { return };
        let Ok(written) = serde_json::to_string_pretty(book) else {
            return;
        };
        if let Some(directory) = path.parent()
            && std::fs::create_dir_all(directory).is_err()
        {
            return;
        }
        if let Err(error) = write_atomically(path, &written) {
            warn!(%error, path = %path.display(), "the seat book could not be written");
        }
    }
}

/// Writes the book in one step, so a crash cannot leave half of it.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let beside = path.with_extension("writing");
    std::fs::write(&beside, text)?;
    std::fs::rename(&beside, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A book in a file of its own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "pushos-seats-{label}-{}",
                pushos_domain::ids::ExecutionId::generate()
            ));
            std::fs::create_dir_all(&directory).expect("writable");
            Self(directory)
        }

        fn book(&self) -> PathBuf {
            self.0.join("seats.json")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[tokio::test]
    async fn a_seat_still_holds_its_session_after_pushos_is_started_again() {
        let scratch = Scratch::new("kept");
        let seats = Seats::kept_in(Some(scratch.book()));
        seats.took("client-1", "55c88714").await;

        let started_again = Seats::kept_in(Some(scratch.book()));
        assert_eq!(
            started_again.holding("client-1").await.as_deref(),
            Some("55c88714")
        );
        assert_eq!(
            started_again.seat_of("55c88714").await.as_deref(),
            Some("client-1")
        );
    }

    #[tokio::test]
    async fn a_seat_asked_for_again_takes_the_newer_session() {
        let scratch = Scratch::new("replaced");
        let seats = Seats::kept_in(Some(scratch.book()));
        seats.took("client-1", "55c88714").await;
        seats.took("client-1", "9db46d48").await;

        assert_eq!(
            seats.holding("client-1").await.as_deref(),
            Some("9db46d48"),
            "one seat, one session"
        );
        assert_eq!(seats.seat_of("55c88714").await, None);
    }

    #[tokio::test]
    async fn a_session_removed_elsewhere_stops_being_held() {
        let scratch = Scratch::new("gone");
        let seats = Seats::kept_in(Some(scratch.book()));
        seats.took("client-1", "55c88714").await;
        seats.took("client-2", "9db46d48").await;

        seats.keep_only(&["9db46d48".to_owned()]).await;

        assert_eq!(seats.holding("client-1").await, None);
        assert_eq!(
            seats.holding("client-2").await.as_deref(),
            Some("9db46d48"),
            "the one that still exists is kept"
        );
    }

    #[tokio::test]
    async fn a_book_nobody_gave_a_file_still_works_for_as_long_as_pushos_runs() {
        let seats = Seats::kept_in(None);
        seats.took("client-1", "55c88714").await;
        assert_eq!(seats.holding("client-1").await.as_deref(), Some("55c88714"));
    }

    #[tokio::test]
    async fn a_book_that_is_not_readable_starts_again_rather_than_failing() {
        let scratch = Scratch::new("broken");
        std::fs::write(scratch.book(), "{ not json").expect("writable");

        let seats = Seats::kept_in(Some(scratch.book()));
        assert_eq!(seats.holding("client-1").await, None);

        seats.took("client-1", "55c88714").await;
        assert_eq!(
            Seats::kept_in(Some(scratch.book()))
                .holding("client-1")
                .await
                .as_deref(),
            Some("55c88714"),
            "and what is written after is readable"
        );
    }
}
