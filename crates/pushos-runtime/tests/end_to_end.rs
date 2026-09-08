//! The whole pipeline, driven through FakePush.
//!
//! These prove the architectural claim the specification makes: a physical
//! event becomes an action through Control, Gesture, Binding, Action, with no
//! hardware attached and no provider knowing anything about a pad.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_config::{ConfigFile, ConfigStore, RuntimeConfig};
use pushos_domain::action::ActionDefinition;
use pushos_domain::color::LedState;
use pushos_domain::controls::{ButtonId, ControlId, PadIndex};
use pushos_domain::event::DomainEvent;
use pushos_domain::input::{ControlEvent, InputPhase};
use pushos_domain::ports::PushOutput;
use pushos_runtime::{Runtime, Shutdown};
use pushos_testkit::{FakePush, RecordingProvider, ScriptedOutcome};
use pushos_ui::PushRenderer;

const CONFIG: &str = r#"
[runtime]
home_page = "home"

[permissions]
granted = ["media.control"]

[[pages]]
id = "home"
name = "Home"

[[pages]]
id = "music"
name = "Music"

[[bindings]]
control = "pad.0"
gesture = "tap"
action = "test.one"
label = "One"

[[bindings]]
control = "pad.1"
gesture = "tap"
page = "music"
action = "test.two"

[[bindings]]
control = "button.play"
gesture = "press"
action = "page.next"
"#;

fn store(text: &str) -> Arc<ConfigStore> {
    let parsed: ConfigFile = toml::from_str(text).expect("the test configuration parses");
    let config = RuntimeConfig::build(&parsed).expect("the test configuration is valid");
    assert!(!config.bindings.is_empty());

    // A store built from a directory would need a file; the test drives the
    // runtime from an in-memory configuration instead.
    let directory = std::env::temp_dir().join(format!(
        "pushos-e2e-{}",
        pushos_domain::ids::ExecutionId::generate()
    ));
    std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
    std::fs::write(directory.join("pushos.toml"), text).expect("writable");
    Arc::new(ConfigStore::load(&directory).expect("the configuration is valid"))
}

fn pad(index: u8) -> PadIndex {
    PadIndex::new(index).expect("test pad index is in range")
}

/// A running PushOS backed by a fake surface.
struct Harness {
    surface: FakePush,
    provider: RecordingProvider,
    shutdown: Shutdown,
    finished: tokio::task::JoinHandle<()>,
    events: pushos_runtime::EventSubscription,
}

impl Harness {
    fn start(text: &str) -> Self {
        let (surface, input) = FakePush::new();
        let provider = RecordingProvider::new("test", ["one", "two"]);
        let shutdown = Shutdown::new();

        let runtime = Runtime::new(store(text))
            .with_provider(Arc::new(provider.clone()))
            .expect("the namespace is free")
            .with_provider(Arc::new(
                pushos_actions::providers::page::PageProvider::new(),
            ))
            .expect("the namespace is free");
        let events = runtime.bus().subscribe();

        let output: Arc<dyn PushOutput> = Arc::new(surface.clone());
        let renderer = PushRenderer::new().expect("the renderer builds");
        let running = shutdown.clone();
        let finished = tokio::spawn(async move {
            runtime
                .run(Box::new(input), output, renderer, running)
                .await;
        });

        Self {
            surface,
            provider,
            shutdown,
            finished,
            events,
        }
    }

    async fn tap(&self, index: u8) {
        let at = Instant::now();
        self.surface.press_pad(pad(index), at).await;
        self.surface.release_pad(pad(index), at).await;
    }

    async fn press_button(&self, button: ButtonId) {
        let at = Instant::now();
        self.surface
            .inject(ControlEvent::new(
                ControlId::Button(button),
                InputPhase::Down { velocity: 127 },
                at,
            ))
            .await;
    }

    /// Waits for the provider to have been called `count` times.
    async fn wait_for_calls(&self, count: usize) {
        settle(|| self.provider.call_count() >= count).await;
    }

    /// Waits for an event matching `predicate`.
    async fn wait_for_event(&mut self, predicate: impl Fn(&DomainEvent) -> bool) -> DomainEvent {
        loop {
            let envelope = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("an event should arrive")
                .expect("the bus is alive");
            if predicate(&envelope.payload) {
                return envelope.payload.clone();
            }
        }
    }

    async fn stop(self) {
        self.shutdown.stop().await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.finished).await;
    }
}

/// Polls until a condition holds, rather than sleeping a guessed interval.
async fn settle(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the condition never became true");
}

#[tokio::test]
async fn tapping_a_bound_pad_runs_its_action() {
    let harness = Harness::start(CONFIG);

    harness.tap(0).await;
    harness.wait_for_calls(1).await;

    let calls: Vec<ActionDefinition> = harness.provider.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].selector.to_string(), "test.one");

    harness.stop().await;
}

