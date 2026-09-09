//! Notes, from a pad going down to a file on disk.
//!
//! The unit tests prove the library writes files and the index finds them. This
//! proves the path an operator actually uses: a real binding table, the real
//! dispatcher, the real permission check, and a real pad.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_actions::providers::memory::MemoryProvider;
use pushos_config::{ConfigFile, ConfigStore, RuntimeConfig};
use pushos_domain::controls::PadIndex;
use pushos_domain::ids::{ExecutionId, SourceId};
use pushos_domain::memory::Search;
use pushos_domain::ports::MemoryStore;
use pushos_domain::ports::{NoteIndex, PushOutput, Source};
use pushos_memory::MarkdownLibrary;
use pushos_runtime::{RunningRuntime, Runtime, Shutdown};
use pushos_storage::StorageWriter;
use pushos_testkit::{FakePush, RecordingProvider};
use pushos_ui::PushRenderer;

/// A pad that writes a note, one that finds them, one that briefs an agent.
const CONFIG: &str = r#"
[runtime]
home_page = "home"

[permissions]
granted = ["filesystem.write"]

[[pages]]
id = "home"
name = "Home"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "memory.write"
[bindings.params]
text = "The deploy needs the new token"

[[bindings]]
control = "pad.1"
gesture = "tap"
action = "memory.recent"

[[bindings]]
control = "pad.2"
gesture = "tap"
action = "memory.brief"
[bindings.params]
text = "token"
"#;

/// The same thing without the permission that lets PushOS write a file.
const UNPERMITTED: &str = r#"
[runtime]
home_page = "home"

[[pages]]
id = "home"
name = "Home"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "memory.write"
[bindings.params]
text = "The deploy needs the new token"
"#;

struct Harness {
    running: RunningRuntime,
    library: Arc<MarkdownLibrary>,
    /// Stands in for the agent a briefing is handed to, reached through the
    /// real dispatcher rather than around it.
    agent: Arc<RecordingProvider>,
    shutdown: Shutdown,
    notes: PathBuf,
    root: PathBuf,
    _writer: StorageWriter,
}

impl Harness {
    async fn start(config_text: &str) -> Self {
        Self::start_with(config_text, &[]).await
    }

    /// Starts with notes already in the directory, as an operator would have.
    async fn start_with(config_text: &str, existing: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("pushos-mem-{}", ExecutionId::generate()));
        let notes = root.join("notes");
        std::fs::create_dir_all(&notes).expect("the temporary directory is writable");
        for (name, text) in existing {
            std::fs::write(notes.join(name), text).expect("writable");
        }
        std::fs::write(root.join("pushos.toml"), config_text).expect("writable");

        let parsed: ConfigFile =
            toml::from_str(config_text).expect("the test configuration parses");
        RuntimeConfig::build(&parsed).expect("the test configuration is valid");
        let config = Arc::new(ConfigStore::load(&root).expect("the configuration is valid"));

        let writer = StorageWriter::in_memory().expect("an in-memory database always opens");
        let index: Arc<dyn NoteIndex> = Arc::new(writer.handle());
        let library = Arc::new(MarkdownLibrary::new(
            vec![Source {
                id: SourceId::new("notes"),
                root: notes.clone(),
                writable: true,
            }],
            index,
        ));

        let provider = Arc::new(MemoryProvider::new(
            Arc::clone(&library) as Arc<_>,
            Some("agent.prompt".parse().expect("a valid selector")),
        ));

        // The runtime gives the provider the real dispatcher, which is the
        // point: a briefing takes the same route a pad does, permission check
        // and all. So the thing it reaches has to be a real provider too.
        let agent = Arc::new(RecordingProvider::new("agent", ["prompt"]));

        let shutdown = Shutdown::new();
        let runtime = Runtime::new(config)
            .with_memory(Arc::clone(&provider))
            .with_provider(provider)
            .expect("the namespace is free")
            .with_provider(Arc::clone(&agent) as Arc<_>)
            .expect("the namespace is free");

