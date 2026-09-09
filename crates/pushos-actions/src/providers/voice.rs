//! Speaking to PushOS.
//!
//! Two bindings on one control make push to talk: `press` starts listening and
//! `release` stops it. There is no wake word, and there will not be one. The
//! operator's finger is what decides PushOS is listening, and they can see it
//! is down.
//!
//! What comes back either matched a phrase they wrote, in which case it runs,
//! or it did not, in which case it goes to the agent as words. Nothing here
//! guesses at a third possibility.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use pushos_domain::action::{
    ActionContext, ActionDefinition, ActionResult, ActionSelector, ActionStatus, DisplayIntent,
    ParamValue, Params,
};
use pushos_domain::context::SurfaceContext;
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ActionRunner, ProviderCapabilities, VoiceError};
use pushos_domain::voice::Routing;
use pushos_voice::{Heard, VoiceListener};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "voice";

/// The parameter the words are passed under.
///
/// `text` because that is what the actions that take words already read:
/// `agent.prompt`, `terminal.run` and `terminal.send`. A name of voice's own
/// would mean none of them could be the destination.
const WORDS: &str = "text";

/// Turns gestures into listening, and speech into actions.
#[derive(Debug)]
pub struct VoiceProvider {
    listener: Arc<VoiceListener>,
    /// What an unrecognised phrase runs, with the words as `text`.
    request: Option<ActionSelector>,
    /// How a routed phrase is actually carried out.
    ///
    /// Weak because the dispatcher owns this provider, and running through the
    /// dispatcher is the point: speech gets the same permission check a finger
    /// does, never a shortcut around it.
    actions: Mutex<Weak<dyn ActionRunner>>,
}

impl VoiceProvider {
    /// Builds the provider over a listener.
    pub fn new(listener: Arc<VoiceListener>, request: Option<ActionSelector>) -> Self {
        Self {
            listener,
            request,
            actions: Mutex::new(Weak::<NoActions>::new()),
        }
    }

    /// What is listening, for whoever has to show that it is.
    pub fn listener(&self) -> &Arc<VoiceListener> {
        &self.listener
    }

    /// Gives the provider the dispatcher to run through.
    ///
    /// Given after construction, because the dispatcher this points at contains
    /// this provider.
    pub async fn use_actions(&self, runner: &Arc<dyn ActionRunner>) {
        *self.actions.lock().await = Arc::downgrade(runner);
    }

    /// Builds the action an unrecognised phrase runs.
    ///
    /// Public so a configuration can be checked against it: an action that
    /// takes words under a different name would leave PushOS listening,
    /// hearing correctly, and then saying there was nothing to send.
    pub fn words_for(request: &ActionSelector, said: &str) -> ActionDefinition {
        let mut params = Params::new();
        params.set(WORDS, ParamValue::Text(said.into()));
        ActionDefinition::new(request.clone(), params)
    }

    /// Carries out whatever was heard.
    async fn act_on(&self, heard: Heard, surface: &SurfaceContext) -> ActionResult {
        match heard.routing {
            Routing::Nothing => ActionResult {
                status: ActionStatus::Completed,
                message: Some("nothing heard".to_owned()),
                display: None,
            },

            Routing::Command { phrase, action } => self.run(*action, &phrase, surface).await,

            // Held rather than run. Transcription is not authorisation: the
            // thing that finally does it is a press.
            Routing::NeedsConfirming { phrase, .. } => ActionResult {
                status: ActionStatus::Waiting,
                message: Some(format!("{phrase}?")),
                display: Some(DisplayIntent::Prompt {
                    question: phrase,
                    choices: vec!["confirm".to_owned(), "cancel".to_owned()],
                }),
            },

            Routing::Request { text } => match &self.request {
                Some(selector) => {
                    let mut params = Params::new();
                    params.set(WORDS, ParamValue::Text(text.as_str().into()));
                    self.run(
                        ActionDefinition::new(selector.clone(), params),
                        &text,
                        surface,
                    )
                    .await
                }
                // Nowhere configured to put words. Saying so beats inventing a
                // destination for them.
                None => ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(text.clone()),
                    display: Some(DisplayIntent::Toast {
                        title: text,
                        detail: Some("no voice request action is configured".to_owned()),
                    }),
                },
            },
        }
    }

    /// Runs one action through the dispatcher.
    async fn run(
        &self,
        definition: ActionDefinition,
        said: &str,
        surface: &SurfaceContext,
    ) -> ActionResult {
        let Some(runner) = self.actions.lock().await.upgrade() else {
            // Only reachable if the dispatcher is gone, which means PushOS is
            // stopping. Nothing useful is left to do about it.
            warn!(%said, "heard something with nothing left to run it");
            return ActionResult {
                status: ActionStatus::Failed,
                message: Some("voice is not connected to anything".to_owned()),
                display: None,
            };
        };

        info!(%said, action = %definition.selector, "running what was said");
        match runner.run(definition, surface.clone()).await {
            Ok(result) => ActionResult {
                status: result.status,
                message: result.message.or_else(|| Some(said.to_owned())),
                display: result.display.or_else(|| {
                    Some(DisplayIntent::Toast {
                        title: said.to_owned(),
                        detail: None,
                    })
                }),
            },
            Err(error) => {
                warn!(%error, %said, "what was said could not be carried out");
                ActionResult {
                    status: ActionStatus::Failed,
                    message: Some(error.to_string()),
                    display: Some(DisplayIntent::Toast {
                        title: said.to_owned(),
                        detail: Some(error.to_string()),
                    }),
                }
            }
        }
    }
}

