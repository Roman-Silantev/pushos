//! Push to talk.
//!
//! A control is held, which starts listening, and released, which stops it.
//! There is no wake word and no silence detection deciding when the operator
//! has finished: their finger decides, and they can always see that it is down.
//!
//! Between those two moments PushOS is recording, and it says so on the display
//! and in the light under their finger. An operator must never be unsure
//! whether the microphone is on.

use std::sync::Arc;
use std::time::Duration;

use pushos_domain::action::ActionDefinition;
use pushos_domain::ports::{Microphone, Transcriber, VoiceError};
use pushos_domain::voice::{Listening, Routing, Utterance};
use tokio::sync::{Mutex, watch};
use tracing::{debug, info, warn};

use crate::router::VoiceRouter;

/// What the operator said, and what is to be done about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    /// Where it was sent.
    pub routing: Routing,
    /// The words, when there were any.
    pub said: Option<Utterance>,
}

/// Something spoken that will not happen without a press.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    /// The phrase that asked for it.
    pub phrase: String,
    /// What it would run.
    pub action: ActionDefinition,
}

/// Owns whether PushOS is listening, and what it heard.
#[derive(Debug)]
pub struct VoiceListener {
    microphone: Arc<dyn Microphone>,
    transcriber: Arc<dyn Transcriber>,
    router: VoiceRouter,
    /// Published rather than only held, so the display and the light under the
    /// operator's finger follow it without asking. An operator must never be
    /// unsure whether the microphone is on.
    state: watch::Sender<Listening>,
    /// Something irreversible, waiting for a finger rather than a word.
    pending: Mutex<Option<Pending>>,
}

impl VoiceListener {
    /// Builds a listener over a microphone and an engine.
    pub fn new(
        microphone: Arc<dyn Microphone>,
        transcriber: Arc<dyn Transcriber>,
        router: VoiceRouter,
    ) -> Self {
        Self {
            microphone,
            transcriber,
            router,
            state: watch::channel(Listening::Idle).0,
            pending: Mutex::new(None),
        }
    }

    /// What the engine is called, for the display and for `doctor`.
    pub fn engine(&self) -> &str {
        self.transcriber.name()
    }

    /// Whether PushOS is listening, and what it is doing.
    pub fn state(&self) -> Listening {
        *self.state.borrow()
    }

    /// Follows whether PushOS is listening.
    ///
    /// What the display and the lights watch. The latest answer is the only one
    /// that matters, so this is a watch rather than a queue.
    pub fn watch(&self) -> watch::Receiver<Listening> {
        self.state.subscribe()
    }

    /// Says where it has got to, to everything following.
    fn now(&self, state: Listening) {
        self.state.send_replace(state);
    }

    /// The phrases it knows.
    pub fn router(&self) -> &VoiceRouter {
        &self.router
    }

    /// Begins listening. Called when the control goes down.
    ///
    /// Pressing again while already listening starts over rather than failing:
    /// two presses is an operator changing their mind, not a fault.
    pub async fn listen(&self) -> Result<(), VoiceError> {
        self.microphone.start().await?;
        self.now(Listening::Recording);
        debug!(engine = self.transcriber.name(), "listening");
        Ok(())
    }

    /// Stops listening and works out what was said. Called on the release.
    ///
    /// A release with no press before it, or a control brushed rather than
    /// held, produces nothing rather than an error: neither is a fault, and
    /// reporting one would put a message on the display for a non-event.
    pub async fn transcribe(&self) -> Result<Heard, VoiceError> {
        let recorded = match self.microphone.stop().await {
            Ok(recorded) => recorded,
            Err(error) => {
                self.now(Listening::Idle);
                return Err(error);
            }
        };

        let Some(recording) = recorded else {
            self.now(Listening::Idle);
            return Ok(Heard {
                routing: Routing::Nothing,
                said: None,
            });
        };

        if recording.is_silent() {
            debug!(
                held = ?recording.duration,
                "the control was held but nothing was said"
            );
            self.now(Listening::Idle);
            return Ok(Heard {
                routing: Routing::Nothing,
                said: None,
            });
        }

        self.now(Listening::Transcribing);
        let said = self.transcriber.transcribe(&recording).await;
        self.now(Listening::Idle);

        let said = said.inspect_err(|error| {
            warn!(%error, engine = self.transcriber.name(), "could not work out what was said");
        })?;

        let routing = self.router.route(&said);
        info!(heard = %said.text, routed = %routing, "voice");

        // Held here rather than run, so that the thing which finally runs it is
        // a press. Transcription is not authorisation.
        if let Routing::NeedsConfirming { phrase, action } = &routing {
            *self.pending.lock().await = Some(Pending {
                phrase: phrase.clone(),
                action: (**action).clone(),
            });
        }

        Ok(Heard {
            routing,
            said: Some(said),
        })
    }

