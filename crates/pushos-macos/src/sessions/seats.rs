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

use pushos_domain::ports::Keeper;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// What one seat holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Held {
    /// A session, and which agent keeps it.
    ///
    /// Knowing the keeper is what lets PushOS leave an agent alone entirely
    /// when no pad uses it: a Mac with only Claude Code seats never starts
    /// Codex's server, and the other way round.
    By {
        /// Who keeps it: `claude` or `codex`.
        keeper: String,
        /// What they call it.
        session: String,
    },
    /// A session written down before the book said who kept it.
    Session(String),
}

impl Held {
    /// What the keeper calls the session.
    pub(super) fn session(&self) -> &str {
        match self {
            Self::By { session, .. } | Self::Session(session) => session,
        }
    }

    /// Who keeps it, when the book says.
    fn keeper(&self) -> Option<Keeper> {
        match self {
            Self::By { keeper, .. } => keeper.parse().ok(),
            Self::Session(_) => None,
        }
    }
}

/// What each seat is holding, kept between runs.
#[derive(Debug)]
pub(super) struct Seats {
    /// Where the book is written, when it is written anywhere.
    path: Option<PathBuf>,
    held: Mutex<Option<BTreeMap<String, Held>>>,
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
    ///
    /// A seat is found however it was capitalised, because that is how a
    /// binding's target finds a session: `Invoices` and `invoices` are one
    /// pad's seat, not two.
    pub(super) async fn holding(&self, seat: &str) -> Option<String> {
        self.read()
            .await
            .get(&called(seat))
            .map(|held| held.session().to_owned())
    }

    /// The seat a session is held by, if any seat holds it.
    pub(super) async fn seat_of(&self, session: &str) -> Option<String> {
        self.read()
            .await
            .iter()
            .find(|(_, held)| held.session() == session)
            .map(|(seat, _)| seat.clone())
    }

    /// Whether any seat is held by this keeper, or by one the book does not
    /// name.
    ///
    /// What an agent is asked is decided by this: one no pad uses is not
    /// started, not spoken to, and costs nothing.
    pub(super) async fn any_held_by(&self, keeper: Keeper) -> bool {
        self.read()
            .await
            .values()
            .any(|held| held.keeper().is_none_or(|whose| whose == keeper))
    }

    /// Every session this keeper holds a seat for.
    pub(super) async fn held_by(&self, keeper: Keeper) -> Vec<String> {
        self.read()
            .await
            .values()
            .filter(|held| held.keeper().is_some_and(|whose| whose == keeper))
            .map(|held| held.session().to_owned())
            .collect()
    }

    /// Records that a seat took a session, replacing whatever it had.
    pub(super) async fn took(&self, seat: &str, keeper: Keeper, session: &str) {
        let mut held = self.held.lock().await;
        let book = held.get_or_insert_with(|| self.load());
        book.insert(
            called(seat),
            Held::By {
                keeper: keeper.as_str().to_owned(),
                session: session.to_owned(),
            },
        );
        debug!(
            seat,
            session,
            keeper = keeper.as_str(),
            "a seat took a session"
        );
        self.save(book);
    }

    /// Drops the seats of one keeper whose sessions it no longer knows about.
    ///
    /// Only that keeper's seats, and only from a listing it actually gave:
    /// what another agent holds is none of its business, and an agent that
    /// said nothing has not said that anything ended.
    pub(super) async fn keep_only(&self, keeper: Keeper, existing: &[String], both: bool) {
        let mut held = self.held.lock().await;
        let book = held.get_or_insert_with(|| self.load());
        let before = book.len();
        book.retain(|_, held| {
            let whose = held.keeper();
            // A seat written before the book named keepers belongs to one of
            // them, and which is not known; it goes only when both have
            // answered and neither claims it.
            let mine = whose.is_none_or(|whose| whose == keeper);
            let decidable = whose.is_some() || both;
            !mine || !decidable || existing.iter().any(|kept| kept == held.session())
        });
        if book.len() != before {
            self.save(book);
        }
    }

