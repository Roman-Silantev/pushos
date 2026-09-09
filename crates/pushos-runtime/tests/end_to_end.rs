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

[[bindings]]
control = "pad.2"
gesture = "tap"
action = "sequence.run"
label = "Start work"
params = { id = "start_work" }

[[sequences]]
id = "start_work"
name = "Start work"

[[sequences.steps]]
action = "test.one"

[[sequences.steps]]
action = "test.two"

[[sequences.steps]]
action = "test.one"
"#;

/// A configuration directory a test can rewrite while PushOS is running.
struct ConfigDirectory(std::path::PathBuf);

impl ConfigDirectory {
    fn new(text: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pushos-e2e-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        let directory = Self(path);
        directory.write(text);
        directory
    }

    fn write(&self, text: &str) {
        std::fs::write(self.0.join("pushos.toml"), text).expect("writable");
    }
}

impl Drop for ConfigDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

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

        let held = store(text);
        // Built the way the real host builds it: from what the configuration
        // declared, and given the dispatcher afterwards by the runtime.
        let sequences = Arc::new(pushos_actions::providers::sequence::SequenceProvider::new(
            held.current().sequences.clone(),
        ));

        let runtime = Runtime::new(Arc::clone(&held))
            .with_provider(Arc::new(provider.clone()))
            .expect("the namespace is free")
            .with_provider(Arc::new(
                pushos_actions::providers::page::PageProvider::new(),
            ))
            .expect("the namespace is free")
            .with_provider(Arc::clone(&sequences) as Arc<dyn pushos_domain::ports::ActionProvider>)
            .expect("the namespace is free")
            .with_sequences(sequences);
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
async fn one_tap_carries_out_several_actions_in_order() {
    // The claim compound actions make: a gesture becomes a binding, the
    // binding becomes one action, and that action becomes several, each
    // carried out through the same dispatcher and in the order written.
    let harness = Harness::start(CONFIG);
    harness.tap(2).await;
    harness.wait_for_calls(3).await;

    assert_eq!(
        harness
            .provider
            .calls()
            .iter()
            .map(|definition| definition.selector.to_string())
            .collect::<Vec<_>>(),
        ["test.one", "test.two", "test.one"]
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_sequence_step_that_fails_stops_the_ones_after_it() {
    let harness = Harness::start(CONFIG);
    harness.provider.will(ScriptedOutcome::Fail(
        pushos_domain::error::ErrorClass::Retryable,
    ));

    harness.tap(2).await;
    settle(|| harness.provider.call_count() >= 1).await;

    // Given a moment in which the second and third would have run had
    // anything been going to run them.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        harness.provider.call_count(),
        1,
        "the rest were written on the assumption the first one happened"
    );
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

    // Three controls are bound on the home page: pads 0 and 2, and play.
    assert_eq!(lit, 3);

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

/// Hot reload is only useful if it reaches the surface. The store used to be
/// updated while the running pipeline kept resolving against the configuration
/// it started with.
#[tokio::test]
async fn reloading_configuration_changes_what_the_running_surface_does() {
    let directory = ConfigDirectory::new(CONFIG);
    let config = Arc::new(ConfigStore::load(&directory.0).expect("valid"));

    let (surface, input) = FakePush::new();
    let provider = RecordingProvider::new("test", ["one", "two"]);
    let shutdown = Shutdown::new();

    let runtime = Runtime::new(Arc::clone(&config))
        .with_provider(Arc::new(provider.clone()))
        .expect("free")
        .with_provider(Arc::new(
            pushos_actions::providers::page::PageProvider::new(),
        ))
        .expect("free");
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

    // Pad 0 runs `test.one` under the configuration PushOS started with.
    let at = Instant::now();
    surface.press_pad(pad(0), at).await;
    surface.release_pad(pad(0), at).await;
    settle(|| provider.call_count() >= 1).await;
    assert_eq!(provider.calls()[0].selector.to_string(), "test.one");

    // Rebind it, and reload.
    directory.write(&CONFIG.replace("action = \"test.one\"", "action = \"test.two\""));
    config.reload().expect("the new configuration is valid");

    let at = Instant::now();
    surface.press_pad(pad(0), at).await;
    surface.release_pad(pad(0), at).await;
    settle(|| provider.call_count() >= 2).await;

    assert_eq!(
        provider.calls()[1].selector.to_string(),
        "test.two",
        "the running surface kept using the configuration it started with"
    );

    shutdown.stop().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), finished).await;
}

/// The waiting screen is the only thing in PushOS that redraws on a timer, and
/// it has to stop on its own. An animation that never ends would keep an idle
/// machine awake for nothing.
#[tokio::test]
async fn the_startup_animation_runs_and_then_stops() {
    let harness = Harness::start(CONFIG);

    // While the splash is up the display is redrawn repeatedly.
    settle_async(|| async { harness.surface.state().await.frame_count() > 5 }).await;
    let during = harness.surface.state().await.frame_count();

    // Well past the splash's lifetime, drawing has stopped.
    tokio::time::sleep(Duration::from_millis(3_200)).await;
    let after = harness.surface.state().await.frame_count();

    tokio::time::sleep(Duration::from_millis(400)).await;
    let later = harness.surface.state().await.frame_count();

    assert!(after > during, "the animation should have kept drawing");
    assert_eq!(later, after, "a still screen must not redraw on a timer");

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