    /// Stops listening and throws away what was heard.
    pub async fn cancel(&self) {
        self.microphone.discard().await;
        self.now(Listening::Idle);
        self.forget().await;
    }

    /// Holds something for a press, replacing whatever was waiting.
    ///
    /// Replacing rather than refusing: saying it again is how an operator
    /// corrects themselves, and the newer words are the ones they meant.
    pub async fn hold(&self, pending: Pending) {
        *self.pending.lock().await = Some(pending);
    }

    /// What is waiting for a press, if anything is.
    pub async fn pending(&self) -> Option<Pending> {
        self.pending.lock().await.clone()
    }

    /// Takes what was waiting, so it can be run.
    ///
    /// Taken rather than read, so one press runs it once.
    pub async fn take_pending(&self) -> Option<Pending> {
        self.pending.lock().await.take()
    }

    /// Forgets what was waiting.
    pub async fn forget(&self) {
        *self.pending.lock().await = None;
    }

    /// How long a control has to be held to be worth transcribing.
    ///
    /// Shorter than this and it was a brush, not a word.
    pub const MINIMUM_HELD: Duration = Duration::from_millis(250);
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionSelector, Params};
    use pushos_domain::voice::SpokenCommand;
    use pushos_testkit::{FakeMicrophone, FakeTranscriber, speech};

    use super::*;

    fn command(phrase: &str, action: &str, confirm: bool) -> SpokenCommand {
        let selector: ActionSelector = action.parse().expect("a valid selector");
        SpokenCommand {
            phrase: phrase.to_owned(),
            action: ActionDefinition::new(selector, Params::new()),
            confirm,
        }
    }

    struct Fixture {
        listener: VoiceListener,
        microphone: FakeMicrophone,
        transcriber: FakeTranscriber,
    }

    impl Fixture {
        fn new() -> Self {
            let microphone = FakeMicrophone::new();
            let transcriber = FakeTranscriber::new();
            Self {
                listener: VoiceListener::new(
                    Arc::new(microphone.clone()),
                    Arc::new(transcriber.clone()),
                    VoiceRouter::new([
                        command("approve", "agent.approve", false),
                        command("deploy", "shortcut.run", true),
                    ]),
                ),
                microphone,
                transcriber,
            }
        }

        /// Holds the control, says something, and lets go.
        async fn says(&self, words: &str) -> Heard {
            self.transcriber.will_say(words);
            self.listener.listen().await.expect("the fake permits it");
            self.listener.transcribe().await.expect("no fault")
        }
    }

    #[tokio::test]
    async fn holding_the_control_is_what_starts_listening() {
        let fixture = Fixture::new();
        assert_eq!(fixture.listener.state(), Listening::Idle);

        fixture
            .listener
            .listen()
            .await
            .expect("the fake permits it");
        assert_eq!(fixture.listener.state(), Listening::Recording);
        assert!(fixture.microphone.is_recording().await);
    }

    #[tokio::test]
    async fn letting_go_stops_listening_whatever_happens_next() {
        // The one thing that must always be true: releasing the control turns
        // the microphone off.
        let fixture = Fixture::new();
        fixture.says("approve").await;

        assert_eq!(fixture.listener.state(), Listening::Idle);
        assert!(!fixture.microphone.is_recording().await);
    }

    #[tokio::test]
    async fn a_spoken_phrase_becomes_the_action_it_names() {
        let fixture = Fixture::new();
        let heard = fixture.says("Approve.").await;

        let Routing::Command { phrase, action } = heard.routing else {
            panic!("expected a command");
        };
        assert_eq!(phrase, "approve");
        assert_eq!(action.selector.to_string(), "agent.approve");
    }

    #[tokio::test]
    async fn anything_else_becomes_a_request() {
        let fixture = Fixture::new();
        let heard = fixture.says("Tell Claude to carry on").await;

        assert!(matches!(heard.routing, Routing::Request { .. }));
        assert_eq!(heard.said.expect("words").text, "Tell Claude to carry on");
    }

    #[tokio::test]
    async fn something_irreversible_waits_for_a_press_and_does_not_run() {
        let fixture = Fixture::new();
        let heard = fixture.says("deploy").await;

        assert!(matches!(heard.routing, Routing::NeedsConfirming { .. }));
        let waiting = fixture.listener.pending().await.expect("it is waiting");
        assert_eq!(waiting.phrase, "deploy");
    }

    #[tokio::test]
    async fn one_press_runs_what_was_waiting_once() {
        let fixture = Fixture::new();
        fixture.says("deploy").await;

        assert!(fixture.listener.take_pending().await.is_some());
        assert!(
            fixture.listener.take_pending().await.is_none(),
            "a second press must not run it again"
        );
    }

    #[tokio::test]
    async fn a_brushed_control_does_nothing_and_is_not_a_fault() {
        // Reporting this would put a message on the display for a non-event.
        let fixture = Fixture::new();
        fixture.microphone.will_hear_nothing();

        fixture.listener.listen().await.expect("permitted");
        let heard = fixture.listener.transcribe().await.expect("not a fault");

        assert_eq!(heard.routing, Routing::Nothing);
        assert!(heard.said.is_none());
        assert!(
            fixture.transcriber.heard().is_empty(),
            "and the engine was not troubled with it"
        );
    }

    #[tokio::test]
    async fn a_release_with_no_press_before_it_does_nothing() {
        let fixture = Fixture::new();
        let heard = fixture.listener.transcribe().await.expect("not a fault");
        assert_eq!(heard.routing, Routing::Nothing);
    }

    #[tokio::test]
    async fn cancelling_throws_away_what_was_heard() {
        let fixture = Fixture::new();
        fixture.listener.listen().await.expect("permitted");
        fixture.listener.cancel().await;

        assert_eq!(fixture.listener.state(), Listening::Idle);
        assert_eq!(fixture.microphone.discards(), 1);
        assert!(fixture.transcriber.heard().is_empty());
    }

    #[tokio::test]
    async fn cancelling_also_forgets_what_was_waiting_for_a_press() {
        let fixture = Fixture::new();
        fixture.says("deploy").await;
        fixture.listener.cancel().await;

        assert!(fixture.listener.pending().await.is_none());
    }

    #[tokio::test]
    async fn a_refused_microphone_leaves_pushos_not_listening() {
        // The display must not go on saying "Listening" when it is not.
        let fixture = Fixture::new();
        fixture.microphone.refuse();

        let error = fixture.listener.listen().await.expect_err("it was refused");
        assert_eq!(error.class(), pushos_domain::error::ErrorClass::Permission);
        assert_eq!(fixture.listener.state(), Listening::Idle);
    }

    #[tokio::test]
    async fn an_engine_that_fails_leaves_pushos_not_listening() {
        let fixture = Fixture::new();
        fixture.transcriber.fail();

        fixture.listener.listen().await.expect("permitted");
        assert!(fixture.listener.transcribe().await.is_err());
        assert_eq!(fixture.listener.state(), Listening::Idle);
    }

    #[tokio::test]
    async fn pressing_again_while_listening_starts_over() {
        let fixture = Fixture::new();
        fixture.listener.listen().await.expect("permitted");
        fixture.listener.listen().await.expect("permitted");

        assert_eq!(fixture.microphone.starts(), 2);
        assert_eq!(fixture.listener.state(), Listening::Recording);
    }

    #[tokio::test]
    async fn the_engine_is_given_exactly_what_the_microphone_heard() {
        let fixture = Fixture::new();
        fixture.microphone.will_hear(speech(Duration::from_secs(2)));
        fixture.says("stop").await;

        let heard = fixture.transcriber.heard();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].duration, Duration::from_secs(2));
    }
}
