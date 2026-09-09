//! The note index, exercised against a real database.
//!
//! Full text search is the sort of thing that looks right in a unit test and is
//! wrong against SQLite, so none of this is faked: a real FTS5 table, real
//! triggers, and real queries.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::{Duration, UNIX_EPOCH};

use pushos_domain::ids::{NoteId, SourceId, WorkspaceId};
use pushos_domain::memory::{Note, Search};
use pushos_domain::ports::NoteIndex;
use pushos_storage::{Storage, StorageWriter};

fn writer() -> StorageWriter {
    StorageWriter::in_memory().expect("an in-memory database always opens")
}

fn note(id: &str, title: &str, body: &str) -> Note {
    Note {
        id: NoteId::new(id),
        title: title.to_owned(),
        body: body.to_owned(),
        source: SourceId::new("notes"),
        path: Some(std::path::PathBuf::from(format!("/tmp/{id}"))),
        workspace: None,
        tags: Vec::new(),
        written_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    }
}

async fn given(storage: &Storage, notes: impl IntoIterator<Item = Note>) {
    for note in notes {
        storage.record(&note).await.expect("indexing succeeds");
    }
}

async fn titles(storage: &Storage, search: &Search) -> Vec<String> {
    storage
        .search(search)
        .await
        .expect("searching succeeds")
        .into_iter()
        .map(|excerpt| excerpt.title)
        .collect()
}

#[tokio::test]
async fn a_note_can_be_found_by_a_word_in_its_body() {
    let writer = writer();
    let storage = writer.handle();
    given(
        &storage,
        [
            note("a.md", "Deploy", "the deploy needs the new token"),
            note("b.md", "Music", "the volume encoder is the master one"),
        ],
    )
    .await;

    assert_eq!(
        titles(&storage, &Search::for_text("token")).await,
        ["Deploy"]
    );
}

#[tokio::test]
async fn a_word_matches_by_its_beginning() {
    // An operator saying "deploy" must find "deployment", because they are
    // speaking rather than typing a query.
    let writer = writer();
    let storage = writer.handle();
    given(&storage, [note("a.md", "Ship", "the deployment is manual")]).await;

    assert_eq!(
        titles(&storage, &Search::for_text("deploy")).await,
        ["Ship"]
    );
}

#[tokio::test]
async fn dictated_punctuation_does_not_break_the_search() {
    // What a transcriber produces, complete with the comma and the question
    // mark. In FTS5's own syntax this is a parse error.
    let writer = writer();
    let storage = writer.handle();
    given(&storage, [note("a.md", "Deploy", "needs the new token")]).await;

    let found = titles(&storage, &Search::for_text("what's the deploy, token?")).await;
    assert!(found.is_empty() || found == ["Deploy"], "{found:?}");

    assert_eq!(
        titles(&storage, &Search::for_text("deploy, token?")).await,
        ["Deploy"]
    );
}

#[tokio::test]
async fn a_title_match_outranks_a_body_match() {
    let writer = writer();
    let storage = writer.handle();
    given(
        &storage,
        [
            note("body.md", "Something else", "a passing mention of deploy"),
            note("title.md", "Deploy", "unrelated words entirely"),
        ],
    )
    .await;

    let found = titles(&storage, &Search::for_text("deploy")).await;
    assert_eq!(
        found.first().map(String::as_str),
        Some("Deploy"),
        "{found:?}"
    );
}

#[tokio::test]
async fn re_recording_a_note_replaces_it_rather_than_duplicating_it() {
    // Re-reading a directory records every note again. Doing that must not
    // leave the operator with two of each.
    let writer = writer();
    let storage = writer.handle();

    given(&storage, [note("a.md", "Deploy", "the old text")]).await;
    given(&storage, [note("a.md", "Deploy", "the new text")]).await;

    let found = storage
        .search(&Search::for_text("text"))
        .await
        .expect("searching succeeds");
    assert_eq!(found.len(), 1);

    assert!(
        titles(&storage, &Search::for_text("old")).await.is_empty(),
        "the words of the old version must go with it"
    );
    assert_eq!(titles(&storage, &Search::for_text("new")).await, ["Deploy"]);
}