    /// The book, read from the file the first time.
    async fn read(&self) -> BTreeMap<String, Held> {
        let mut held = self.held.lock().await;
        held.get_or_insert_with(|| self.load()).clone()
    }

    fn load(&self) -> BTreeMap<String, Held> {
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

    fn save(&self, book: &BTreeMap<String, Held>) {
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

/// How a seat's name is written down, so it is found however it was typed.
fn called(seat: &str) -> String {
    seat.trim().to_lowercase()
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
        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;

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
        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;
        seats.took("client-1", Keeper::ClaudeCode, "9db46d48").await;

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
        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;
        seats.took("client-2", Keeper::ClaudeCode, "9db46d48").await;

        seats
            .keep_only(Keeper::ClaudeCode, &["9db46d48".to_owned()], true)
            .await;

        assert_eq!(seats.holding("client-1").await, None);
        assert_eq!(
            seats.holding("client-2").await.as_deref(),
            Some("9db46d48"),
            "the one that still exists is kept"
        );
    }

    #[tokio::test]
    async fn an_agent_no_pad_uses_is_not_asked_about() {
        let seats = Seats::kept_in(None);
        assert!(
            !seats.any_held_by(Keeper::CodexThreads).await,
            "an empty book wants nothing of anybody"
        );

        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;
        assert!(seats.any_held_by(Keeper::ClaudeCode).await);
        assert!(
            !seats.any_held_by(Keeper::CodexThreads).await,
            "Codex is not started for a seat Claude Code keeps"
        );

        seats.took("client-2", Keeper::CodexThreads, "abc").await;
        assert!(seats.any_held_by(Keeper::CodexThreads).await);
    }

    #[tokio::test]
    async fn one_agent_answering_never_drops_another_agent_s_seats() {
        let scratch = Scratch::new("apart");
        let seats = Seats::kept_in(Some(scratch.book()));
        seats.took("builder", Keeper::ClaudeCode, "55c88714").await;
        seats.took("scraper", Keeper::CodexThreads, "abc").await;

        // Codex answers, and says it has nothing.
        seats.keep_only(Keeper::CodexThreads, &[], false).await;

        assert_eq!(
            seats.holding("builder").await.as_deref(),
            Some("55c88714"),
            "what Claude Code keeps is none of Codex's business"
        );
        assert_eq!(seats.holding("scraper").await, None, "and its own went");
    }

    #[tokio::test]
    async fn a_seat_written_before_the_book_named_keepers_waits_for_both() {
        let scratch = Scratch::new("older");
        std::fs::write(scratch.book(), r#"{"builder": "55c88714"}"#).expect("writable");
        let seats = Seats::kept_in(Some(scratch.book()));

        // One agent answers and does not know it. The other has said nothing,
        // so it is still possible that the seat is theirs.
        seats.keep_only(Keeper::ClaudeCode, &[], false).await;
        assert_eq!(
            seats.holding("builder").await.as_deref(),
            Some("55c88714"),
            "nobody has ruled out the other agent"
        );

        // Both have answered now, and neither claims it.
        seats.keep_only(Keeper::ClaudeCode, &[], true).await;
        assert_eq!(seats.holding("builder").await, None);
    }

    #[tokio::test]
    async fn a_book_nobody_gave_a_file_still_works_for_as_long_as_pushos_runs() {
        let seats = Seats::kept_in(None);
        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;
        assert_eq!(seats.holding("client-1").await.as_deref(), Some("55c88714"));
    }

    #[tokio::test]
    async fn a_book_that_is_not_readable_starts_again_rather_than_failing() {
        let scratch = Scratch::new("broken");
        std::fs::write(scratch.book(), "{ not json").expect("writable");

        let seats = Seats::kept_in(Some(scratch.book()));
        assert_eq!(seats.holding("client-1").await, None);

        seats.took("client-1", Keeper::ClaudeCode, "55c88714").await;
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
