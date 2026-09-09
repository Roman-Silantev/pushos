//! The note index, kept in the database.
//!
//! Everything here is derived. The notes themselves are files the operator can
//! read without PushOS; this is only what makes them findable, and deleting it
//! costs a re-read and nothing else.
//!
//! Full text search is FTS5 over an external content table, so the words and
//! the note are the same copy and cannot drift apart.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pushos_domain::ids::{NoteId, SourceId, WorkspaceId};
use pushos_domain::memory::{Excerpt, Note, Search};
use pushos_domain::ports::{MemoryError, NoteIndex};

use crate::error::StorageError;
use crate::writer::Storage;

/// How tags are joined into one indexable field.
const TAG_SEPARATOR: &str = " ";

#[async_trait]
impl NoteIndex for Storage {
    async fn record(&self, note: &Note) -> Result<(), MemoryError> {
        self.save_note(NoteRow::from(note))
            .await
            .map_err(|error| MemoryError::unreadable("recording a note", error))
    }

    async fn search(&self, search: &Search) -> Result<Vec<Excerpt>, MemoryError> {
        self.search_notes(SearchRequest::from(search))
            .await
            .map_err(|error| MemoryError::unreadable("searching notes", error))
    }

    async fn erase(&self, note: &NoteId) -> Result<(), MemoryError> {
        self.forget_note(note.to_string())
            .await
            .map_err(|error| MemoryError::unreadable("forgetting a note", error))
    }

    async fn empty(&self, source: &SourceId) -> Result<(), MemoryError> {
        self.empty_source(source.to_string())
            .await
            .map_err(|error| MemoryError::unreadable("emptying a note source", error))
    }
}

/// One note, as a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRow {
    /// Where the note lives, which is also its identity.
    pub id: String,
    /// The source it came from.
    pub source_id: String,
    /// What it is called.
    pub title: String,
    /// Everything under the title.
    pub body: String,
    /// The file it was read from.
    pub path: Option<String>,
    /// The project it belongs to.
    pub(crate) workspace_id: Option<String>,
    /// How it was filed, separated by spaces so the index can tokenise them.
    pub tags: String,
    /// When it last changed.
    pub written_at: i64,
}

impl From<&Note> for NoteRow {
    fn from(note: &Note) -> Self {
        Self {
            id: note.id.to_string(),
            source_id: note.source.to_string(),
            title: note.title.clone(),
            body: note.body.clone(),
            path: note.path.as_ref().map(|path| path.display().to_string()),
            workspace_id: note.workspace.as_ref().map(ToString::to_string),
            tags: note.tags.join(TAG_SEPARATOR),
            written_at: seconds(note.written_at),
        }
    }
}

impl NoteRow {
    /// Rebuilds the note this row came from.
    pub fn to_note(&self) -> Note {
        Note {
            id: NoteId::new(&self.id),
            title: self.title.clone(),
            body: self.body.clone(),
            source: SourceId::new(&self.source_id),
            path: self.path.as_ref().map(std::path::PathBuf::from),
            workspace: self.workspace_id.as_deref().map(WorkspaceId::new),
            tags: self
                .tags
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect(),
            written_at: at(self.written_at),
        }
    }
}

/// A search, as the database takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchRequest {
    /// The words, already in FTS5's own syntax. Empty means the latest.
    pub(crate) words: String,
    /// Only notes filed under this project.
    pub(crate) workspace_id: Option<String>,
    /// Only notes filed under this tag.
    pub(crate) tag: Option<String>,
    /// How many to return.
    pub(crate) limit: i64,
}

impl From<&Search> for SearchRequest {
    fn from(search: &Search) -> Self {
        Self {
            words: match_expression(&search.text),
            workspace_id: search.workspace.as_ref().map(ToString::to_string),
            tag: search.tag.clone(),
            limit: i64::try_from(search.taking()).unwrap_or(i64::from(u8::MAX)),
        }
    }
}

/// Turns what the operator said into something FTS5 will accept.
///
/// Every word becomes a prefix term, joined by AND, and everything that is not
/// a word is dropped. A quote or a stray bracket in dictated speech is a typing
/// accident rather than query syntax, and letting it through would turn a
/// search into a parse error the operator cannot read.
fn match_expression(text: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() {
            terms.push(format!("\"{}\"*", word.to_lowercase()));
        }
    }
    terms.join(" AND ")
}

/// Seconds since the epoch, for a time that came from the clock.
fn seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0))
}

/// The time a stored second stands for.
fn at(seconds: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
}

/// Reads a row from a query that selected every note column.
pub(crate) fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: row.get(0)?,
        source_id: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        path: row.get(4)?,
        workspace_id: row.get(5)?,
        tags: row.get(6)?,
        written_at: row.get(7)?,
    })
}

/// Every note column, in the order [`read_row`] expects them.
pub(crate) const COLUMNS: &str = "id, source_id, title, body, path, workspace_id, tags, written_at";

/// Turns a row and the part of it that matched into something showable.
pub(crate) fn excerpt_of(row: &NoteRow, snippet: Option<String>) -> Excerpt {
    Excerpt {
        id: NoteId::new(&row.id),
        title: row.title.clone(),
        snippet: snippet.unwrap_or_else(|| opening(&row.body)),
        source: SourceId::new(&row.source_id),
    }
}

/// How long a snippet may be before it stops fitting the display.
const SNIPPET_LIMIT: usize = 80;

/// The opening of a note, for a result that matched on its title alone.
fn opening(body: &str) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match first.char_indices().nth(SNIPPET_LIMIT) {
        None => first.to_owned(),
        Some((cut, _)) => format!("{}…", first[..cut].trim_end()),
    }
}

/// Reports a query that could not run.
pub(crate) fn failed(what: &'static str, source: rusqlite::Error) -> StorageError {
    StorageError::query(what, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictated_punctuation_never_reaches_the_query() {
        // "search for: deploy, token" is what a transcriber produces, and the
        // colon and comma would be syntax rather than words.
        assert_eq!(
            match_expression("deploy, token"),
            "\"deploy\"* AND \"token\"*"
        );
        assert_eq!(
            match_expression("what's \"broken\"?"),
            "\"what\"* AND \"s\"* AND \"broken\"*"
        );
    }

    #[test]
    fn a_search_for_nothing_produces_no_expression() {
        assert!(match_expression("   ").is_empty());
        assert!(match_expression("!!!").is_empty());
    }

    #[test]
    fn words_are_matched_by_their_beginnings() {
        // An operator saying "deploy" should find "deployment".
        assert_eq!(match_expression("deploy"), "\"deploy\"*");
    }

    #[test]
    fn a_note_survives_being_written_down_and_read_back() {
        let note = Note {
            id: NoteId::new("notes/deploy.md"),
            title: "Deploy".to_owned(),
            body: "needs the new token".to_owned(),
            source: SourceId::new("notes"),
            path: Some(std::path::PathBuf::from("/tmp/notes/deploy.md")),
            workspace: Some(WorkspaceId::new("sydclaw")),
            tags: vec!["decision".to_owned(), "infra".to_owned()],
            written_at: at(1_700_000_000),
        };
        assert_eq!(NoteRow::from(&note).to_note(), note);
    }

    #[test]
    fn a_long_opening_is_cut_rather_than_overflowing_the_display() {
        let long = "word ".repeat(60);
        let excerpt = opening(&long);
        assert!(excerpt.chars().count() <= SNIPPET_LIMIT + 1);
        assert!(excerpt.ends_with('…'));
    }
}