#[tokio::test]
async fn a_forgotten_note_stops_being_found() {
    let writer = writer();
    let storage = writer.handle();
    given(&storage, [note("a.md", "Deploy", "the new token")]).await;

    storage
        .erase(&NoteId::new("a.md"))
        .await
        .expect("forgetting succeeds");
    assert!(
        titles(&storage, &Search::for_text("token"))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn emptying_a_source_leaves_the_others_alone() {
    let writer = writer();
    let storage = writer.handle();

    let mut elsewhere = note("other/a.md", "Elsewhere", "the same token appears here");
    elsewhere.source = SourceId::new("archive");
    given(
        &storage,
        [note("a.md", "Deploy", "the new token"), elsewhere],
    )
    .await;

    storage
        .empty(&SourceId::new("notes"))
        .await
        .expect("emptying succeeds");
    assert_eq!(
        titles(&storage, &Search::for_text("token")).await,
        ["Elsewhere"]
    );
}

#[tokio::test]
async fn a_search_with_no_words_gives_the_latest_first() {
    let writer = writer();
    let storage = writer.handle();

    let mut older = note("old.md", "Older", "written first");
    older.written_at = UNIX_EPOCH + Duration::from_secs(1_000);
    let mut newer = note("new.md", "Newer", "written second");
    newer.written_at = UNIX_EPOCH + Duration::from_secs(2_000);
    given(&storage, [older, newer]).await;

    assert_eq!(
        titles(&storage, &Search::recent(5)).await,
        ["Newer", "Older"]
    );
}

#[tokio::test]
async fn a_search_can_be_narrowed_to_one_project() {
    let writer = writer();
    let storage = writer.handle();

    let mut mine = note("mine.md", "Mine", "the token is here");
    mine.workspace = Some(WorkspaceId::new("sydclaw"));
    let mut theirs = note("theirs.md", "Theirs", "the token is here too");
    theirs.workspace = Some(WorkspaceId::new("other"));
    given(&storage, [mine, theirs]).await;

    let narrowed = Search::for_text("token").within(Some(WorkspaceId::new("sydclaw")));
    assert_eq!(titles(&storage, &narrowed).await, ["Mine"]);

    let latest = Search::recent(5).within(Some(WorkspaceId::new("sydclaw")));
    assert_eq!(titles(&storage, &latest).await, ["Mine"]);
}

#[tokio::test]
async fn a_search_can_be_narrowed_to_one_tag() {
    let writer = writer();
    let storage = writer.handle();

    let mut decision = note("a.md", "Decided", "we use the new token");
    decision.tags = vec!["decision".to_owned()];
    let mut idea = note("b.md", "Idea", "maybe a different token");
    idea.tags = vec!["idea".to_owned()];
    given(&storage, [decision, idea]).await;

    let mut search = Search::for_text("token");
    search.tag = Some("decision".to_owned());
    assert_eq!(titles(&storage, &search).await, ["Decided"]);
}

#[tokio::test]
async fn a_tag_matches_whole_words_rather_than_pieces_of_them() {
    // "deploy" must not match a note tagged "deployment", or filing would stop
    // meaning anything.
    let writer = writer();
    let storage = writer.handle();

    let mut tagged = note("a.md", "Long", "the token");
    tagged.tags = vec!["deployment".to_owned()];
    given(&storage, [tagged]).await;

    let mut search = Search::recent(5);
    search.tag = Some("deploy".to_owned());
    assert!(titles(&storage, &search).await.is_empty());

    search.tag = Some("deployment".to_owned());
    assert_eq!(titles(&storage, &search).await, ["Long"]);
}

#[tokio::test]
async fn a_search_returns_no_more_than_it_was_asked_for() {
    let writer = writer();
    let storage = writer.handle();
    given(
        &storage,
        (0..10).map(|n| note(&format!("{n}.md"), "Note", "the same token every time")),
    )
    .await;

    let mut search = Search::for_text("token");
    search.limit = 3;
    assert_eq!(titles(&storage, &search).await.len(), 3);
}

#[tokio::test]
async fn a_result_carries_the_part_that_matched() {
    let writer = writer();
    let storage = writer.handle();
    given(
        &storage,
        [note(
            "a.md",
            "Deploy",
            "a long preamble that is not relevant at all, and then the token itself",
        )],
    )
    .await;

    let found = storage
        .search(&Search::for_text("token"))
        .await
        .expect("searching succeeds");
    assert!(
        found[0].snippet.contains("token"),
        "the operator needs to see why it matched: {:?}",
        found[0].snippet
    );
}

#[tokio::test]
async fn a_note_indexed_now_is_findable_now() {
    // The capture pad and the search pad are two presses apart, so a note that
    // needed a flush before it could be found would look lost.
    let writer = writer();
    let storage = writer.handle();

    storage
        .record(&note("a.md", "Just now", "the thing I just said"))
        .await
        .expect("indexing succeeds");
    assert_eq!(
        titles(&storage, &Search::for_text("said")).await,
        ["Just now"]
    );
}