#[async_trait]
impl ActionProvider for VoiceProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            ["listen", "transcribe", "cancel", "confirm"].map(ActionVerb::new),
        )
        // Only the verb that turns the microphone on. Stopping, discarding
        // and confirming are all ways of not listening, and an operator
        // must never need a permission to stop.
        .verb_requiring(ActionVerb::new("listen"), [Permission::MicrophoneListen])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "listen" => {
                self.listener.listen().await.map_err(into_action_error)?;
                Ok(ActionResult {
                    status: ActionStatus::Started,
                    message: Some("listening".to_owned()),
                    display: None,
                })
            }

            "transcribe" => {
                let heard = self
                    .listener
                    .transcribe()
                    .await
                    .map_err(into_action_error)?;
                Ok(self.act_on(heard, &context.surface).await)
            }

            "cancel" => {
                self.listener.cancel().await;
                Ok(ActionResult {
                    status: ActionStatus::Cancelled,
                    message: Some("stopped listening".to_owned()),
                    display: None,
                })
            }

            "confirm" => {
                let Some(pending) = self.listener.take_pending().await else {
                    return Ok(ActionResult {
                        status: ActionStatus::Completed,
                        message: Some("nothing is waiting".to_owned()),
                        display: None,
                    });
                };
                let phrase = pending.phrase.clone();
                Ok(self.run(pending.action, &phrase, &context.surface).await)
            }

            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Stands in for a runner before one is given, so the weak reference has a type.
#[derive(Debug)]
struct NoActions;

#[async_trait]
impl ActionRunner for NoActions {
    async fn run(
        &self,
        _definition: ActionDefinition,
        _surface: SurfaceContext,
    ) -> Result<ActionResult, ActionError> {
        unreachable!("the placeholder runner is never upgraded")
    }
}