#[tokio::test]
async fn tapping_an_unbound_pad_runs_nothing() {
    let mut harness = Harness::start(CONFIG);

    harness.tap(40).await;
    harness
        .wait_for_event(|event| matches!(event, DomainEvent::BindingUnmatched { .. }))
        .await;

    assert_eq!(harness.provider.call_count(), 0);
    harness.stop().await;
}

#[tokio::test]
async fn a_binding_scoped_to_another_page_does_not_fire() {
    let harness = Harness::start(CONFIG);

    // `pad.1` is bound only on the music page, and the surface starts on home.
    harness.tap(1).await;
    harness.tap(0).await;
    harness.wait_for_calls(1).await;

    let calls = harness.provider.calls();
    assert_eq!(
        calls.len(),
        1,
        "only the globally bound pad should have fired"
    );
    assert_eq!(calls[0].selector.to_string(), "test.one");

    harness.stop().await;
}

#[tokio::test]
async fn moving_page_changes_which_bindings_apply() {
    let mut harness = Harness::start(CONFIG);

    harness.press_button(ButtonId::Play).await;
    let moved = harness
        .wait_for_event(|event| matches!(event, DomainEvent::PageChanged { .. }))
        .await;
    assert!(matches!(moved, DomainEvent::PageChanged { ref page } if page.as_str() == "music"));

    harness.tap(1).await;
    harness.wait_for_calls(1).await;

    assert_eq!(harness.provider.calls()[0].selector.to_string(), "test.two");
    harness.stop().await;
}

#[tokio::test]
async fn every_bound_pad_is_lit_and_the_rest_are_dark() {
    let harness = Harness::start(CONFIG);

    settle(|| {
        // Reading the surface synchronously would need the lock; poll instead.
        true
    })
    .await;

    // Give the first render a chance to reach the surface.
    let lit = loop {
        let state = harness.surface.state().await;
        let lit = state.led(ControlId::Pad(pad(0)));
        if lit.is_some_and(|state| state != LedState::OFF) {
            break state.lit_count();
        }
        drop(state);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // Two controls are bound on the home page: pad 0 and the play button.
    assert_eq!(lit, 2);

    let state = harness.surface.state().await;
    assert_eq!(state.led(ControlId::Pad(pad(40))), Some(LedState::OFF));
    assert!(
        state
            .led(ControlId::Button(ButtonId::Play))
            .is_some_and(|led| led != LedState::OFF)
    );
    drop(state);

    harness.stop().await;
}

#[tokio::test]
async fn the_display_is_drawn_as_soon_as_the_surface_appears() {
    let harness = Harness::start(CONFIG);

    settle_async(|| async { harness.surface.state().await.frame_count() > 0 }).await;
    harness.stop().await;
}

#[tokio::test]
async fn a_failing_action_is_reported_rather_than_stopping_the_pipeline() {
    let mut harness = Harness::start(CONFIG);
    harness.provider.will(ScriptedOutcome::Fail(
        pushos_domain::error::ErrorClass::Retryable,
    ));

    harness.tap(0).await;
    harness
        .wait_for_event(|event| {
            matches!(
                event,
                DomainEvent::ActionProgressed {
                    status: pushos_domain::action::ActionStatus::Failed,
                    ..
                }
            )
        })
        .await;

    // The pipeline is still running: a second gesture still reaches the provider.
    harness.provider.will(ScriptedOutcome::Complete);
    harness.tap(0).await;
    harness.wait_for_calls(2).await;

    harness.stop().await;
}

#[tokio::test]
async fn every_event_from_one_gesture_shares_a_correlation() {
    let mut harness = Harness::start(CONFIG);

    harness.tap(0).await;
    let resolved = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = harness.events.recv().await.expect("the bus is alive");
            if matches!(envelope.payload, DomainEvent::BindingResolved { .. }) {
                break envelope;
            }
        }
    })
    .await
    .expect("the binding resolves");

    let progressed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = harness.events.recv().await.expect("the bus is alive");
            if matches!(envelope.payload, DomainEvent::ActionProgressed { .. }) {
                break envelope;
            }
        }
    })
    .await
    .expect("the action progresses");

    assert_eq!(resolved.correlation_id, progressed.correlation_id);
    harness.stop().await;
}

#[tokio::test]
async fn losing_the_surface_stops_the_runtime_cleanly() {
    let (surface, input) = FakePush::new();
    let shutdown = Shutdown::new();
    let runtime = Runtime::new(store(CONFIG));
    let output: Arc<dyn PushOutput> = Arc::new(surface.clone());

    let running = shutdown.clone();
    let finished = tokio::spawn(async move {
        runtime
            .run(
                Box::new(input),
                output,
                PushRenderer::new().expect("builds"),
                running,
            )
            .await;
    });

    // Unplugging ends input, which is what the runtime treats as the surface
    // going away.
    surface.disconnect().await;

    tokio::time::timeout(Duration::from_secs(5), finished)
        .await
        .expect("the runtime should stop on its own")
        .expect("the runtime task should not panic");
}

/// Polls an asynchronous condition until it holds.
async fn settle_async<F, Fut>(mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..500 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the condition never became true");
}
