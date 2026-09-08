//! Storage behaviour, exercised against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use pushos_domain::event::{DomainEvent, EventEnvelope, EventSource};
use pushos_domain::ids::CorrelationId;
use pushos_storage::{EventRecord, StorageWriter, WorkspaceMemoryRow};

fn writer() -> StorageWriter {
    StorageWriter::in_memory().expect("an in-memory database always opens")
}

fn event(correlation: CorrelationId, payload: DomainEvent) -> EventRecord {
    EventRecord::from_envelope(&EventEnvelope::root(
        EventSource::Actions,
        correlation,
        payload,
    ))
}

#[tokio::test]
async fn a_setting_survives_a_round_trip() {
    let writer = writer();
    let storage = writer.handle();

    storage
        .put_setting("last_page", "development")
        .expect("the writer accepts work");
    assert_eq!(
        storage
            .get_setting("last_page")
            .await
            .expect("the read completes"),
        Some("development".to_owned())
    );
}

#[tokio::test]
async fn reading_a_setting_that_was_never_written_yields_nothing() {
    let writer = writer();
    assert_eq!(
        writer
            .handle()
            .get_setting("absent")
            .await
            .expect("completes"),
        None
    );
}

#[tokio::test]
async fn writing_a_setting_twice_replaces_it() {
    let writer = writer();
    let storage = writer.handle();

    storage.put_setting("home", "one").expect("accepted");
    storage.put_setting("home", "two").expect("accepted");

    assert_eq!(
        storage.get_setting("home").await.expect("completes"),
        Some("two".to_owned())
    );
}

#[tokio::test]
async fn a_workspace_remembers_where_it_was_left() {
    let writer = writer();
    let storage = writer.handle();

    storage
        .remember_workspace(WorkspaceMemoryRow {
            workspace_id: "sydclaw".to_owned(),
            last_page: Some("development".to_owned()),
            selected_session: Some("builder".to_owned()),
        })
        .expect("accepted");

    let recalled = storage
        .recall_workspace("sydclaw")
        .await
        .expect("completes")
        .expect("the workspace was remembered");

    assert_eq!(recalled.last_page.as_deref(), Some("development"));
    assert_eq!(recalled.selected_session.as_deref(), Some("builder"));
}

#[tokio::test]
async fn recalling_an_unknown_workspace_yields_nothing_rather_than_failing() {
    let writer = writer();
    assert!(
        writer
            .handle()
            .recall_workspace("never-used")
            .await
            .expect("completes")
            .is_none()
    );
}

#[tokio::test]
async fn remembering_a_workspace_twice_updates_it() {
    let writer = writer();
    let storage = writer.handle();

    for page in ["home", "music"] {
        storage
            .remember_workspace(WorkspaceMemoryRow {
                workspace_id: "sydclaw".to_owned(),
                last_page: Some(page.to_owned()),
                selected_session: None,
            })
            .expect("accepted");
    }

    let recalled = storage
        .recall_workspace("sydclaw")
        .await
        .expect("completes")
        .expect("present");
    assert_eq!(recalled.last_page.as_deref(), Some("music"));
}

#[tokio::test]
async fn events_are_kept_newest_first() {
    let writer = writer();
    let storage = writer.handle();
    let correlation = CorrelationId::generate();

    storage
        .record_event(event(correlation, DomainEvent::PushConnected))
        .expect("accepted");
    storage
        .record_event(event(
            correlation,
            DomainEvent::PageChanged {
                page: "music".into(),
            },
        ))
        .expect("accepted");

    let recent = storage.recent_events(10).await.expect("completes");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].kind, "page.changed");
    assert_eq!(recent[1].kind, "push.connected");
}

#[tokio::test]
async fn every_event_from_one_gesture_shares_a_correlation() {
    let writer = writer();
    let storage = writer.handle();
    let correlation = CorrelationId::generate();

    for payload in [
        DomainEvent::PushConnected,
        DomainEvent::PageChanged {
            page: "home".into(),
        },
        DomainEvent::ConfigUpdated { binding_count: 3 },
    ] {
        storage
            .record_event(event(correlation, payload))
            .expect("accepted");
    }

    let recent = storage.recent_events(10).await.expect("completes");
    assert!(
        recent
            .iter()
            .all(|row| row.correlation_id == correlation.to_string())
    );
}

#[tokio::test]
async fn recording_the_same_event_twice_stores_it_once() {
    let writer = writer();
    let storage = writer.handle();
    let record = event(CorrelationId::generate(), DomainEvent::PushConnected);

    storage.record_event(record.clone()).expect("accepted");
    storage.record_event(record).expect("accepted");

    assert_eq!(storage.recent_events(10).await.expect("completes").len(), 1);
}

#[tokio::test]
async fn a_limit_is_honoured() {
    let writer = writer();
    let storage = writer.handle();

    for _ in 0..20 {
        storage
            .record_event(event(CorrelationId::generate(), DomainEvent::PushConnected))
            .expect("accepted");
    }

    assert_eq!(storage.recent_events(5).await.expect("completes").len(), 5);
}

#[tokio::test]
async fn queued_writes_are_committed_before_the_writer_stops() {
    let writer = writer();
    let storage = writer.handle();

    for _ in 0..50 {
        storage
            .record_event(event(CorrelationId::generate(), DomainEvent::PushConnected))
            .expect("accepted");
    }

    // The count is read through the same task, so it cannot observe a partial
    // queue: the read is ordered behind every write above.
    assert_eq!(
        storage.recent_events(100).await.expect("completes").len(),
        50
    );
}

#[tokio::test]
async fn work_offered_after_the_writer_stops_is_refused_rather_than_lost_silently() {
    let storage = {
        let writer = writer();
        writer.handle()
    };

    let error = storage
        .record_event(event(CorrelationId::generate(), DomainEvent::PushConnected))
        .expect_err("the writer has stopped");
    assert!(error.is_fatal());
}

#[tokio::test]
async fn a_database_on_disk_keeps_its_contents_across_reopening() {
    let directory = std::env::temp_dir().join(format!(
        "pushos-storage-{}",
        pushos_domain::ids::ExecutionId::generate()
    ));
    let path = directory.join("state.sqlite");

    {
        let writer = StorageWriter::open(&path).expect("the database is creatable");
        writer
            .handle()
            .put_setting("home_page", "development")
            .expect("accepted");
    }

    {
        let writer = StorageWriter::open(&path).expect("the database reopens");
        assert_eq!(
            writer
                .handle()
                .get_setting("home_page")
                .await
                .expect("completes"),
            Some("development".to_owned())
        );
    }

    std::fs::remove_dir_all(&directory).ok();
}

/// Regression: handles are cloneable and may outlive the writer, so shutdown
/// cannot wait for the request channel to disconnect. It once did, and dropping
/// the writer while any handle was still alive hung forever.
#[tokio::test]
async fn dropping_the_writer_returns_even_while_handles_are_still_alive() {
    let held = {
        let writer = writer();
        let handle = writer.handle();
        let another = handle.clone();

        handle.put_setting("key", "value").expect("accepted");
        drop(writer);

        another
    };

    assert!(held.get_setting("key").await.is_err(), "the writer is gone");
}
