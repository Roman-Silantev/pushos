//! Push to talk, from a control going down to an action being carried out.
//!
//! The unit tests prove the router routes and the listener listens. This proves
//! the path an operator actually uses: a real binding table, the real
//! dispatcher, the real permission check, and a control that goes down and
//! comes back up.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_actions::providers::voice::VoiceProvider;
use pushos_config::{ConfigFile, ConfigStore, RuntimeConfig};
use pushos_domain::controls::{ButtonId, ControlId};
use pushos_domain::ids::ExecutionId;
use pushos_domain::input::{ControlEvent, InputPhase};
use pushos_domain::ports::PushOutput;
use pushos_domain::voice::Listening;
use pushos_runtime::{RunningRuntime, Runtime, Shutdown};
use pushos_testkit::{FakeMicrophone, FakePush, FakeTranscriber, speech};
use pushos_ui::PushRenderer;
use pushos_voice::{VoiceListener, VoiceRouter};

/// One control bound twice, which is what push to talk is.
const CONFIG: &str = r#"
[runtime]
home_page = "home"

[permissions]
granted = ["microphone.listen"]

[[pages]]
id = "home"
name = "Home"

[voice]
request = "page.next"

[[voice.commands]]
phrase = "go back"
action = "page.back"

[[voice.commands]]
phrase = "ship it"
action = "page.home"
confirm = true

[[bindings]]
control = "button.select"
gesture = "press"
action = "voice.listen"

[[bindings]]
control = "button.select"
gesture = "release"
action = "voice.transcribe"

[[bindings]]
control = "button.lower_8"
gesture = "press"
action = "voice.confirm"
"#;

struct Harness {
    running: RunningRuntime,
    microphone: Arc<FakeMicrophone>,
    transcriber: Arc<FakeTranscriber>,
    listener: Arc<VoiceListener>,
    shutdown: Shutdown,
    root: std::path::PathBuf,
}

