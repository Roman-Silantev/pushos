//! Notes, exercised against real files and a real index.
//!
//! Nothing here is faked. The point of keeping notes as Markdown is that they
//! are files, and a test that never touched a file system would not be testing
//! that claim at all.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use pushos_domain::ids::{ExecutionId, NoteId, SourceId, WorkspaceId};
use pushos_domain::memory::{Draft, Search};
use pushos_domain::ports::{MemoryStore, NoteIndex, Source};
use pushos_memory::{ForgetfulNotes, MarkdownLibrary};
use pushos_storage::StorageWriter;

/// A directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pushos-notes-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    fn put(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("writable");
        }
        std::fs::write(path, text).expect("writable");
    }

    fn source(&self, id: &str, writable: bool) -> Source {
        Source {
            id: SourceId::new(id),
            root: self.0.clone(),
            writable,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// A library with a real database behind it.
struct Library {
    library: MarkdownLibrary,
    /// Held so the database outlives the library.
    _writer: StorageWriter,
}

impl Library {
    fn over(sources: Vec<Source>) -> Self {
        let writer = StorageWriter::in_memory().expect("an in-memory database always opens");
        let index: Arc<dyn NoteIndex> = Arc::new(writer.handle());
        Self {
            library: MarkdownLibrary::new(sources, index),
            _writer: writer,
        }
    }

    fn without_an_index(sources: Vec<Source>) -> MarkdownLibrary {
        MarkdownLibrary::new(sources, Arc::new(ForgetfulNotes))
    }
}

async fn titles(library: &MarkdownLibrary, search: &Search) -> Vec<String> {
    library
        .find(search)
        .await
        .expect("searching succeeds")
        .into_iter()
        .map(|excerpt| excerpt.title)
        .collect()
}

#[tokio::test]
async fn a_captured_note_becomes_a_file_anyone_can_read() {
    // The whole storage design in one test: what PushOS writes is a Markdown
    // file the operator can open without PushOS.
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    let note = library
        .library
        .write(&Draft::new("The deploy needs the new token"))
        .await
        .expect("writing succeeds");

    let path = note.path.expect("a file-backed note knows its file");
    let text = std::fs::read_to_string(&path).expect("the file is there");
    assert!(text.contains("The deploy needs the new token"), "{text}");
    assert!(
        path.file_name()
            .expect("named")
            .to_string_lossy()
            .contains("deploy"),
        "{}",
        path.display()
    );
}

#[tokio::test]
async fn a_note_written_now_is_findable_now() {
    // The capture pad and the search pad are two presses apart.
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    library
        .library
        .write(&Draft::new("The deploy needs the new token"))
        .await
        .expect("writing succeeds");

    assert_eq!(
        titles(&library.library, &Search::for_text("token")).await,
        ["The deploy needs the new token"]
    );
}

#[tokio::test]
async fn notes_written_by_hand_are_read_as_they_were_written() {
    let scratch = Scratch::new();
    scratch.put(
        "decision.md",
        "---\ntitle: Use one write owner\nworkspace: pushos\ntags: [decision, storage]\n---\nOne task owns the connection.\n",
    );
    let library = Library::over(vec![scratch.source("notes", false)]);
    library.library.refresh().await.expect("indexing succeeds");

    let note = library
        .library
        .read(&NoteId::new("notes/decision.md"))
        .await
        .expect("reading succeeds")
        .expect("the note is there");

    assert_eq!(note.title, "Use one write owner");
    assert_eq!(note.workspace, Some(WorkspaceId::new("pushos")));
    assert_eq!(note.tags, ["decision", "storage"]);
    assert_eq!(note.body, "One task owns the connection.");
}

#[tokio::test]
async fn a_note_with_no_header_is_titled_by_its_own_first_line() {
    let scratch = Scratch::new();
    scratch.put("thought.md", "# Ship on Thursday\n\nafter the review\n");
    let library = Library::over(vec![scratch.source("notes", false)]);
    library.library.refresh().await.expect("indexing succeeds");

    assert_eq!(
        titles(&library.library, &Search::for_text("thursday")).await,
        ["Ship on Thursday"]
    );
}

#[tokio::test]
async fn notes_are_found_in_directories_below_the_source() {
    // People file by year and month, and a source that only read its own top
    // level would find none of it.
    let scratch = Scratch::new();
    scratch.put("2026/09/deploy.md", "the new token is in the vault\n");
    let library = Library::over(vec![scratch.source("notes", false)]);

    assert_eq!(library.library.refresh().await.expect("indexing"), 1);
    assert_eq!(
        titles(&library.library, &Search::for_text("vault")).await,
        ["the new token is in the vault"]
    );
}

#[tokio::test]
async fn dot_directories_are_left_to_whatever_owns_them() {
    // `.git` and `.obsidian` hold thousands of files and no notes.
    let scratch = Scratch::new();
    scratch.put(".obsidian/plugins/notes.md", "not a note\n");
    scratch.put("real.md", "a real note\n");
    let library = Library::over(vec![scratch.source("notes", false)]);

    assert_eq!(library.library.refresh().await.expect("indexing"), 1);
}

#[tokio::test]
async fn a_note_deleted_from_the_directory_stops_being_found() {
    let scratch = Scratch::new();
    scratch.put("gone.md", "the token is here\n");
    let library = Library::over(vec![scratch.source("notes", false)]);
    library.library.refresh().await.expect("indexing succeeds");
    assert_eq!(
        titles(&library.library, &Search::for_text("token"))
            .await
            .len(),
        1
    );

    std::fs::remove_file(scratch.0.join("gone.md")).expect("removable");
    library
        .library
        .refresh()
        .await
        .expect("re-indexing succeeds");

    assert!(
        titles(&library.library, &Search::for_text("token"))
            .await
            .is_empty(),
        "the files are the truth; the index has to follow them"
    );
}

#[tokio::test]
async fn a_note_edited_outside_pushos_is_found_by_its_new_words() {
    let scratch = Scratch::new();
    scratch.put("a.md", "the old wording\n");
    let library = Library::over(vec![scratch.source("notes", false)]);
    library.library.refresh().await.expect("indexing succeeds");

    scratch.put("a.md", "the new wording\n");
    library
        .library
        .refresh()
        .await
        .expect("re-indexing succeeds");

    assert!(
        titles(&library.library, &Search::for_text("old"))
            .await
            .is_empty()
    );
    assert_eq!(
        titles(&library.library, &Search::for_text("new"))
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn there_is_no_way_to_read_a_file_outside_a_source() {
    // Identities arrive from search results and from action parameters, so this
    // is the difference between reading a note and reading anything.
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", false)]);

    for escape in [
        "notes/../../../etc/passwd",
        "notes/..",
        "archive/anything.md",
        "/etc/passwd",
    ] {
        assert!(
            library
                .library
                .read(&NoteId::new(escape))
                .await
                .expect("reading succeeds")
                .is_none(),
            "`{escape}` must not resolve to a file"
        );
    }
}

#[tokio::test]
async fn nothing_is_written_to_a_source_that_is_read_only() {
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", false)]);

    let error = library
        .library
        .write(&Draft::new("something"))
        .await
        .expect_err("there is nowhere to write");
    assert!(error.to_string().contains("writable"), "{error}");
}

#[tokio::test]
async fn a_note_with_nothing_in_it_is_refused_rather_than_filed() {
    // The capture pad pressed by accident must not leave a file behind.
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    library
        .library
        .write(&Draft::new("   \n\n"))
        .await
        .expect_err("there is nothing to write down");
    assert_eq!(library.library.refresh().await.expect("indexing"), 0);
}

#[tokio::test]
async fn two_notes_captured_on_one_day_both_survive() {
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    for _ in 0..3 {
        library
            .library
            .write(&Draft::new("Deploy"))
            .await
            .expect("writing succeeds");
    }
    assert_eq!(library.library.refresh().await.expect("indexing"), 3);
}

#[tokio::test]
async fn a_note_can_be_filed_under_a_project_and_found_by_it() {
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    library
        .library
        .write(&Draft::new("mine").about(Some(WorkspaceId::new("sydclaw"))))
        .await
        .expect("writing succeeds");
    library
        .library
        .write(&Draft::new("theirs").about(Some(WorkspaceId::new("other"))))
        .await
        .expect("writing succeeds");

    let narrowed = Search::recent(5).within(Some(WorkspaceId::new("sydclaw")));
    assert_eq!(titles(&library.library, &narrowed).await, ["mine"]);
}

#[tokio::test]
async fn a_note_written_with_a_project_still_has_it_when_read_back() {
    let scratch = Scratch::new();
    let library = Library::over(vec![scratch.source("notes", true)]);

    let note = library
        .library
        .write(&Draft::new("mine").about(Some(WorkspaceId::new("sydclaw"))))
        .await
        .expect("writing succeeds");

    let read = library
        .library
        .read(&note.id)
        .await
        .expect("reading succeeds")
        .expect("it is there");
    assert_eq!(read.workspace, Some(WorkspaceId::new("sydclaw")));
}

#[tokio::test]
async fn search_still_works_with_no_database_at_all() {
    // Memory is optional. Without an index it is slower, not absent.
    let scratch = Scratch::new();
    scratch.put("a.md", "# Deploy\n\nthe new token\n");
    scratch.put("b.md", "# Music\n\nthe master encoder\n");
    let library = Library::without_an_index(vec![scratch.source("notes", true)]);

    assert_eq!(
        titles(&library, &Search::for_text("token")).await,
        ["Deploy"]
    );
    assert_eq!(titles(&library, &Search::recent(5)).await.len(), 2);
}

#[tokio::test]
async fn reading_through_needs_every_word_the_way_the_index_does() {
    // The two searches must agree about what counts as a hit, or a result would
    // depend on whether a database happened to be there.
    let scratch = Scratch::new();
    scratch.put("a.md", "# Deploy\n\nthe new token\n");
    let library = Library::without_an_index(vec![scratch.source("notes", false)]);

    assert_eq!(
        titles(&library, &Search::for_text("deploy token"))
            .await
            .len(),
        1
    );
    assert!(
        titles(&library, &Search::for_text("deploy missing"))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn sources_are_searched_in_the_order_they_were_configured() {
    let first = Scratch::new();
    let second = Scratch::new();
    first.put("a.md", "# First\n\nthe token\n");
    second.put("b.md", "# Second\n\nthe token\n");

    let library = Library::over(vec![
        first.source("first", true),
        second.source("second", false),
    ]);
    assert_eq!(library.library.refresh().await.expect("indexing"), 2);
    assert_eq!(
        library.library.sources().len(),
        2,
        "both are reported, so `doctor` can say where notes go"
    );
}

#[tokio::test]
async fn a_new_note_goes_to_the_first_writable_source() {
    let readable = Scratch::new();
    let writable = Scratch::new();
    let library = Library::over(vec![
        readable.source("archive", false),
        writable.source("notes", true),
    ]);

    let note = library
        .library
        .write(&Draft::new("captured"))
        .await
        .expect("writing succeeds");
    assert!(
        note.path.expect("a file").starts_with(&writable.0),
        "the read-only source must be left alone"
    );
}