        Self {
            running: runtime.start(&shutdown).await,
            library,
            agent,
            shutdown,
            notes,
            root,
            _writer: writer,
        }
    }

    /// Attaches a surface, taps some pads, then unplugs it.
    async fn tapping(&self, pads: &[u8]) {
        let (surface, input) = FakePush::new();
        let output: Arc<dyn PushOutput> = Arc::new(surface.clone());
        let renderer = PushRenderer::new().expect("the renderer builds");

        let serving = self
            .running
            .serve(Box::new(input), output, renderer, &self.shutdown);

        let driving = async {
            for index in pads {
                let at = Instant::now();
                let pad = PadIndex::new(*index).expect("test pad index is in range");
                surface.press_pad(pad, at).await;
                surface.release_pad(pad, at).await;
                tokio::time::sleep(Duration::from_millis(160)).await;
            }
            surface.disconnect().await;
        };

        tokio::join!(serving, driving);
    }

    /// How many notes match, once the startup indexing has caught up.
    async fn eventually_finds(&self, text: &str) -> usize {
        for _ in 0..50 {
            let found = self
                .library
                .find(&Search::for_text(text))
                .await
                .expect("searching succeeds");
            if !found.is_empty() {
                return found.len();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        0
    }

    /// The note files that exist.
    fn written(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.notes)
            .expect("the directory is there")
            .flatten()
            .map(|entry| entry.path())
            .collect()
    }

    async fn stop(self) {
        self.running.stop().await;
        self.shutdown.stop().await;
        std::fs::remove_dir_all(&self.root).ok();
    }
}

#[tokio::test]
async fn notes_written_while_pushos_was_not_running_are_findable_without_being_asked() {
    // A directory of notes is the normal case, not an exception. A first search
    // that found nothing until a pad was pressed would look like a fault.
    let harness = Harness::start_with(
        CONFIG,
        &[(
            "decision.md",
            "# One write owner\n\nthe storage task owns the connection\n",
        )],
    )
    .await;

    // The runtime indexes in the background rather than holding up the surface,
    // so this waits for it rather than assuming it has happened.
    let found = harness.eventually_finds("connection").await;
    assert_eq!(found, 1, "the note should have been indexed at startup");

    harness.stop().await;
}

#[tokio::test]
async fn a_pad_writes_a_note_the_operator_can_open_themselves() {
    let harness = Harness::start(CONFIG).await;
    harness.tapping(&[0]).await;

    let written = harness.written();
    assert_eq!(written.len(), 1, "one press, one file");
    let text = std::fs::read_to_string(&written[0]).expect("the file is there");
    assert!(text.contains("The deploy needs the new token"), "{text}");

    harness.stop().await;
}

#[tokio::test]
async fn a_note_written_by_one_pad_is_found_by_another() {
    let harness = Harness::start(CONFIG).await;
    harness.tapping(&[0, 1]).await;

    let found = harness
        .library
        .find(&Search::for_text("token"))
        .await
        .expect("searching succeeds");
    assert_eq!(found.len(), 1, "{found:?}");

    harness.stop().await;
}

#[tokio::test]
async fn a_pad_can_put_what_it_found_in_front_of_an_agent() {
    let harness = Harness::start(CONFIG).await;
    harness.tapping(&[0, 2]).await;

    let calls = harness.agent.calls();
    assert_eq!(calls.len(), 1, "the briefing should have reached the agent");
    assert_eq!(calls[0].selector.to_string(), "agent.prompt");
    assert!(
        calls[0]
            .params
            .text("text")
            .is_some_and(|text| text.contains("new token")),
        "the agent has to receive the note itself"
    );

    harness.stop().await;
}

#[tokio::test]
async fn without_the_permission_nothing_is_written() {
    // Permission is checked in the dispatcher, so the provider never runs and
    // no file appears. This is what makes the grant mean something.
    let harness = Harness::start(UNPERMITTED).await;
    harness.tapping(&[0]).await;

    assert!(
        harness.written().is_empty(),
        "a note is a file on the operator's disk, and that needs saying yes to"
    );

    harness.stop().await;
}