impl Harness {
    async fn start() -> Self {
        let root = std::env::temp_dir().join(format!("pushos-voice-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&root).expect("the temporary directory is writable");
        std::fs::write(root.join("pushos.toml"), CONFIG).expect("writable");

        let parsed: ConfigFile = toml::from_str(CONFIG).expect("the test configuration parses");
        let built = RuntimeConfig::build(&parsed).expect("the test configuration is valid");
        let request = built
            .voice
            .as_ref()
            .and_then(|settings| settings.request.clone());
        let commands = built
            .voice
            .as_ref()
            .map(|settings| settings.commands.clone())
            .unwrap_or_default();

        let config = Arc::new(ConfigStore::load(&root).expect("the configuration is valid"));

        let microphone = Arc::new(FakeMicrophone::new());
        let transcriber = Arc::new(FakeTranscriber::new());
        let listener = Arc::new(VoiceListener::new(
            Arc::clone(&microphone) as Arc<_>,
            Arc::clone(&transcriber) as Arc<_>,
            VoiceRouter::new(commands),
        ));
        let provider = Arc::new(VoiceProvider::new(Arc::clone(&listener), request));

        let shutdown = Shutdown::new();
        let runtime = Runtime::new(config)
            .with_voice(Arc::clone(&provider))
            .with_provider(Arc::new(
                pushos_actions::providers::page::PageProvider::new(),
            ))
            .expect("the namespace is free")
            .with_provider(provider)
            .expect("the namespace is free");

        Self {
            running: runtime.start(&shutdown).await,
            microphone,
            transcriber,
            listener,
            shutdown,
            root,
        }
    }

    /// Attaches a surface, runs the closure against it, then unplugs it.
    async fn with_surface<F, Fut>(&self, use_it: F)
    where
        F: FnOnce(FakePush) -> Fut,
        Fut: Future<Output = ()>,
    {
        let (surface, input) = FakePush::new();
        let output: Arc<dyn PushOutput> = Arc::new(surface.clone());
        let renderer = PushRenderer::new().expect("the renderer builds");

        let serving = self
            .running
            .serve(Box::new(input), output, renderer, &self.shutdown);

        let driving = async {
            use_it(surface.clone()).await;
            surface.disconnect().await;
        };

        tokio::join!(serving, driving);
    }

    async fn stop(self) {
        self.running.stop().await;
        self.shutdown.stop().await;
        std::fs::remove_dir_all(&self.root).ok();
    }
}

/// Presses a button and lets go, the way an operator holds one to talk.
async fn hold(surface: &FakePush, button: ButtonId) {
    let at = Instant::now();
    surface
        .inject(ControlEvent::new(
            ControlId::Button(button),
            InputPhase::Down { velocity: 127 },
            at,
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    surface
        .inject(ControlEvent::new(
            ControlId::Button(button),
            InputPhase::Up,
            Instant::now(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(160)).await;
}

/// Presses a button and lets go immediately.
async fn press(surface: &FakePush, button: ButtonId) {
    let at = Instant::now();
    surface
        .inject(ControlEvent::new(
            ControlId::Button(button),
            InputPhase::Down { velocity: 127 },
            at,
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    surface
        .inject(ControlEvent::new(
            ControlId::Button(button),
            InputPhase::Up,
            Instant::now(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(60)).await;
}

#[tokio::test]
async fn holding_a_control_and_speaking_runs_what_was_said() {
    let harness = Harness::start().await;
    harness.microphone.will_hear(speech(Duration::from_secs(1)));
    harness.transcriber.will_say("Go back.");

    harness
        .with_surface(|surface| async move {
            hold(&surface, ButtonId::Select).await;
        })
        .await;

    assert_eq!(
        harness.microphone.starts(),
        1,
        "the press should have opened the microphone"
    );
    assert_eq!(
        harness.transcriber.heard().len(),
        1,
        "the release should have sent what was recorded to the engine"
    );
    assert_eq!(
        harness.listener.state(),
        Listening::Idle,
        "letting go must always end with the microphone off"
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_phrase_needing_confirmation_waits_for_a_press() {
    let harness = Harness::start().await;
    harness.microphone.will_hear(speech(Duration::from_secs(1)));
    harness.transcriber.will_say("ship it");

    harness
        .with_surface(|surface| async move {
            hold(&surface, ButtonId::Select).await;
        })
        .await;

    assert!(
        harness.listener.pending().await.is_some(),
        "transcription is not authorisation; it must still be waiting"
    );

    harness
        .with_surface(|surface| async move {
            press(&surface, ButtonId::Lower8).await;
        })
        .await;

    assert!(
        harness.listener.pending().await.is_none(),
        "one press should take it, so a second cannot run it again"
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_control_brushed_rather_than_held_does_nothing_at_all() {
    let harness = Harness::start().await;
    harness.microphone.will_hear_nothing();

    harness
        .with_surface(|surface| async move {
            hold(&surface, ButtonId::Select).await;
        })
        .await;

    assert!(
        harness.transcriber.heard().is_empty(),
        "a mis-press must not reach the engine"
    );
    assert_eq!(harness.listener.state(), Listening::Idle);
    harness.stop().await;
}

#[tokio::test]
async fn the_surface_says_it_is_listening_while_the_control_is_down() {
    // The one thing an operator must never be unsure about.
    //
    // This is also what caught the input loop spinning: with no agents,
    // terminals or workflows configured, the session publisher was dropped, and
    // the closed channel made one arm of the loop ready for ever. Everything
    // below it, including this, was starved.
    let harness = Harness::start().await;
    harness.microphone.will_hear(speech(Duration::from_secs(1)));
    harness.transcriber.will_say("go back");

    harness
        .with_surface(|surface| async move {
            let at = Instant::now();
            surface
                .inject(ControlEvent::new(
                    ControlId::Button(ButtonId::Select),
                    InputPhase::Down { velocity: 127 },
                    at,
                ))
                .await;
            tokio::time::sleep(Duration::from_millis(150)).await;
        })
        .await;

    assert_eq!(
        harness.running.view().borrow().snapshot.listening,
        Listening::Recording,
        "the display must say so while the microphone is on"
    );
    harness.stop().await;
}