fn into_action_error(error: VoiceError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pushos_domain::ids::CorrelationId;
    use pushos_domain::voice::SpokenCommand;
    use pushos_testkit::{FakeActions, FakeMicrophone, FakeTranscriber, speech};
    use pushos_voice::VoiceRouter;

    use super::*;

    fn definition(action: &str) -> ActionDefinition {
        ActionDefinition::new(
            action.parse::<ActionSelector>().expect("a valid selector"),
            Params::new(),
        )
    }

    fn command(phrase: &str, action: &str, confirm: bool) -> SpokenCommand {
        SpokenCommand {
            phrase: phrase.to_owned(),
            action: definition(action),
            confirm,
        }
    }

    fn context(verb: &str) -> ActionContext {
        ActionContext::new(
            definition(&format!("voice.{verb}")),
            CorrelationId::generate(),
            SurfaceContext::default(),
        )
    }

    /// A provider wired to a microphone, an engine and a recording dispatcher.
    struct Rig {
        provider: Arc<VoiceProvider>,
        microphone: Arc<FakeMicrophone>,
        transcriber: Arc<FakeTranscriber>,
        actions: FakeActions,
        /// Held so the weak reference inside the provider stays upgradable.
        _runner: Arc<dyn ActionRunner>,
    }

    impl Rig {
        async fn new(commands: Vec<SpokenCommand>, request: Option<&str>) -> Self {
            let microphone = Arc::new(FakeMicrophone::new());
            let transcriber = Arc::new(FakeTranscriber::new());
            let listener = Arc::new(VoiceListener::new(
                Arc::clone(&microphone) as Arc<_>,
                Arc::clone(&transcriber) as Arc<_>,
                VoiceRouter::new(commands),
            ));

            let request = request.map(|written| {
                written
                    .parse::<ActionSelector>()
                    .expect("a valid request selector")
            });
            let provider = Arc::new(VoiceProvider::new(listener, request));

            let actions = FakeActions::new();
            let runner: Arc<dyn ActionRunner> = Arc::new(actions.clone());
            provider.use_actions(&runner).await;

            Self {
                provider,
                microphone,
                transcriber,
                actions,
                _runner: runner,
            }
        }

        /// Holds the control, says something, and lets go.
        async fn says(&self, words: &str) -> ActionResult {
            self.microphone.will_hear(speech(Duration::from_secs(1)));
            self.transcriber.will_say(words);
            self.provider
                .execute(context("listen"))
                .await
                .expect("listening starts");
            self.provider
                .execute(context("transcribe"))
                .await
                .expect("transcribing finishes")
        }
    }

    #[tokio::test]
    async fn a_configured_phrase_runs_its_action() {
        let rig = Rig::new(vec![command("stop", "agent.stop", false)], None).await;

        let result = rig.says("Stop.").await;
        assert_eq!(rig.actions.selectors(), ["agent.stop"]);
        assert_eq!(result.status, ActionStatus::Completed);
    }

    #[tokio::test]
    async fn anything_else_goes_to_the_configured_request_action_as_words() {
        let rig = Rig::new(Vec::new(), Some("agent.prompt")).await;

        rig.says("look at the failing test").await;
        let ran = rig.actions.ran();
        assert_eq!(ran.len(), 1);
        assert_eq!(
            ran[0].params.text(WORDS),
            Some("look at the failing test"),
            "the agent has to receive what was actually said"
        );
    }

    #[tokio::test]
    async fn with_nowhere_to_send_words_nothing_is_invented() {
        let rig = Rig::new(Vec::new(), None).await;

        let result = rig.says("carry on").await;
        assert!(
            rig.actions.ran().is_empty(),
            "PushOS must not choose a destination the operator did not"
        );
        assert!(result.display.is_some(), "but it has to say so");
    }

    #[tokio::test]
    async fn a_phrase_needing_confirmation_does_not_run_on_being_said() {
        let rig = Rig::new(vec![command("deploy", "workflow.start", true)], None).await;

        let result = rig.says("deploy").await;
        assert_eq!(result.status, ActionStatus::Waiting);
        assert!(
            rig.actions.ran().is_empty(),
            "transcription is not authorisation"
        );

        rig.provider
            .execute(context("confirm"))
            .await
            .expect("confirming works");
        assert_eq!(rig.actions.selectors(), ["workflow.start"]);
    }

    #[tokio::test]
    async fn one_press_confirms_it_once() {
        let rig = Rig::new(vec![command("deploy", "workflow.start", true)], None).await;
        rig.says("deploy").await;

        for _ in 0..3 {
            rig.provider
                .execute(context("confirm"))
                .await
                .expect("confirming works");
        }
        assert_eq!(
            rig.actions.selectors().len(),
            1,
            "a leaning finger must not deploy three times"
        );
    }

    #[tokio::test]
    async fn cancelling_throws_away_what_was_waiting() {
        let rig = Rig::new(vec![command("deploy", "workflow.start", true)], None).await;
        rig.says("deploy").await;

        rig.provider
            .execute(context("cancel"))
            .await
            .expect("cancelling works");
        rig.provider
            .execute(context("confirm"))
            .await
            .expect("confirming works");

        assert!(
            rig.actions.ran().is_empty(),
            "what was cancelled must not still be able to run"
        );
    }

    #[tokio::test]
    async fn a_control_held_but_not_spoken_into_does_nothing_and_says_nothing() {
        let rig = Rig::new(vec![command("stop", "agent.stop", false)], None).await;
        rig.microphone.will_hear_nothing();

        rig.provider
            .execute(context("listen"))
            .await
            .expect("listening starts");
        let result = rig
            .provider
            .execute(context("transcribe"))
            .await
            .expect("a mis-press is not a fault");

        assert_eq!(result.status, ActionStatus::Completed);
        assert!(result.display.is_none(), "a mis-press earns no banner");
        assert!(rig.actions.ran().is_empty());
    }

    #[tokio::test]
    async fn a_refused_microphone_is_reported_as_something_the_operator_can_fix() {
        let rig = Rig::new(Vec::new(), None).await;
        rig.microphone.refuse();

        let error = rig
            .provider
            .execute(context("listen"))
            .await
            .expect_err("the microphone was refused");
        assert_eq!(error.class(), pushos_domain::error::ErrorClass::Permission);
    }

    #[tokio::test]
    async fn only_turning_the_microphone_on_needs_permission() {
        // An operator must never need a permission in order to stop.
        let capabilities = Rig::new(Vec::new(), None).await.provider.capabilities();
        assert_eq!(
            capabilities.required_for(&ActionVerb::new("listen")),
            [Permission::MicrophoneListen]
        );
        assert!(
            capabilities
                .required_for(&ActionVerb::new("cancel"))
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_action_that_fails_is_reported_rather_than_swallowed() {
        let rig = Rig::new(vec![command("stop", "agent.stop", false)], None).await;
        rig.actions.refuse_everything();

        let result = rig.says("stop").await;
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(result.display.is_some());
    }

    #[tokio::test]
    async fn an_unknown_verb_is_refused_by_name() {
        let rig = Rig::new(Vec::new(), None).await;
        let error = rig
            .provider
            .execute(context("shout"))
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }
}
