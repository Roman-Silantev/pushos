//! What PushOS knows, as opposed to what it is doing.
//!
//! Notes: decisions, context, things worth not losing. Deliberately small.
//! PushOS is a control surface, not a knowledge base, and memory exists here so
//! that a pad can capture a thought in one press and an agent can be given what
//! it needs to know before it starts.
//!
//! Not to be confused with [`crate::ports::WorkspaceMemoryStore`], which is
//! operational state: which page a project was on. That is PushOS's own
//! bookkeeping. This is the operator's, and it lives in files they can read
//! without PushOS.

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::ids::{NoteId, SourceId, WorkspaceId};

/// One note, in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Where it lives, which is also what it is called.
    pub id: NoteId,
    /// The first line, as a person would name it.
    pub title: String,
    /// Everything under the title.
    pub body: String,
    /// Which source it came from.
    pub source: SourceId,
    /// The file it was read from, when it came from one.
    pub path: Option<PathBuf>,
    /// The project it belongs to, when it belongs to one.
    pub workspace: Option<WorkspaceId>,
    /// How it was filed.
    pub tags: Vec<String>,
    /// When it last changed.
    pub written_at: SystemTime,
}

impl Note {
    /// The first line of the body, for a display with one line to give it.
    pub fn summary(&self) -> &str {
        self.body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
    }
}

/// A note about to be written.
///
/// Separate from [`Note`] because what the operator supplies and what a store
/// hands back are not the same thing: identity, source and time are the store's
/// to decide, and a caller inventing them would be inventing facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// What to call it. Absent means take the first line of the body.
    pub title: Option<String>,
    /// What it says.
    pub body: String,
    /// The project it belongs to.
    pub workspace: Option<WorkspaceId>,
    /// How to file it.
    pub tags: Vec<String>,
}

impl Draft {
    /// Builds a draft with nothing but words.
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            ..Self::default()
        }
    }

    /// Files it under a project.
    #[must_use]
    pub fn about(mut self, workspace: Option<WorkspaceId>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Names it.
    #[must_use]
    pub fn called(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The title it should end up with.
    ///
    /// The first line when none was given, shortened to something that fits a
    /// file name and a display. A note called by its whole first paragraph is
    /// a note nobody can find again.
    pub fn heading(&self) -> String {
        let given = self.title.as_deref().map_or("", str::trim);
        let taken = if given.is_empty() {
            self.body.lines().map(str::trim).find(|l| !l.is_empty())
        } else {
            Some(given)
        };

        let heading = taken.unwrap_or("Note");
        match heading.char_indices().nth(TITLE_LIMIT) {
            None => heading.to_owned(),
            Some((cut, _)) => format!("{}…", heading[..cut].trim_end()),
        }
    }

    /// Whether there is anything worth writing down.
    pub fn is_empty(&self) -> bool {
        self.body.trim().is_empty()
    }
}

/// How many characters of a first line become a title.
const TITLE_LIMIT: usize = 60;

/// One search result: enough to recognise a note, not the note itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    /// Which note it is.
    pub id: NoteId,
    /// What it is called.
    pub title: String,
    /// The part that matched, or the opening when nothing did.
    pub snippet: String,
    /// Which source it came from.
    pub source: SourceId,
}

impl fmt::Display for Excerpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.snippet.is_empty() {
            f.write_str(&self.title)
        } else {
            write!(f, "{} — {}", self.title, self.snippet)
        }
    }
}

/// What to look for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// The words. Empty means everything, most recent first.
    pub text: String,
    /// Only notes filed under this project.
    pub workspace: Option<WorkspaceId>,
    /// Only notes filed under this tag.
    pub tag: Option<String>,
    /// How many to return.
    pub limit: usize,
}

/// How many results a search returns when nothing says otherwise.
pub const DEFAULT_LIMIT: usize = 5;

impl Search {
    /// Builds a search for some words.
    pub fn for_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: DEFAULT_LIMIT,
            ..Self::default()
        }
    }

    /// Builds a search for the most recent notes.
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    /// Narrows it to one project.
    #[must_use]
    pub fn within(mut self, workspace: Option<WorkspaceId>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Whether this is a search at all, or just the latest.
    pub fn is_open(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// How many to return, never zero and never unbounded.
    ///
    /// A display with 160 pixels of height cannot show a hundred results, and
    /// asking for none is always a mistake rather than a request.
    pub fn taking(&self) -> usize {
        self.limit.clamp(1, MOST_RESULTS)
    }
}

/// The most results any one search will return.
pub const MOST_RESULTS: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_with_no_title_is_named_by_its_first_line() {
        let draft = Draft::new("  \n\nThe deploy needs the new token\nand a restart");
        assert_eq!(draft.heading(), "The deploy needs the new token");
    }

    #[test]
    fn a_given_title_wins_over_the_first_line() {
        let draft = Draft::new("something else").called("Deploy notes");
        assert_eq!(draft.heading(), "Deploy notes");
    }

    #[test]
    fn a_title_taken_from_a_paragraph_is_cut_to_something_findable() {
        let long = "a".repeat(200);
        let heading = Draft::new(&long).heading();
        assert!(heading.chars().count() <= TITLE_LIMIT + 1, "{heading}");
        assert!(heading.ends_with('…'));
    }

    #[test]
    fn a_note_with_nothing_in_it_is_empty() {
        // Pressing the capture pad by accident must not leave a file behind.
        assert!(Draft::new("   \n\n").is_empty());
        assert!(!Draft::new("something").is_empty());
    }

    #[test]
    fn an_empty_title_falls_back_rather_than_producing_a_nameless_note() {
        let draft = Draft::new("body text").called("   ");
        assert_eq!(draft.heading(), "body text");
    }

    #[test]
    fn a_search_never_asks_for_none_and_never_asks_for_everything() {
        assert_eq!(Search::recent(0).taking(), 1);
        assert_eq!(Search::recent(10_000).taking(), MOST_RESULTS);
        assert_eq!(Search::recent(3).taking(), 3);
    }

    #[test]
    fn a_search_with_no_words_is_a_request_for_the_latest() {
        assert!(Search::recent(5).is_open());
        assert!(!Search::for_text("deploy").is_open());
        assert!(Search::for_text("   ").is_open());
    }

    #[test]
    fn an_excerpt_reads_as_one_line() {
        let excerpt = Excerpt {
            id: NoteId::new("notes/deploy.md"),
            title: "Deploy".to_owned(),
            snippet: "needs the new token".to_owned(),
            source: SourceId::new("notes"),
        };
        assert_eq!(excerpt.to_string(), "Deploy — needs the new token");
    }
}
