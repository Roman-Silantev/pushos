//! Several actions behind one gesture.
//!
//! "Start work" is a Focus Mode shortcut, an editor, a workspace, an agent and
//! a playlist. That is one intention and six actions, and an operator should
//! press one pad rather than six.
//!
//! Nothing here knows how to do any of those things. Every step is carried out
//! through the same dispatcher a finger uses, which is the whole design: a step
//! gets the same permission check, reports the same result, and can be anything
//! any provider offers, including a sequence. What this adds is order, and an
//! honest account of how far it got.
//!
//! It is not a workflow. A workflow branches, waits for a person and survives a
//! restart halfway through. A sequence is a handful of actions that either
//! happen now or say why they did not.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use pushos_domain::action::{
    ActionContext, ActionDefinition, ActionResult, ActionStatus, DisplayIntent,
};
use pushos_domain::context::SurfaceContext;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName, SequenceId};
use pushos_domain::ports::{ActionProvider, ActionRunner, ProviderCapabilities};
use pushos_domain::sequence::{Sequence, SequenceMode};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// The namespace this provider claims.
pub const NAMESPACE: &str = pushos_domain::sequence::NAMESPACE;

/// Runs the named sequences a configuration declared.
#[derive(Debug)]
pub struct SequenceProvider {
    /// What was configured, in declaration order.
    ///
    /// Checked when the file was read, so nothing here has to ask whether a
    /// sequence terminates: one that could reach itself never became a
    /// `Sequence` in the first place.
    sequences: Vec<Sequence>,
    /// How each step is actually carried out.
    ///
    /// Weak because the dispatcher owns this provider, and going back through
    /// the dispatcher is the point: a step gets the permission check it would
    /// have got from its own pad, never a way around one.
    actions: Mutex<Weak<dyn ActionRunner>>,
}

impl SequenceProvider {
    /// Builds the provider over what a configuration declared.
    pub fn new(sequences: impl IntoIterator<Item = Sequence>) -> Self {
        Self {
            sequences: sequences.into_iter().collect(),
            actions: Mutex::new(Weak::<NoActions>::new()),
        }
    }

    /// Whether anything is configured.
    pub fn is_empty(&self) -> bool {
        self.sequences.is_empty()
    }

    /// Gives the provider the dispatcher to run steps through.
    ///
    /// Given after construction, because the dispatcher this points at contains
    /// this provider.
    pub async fn use_actions(&self, runner: &Arc<dyn ActionRunner>) {
        *self.actions.lock().await = Arc::downgrade(runner);
    }

    /// The sequence a request named.
    fn find(&self, context: &ActionContext) -> Result<&Sequence, ActionError> {
        let named = context.params().require_text("id")?;
        self.sequences
            .iter()
            .find(|run| run.id.as_str() == named)
            .ok_or_else(|| {
                ActionError::backend(
                    format!("no sequence called `{named}` is configured"),
                    ErrorClass::Validation,
                    std::io::Error::other("no such sequence"),
                )
            })
    }

    /// The dispatcher, or a failure that says why there is not one.
    async fn runner(&self) -> Result<Arc<dyn ActionRunner>, ActionError> {
        self.actions.lock().await.upgrade().ok_or_else(|| {
            // Only reachable while PushOS is stopping, when the dispatcher has
            // already gone. Reported rather than ignored, because a sequence
            // that quietly did nothing would be worse than one that said so.
            ActionError::backend(
                "sequences are not connected to anything",
                ErrorClass::ComponentFailure,
                std::io::Error::other("no dispatcher"),
            )
        })
    }

    /// Runs the steps one after another, stopping at a failure that matters.
    async fn in_order(
        &self,
        run: &Sequence,
        runner: &Arc<dyn ActionRunner>,
        surface: &SurfaceContext,
    ) -> Outcome {
        let mut outcome = Outcome::new(run);

        for (at, step) in run.steps.iter().enumerate() {
            match carry_out(runner, &step.action, surface).await {
                Ok(()) => outcome.worked(),
                Err(failure) => {
                    outcome.failed(at, &step.action, &failure);
                    if !step.optional {
                        // The later steps were written on the assumption that
                        // the earlier ones happened. Carrying on would be
                        // guessing at what the operator wanted.
                        outcome.stopped = true;
                        break;
                    }
                }
            }
        }

        outcome
    }

    /// Starts every step at once and waits for all of them.
    ///
    /// Nothing stops early, because there is no "early": by the time one has
    /// failed the others are already running. What the operator gets is the
    /// whole account rather than the first thing that went wrong.
    async fn all_at_once(
        &self,
        run: &Sequence,
        runner: &Arc<dyn ActionRunner>,
        surface: &SurfaceContext,
    ) -> Outcome {
        let results = futures_util::future::join_all(
            run.steps
                .iter()
                .map(|step| carry_out(runner, &step.action, surface)),
        )
        .await;

        let mut outcome = Outcome::new(run);
        for (at, (step, result)) in run.steps.iter().zip(results).enumerate() {
            match result {
                Ok(()) => outcome.worked(),
                Err(failure) => outcome.failed(at, &step.action, &failure),
            }
        }
        outcome
    }
}

/// Runs one step, turning anything short of success into a reason.
///
/// A step that a provider accepted but reported as failed counts as failed
/// here. A sequence saying it worked when a step said it had not would be a
/// sequence not worth pressing.
async fn carry_out(
    runner: &Arc<dyn ActionRunner>,
    action: &ActionDefinition,
    surface: &SurfaceContext,
) -> Result<(), String> {
    match runner.run(action.clone(), surface.clone()).await {
        Err(error) => Err(error.to_string()),
        Ok(result) if result.status == ActionStatus::Failed => {
            Err(result.message.unwrap_or_else(|| "failed".to_owned()))
        }
        Ok(_) => Ok(()),
    }
}

/// How far a run got, and what went wrong on the way.
#[derive(Debug)]
struct Outcome {
    name: String,
    total: usize,
    done: usize,
    /// The first failure, as the step's name and what it said.
    ///
    /// The first rather than all of them, because the panel has one line and
    /// the first is the one that explains the rest.
    first_failure: Option<(String, String)>,
    failures: usize,
    stopped: bool,
}

impl Outcome {
    fn new(run: &Sequence) -> Self {
        Self {
            name: run.name.clone(),
            total: run.steps.len(),
            done: 0,
            first_failure: None,
            failures: 0,
            stopped: false,
        }
    }

    fn worked(&mut self) {
        self.done += 1;
    }

    fn failed(&mut self, at: usize, action: &ActionDefinition, why: &str) {
        let named = action.selector.to_string();
        warn!(step = at + 1, action = %named, %why, "a sequence step failed");
        self.failures += 1;
        if self.first_failure.is_none() {
            self.first_failure = Some((named, why.to_owned()));
        }
    }

    /// Turns the account into what the dispatcher and the panel are told.
    fn into_result(self) -> ActionResult {
        let Some((step, why)) = self.first_failure else {
            info!(sequence = %self.name, steps = self.total, "sequence finished");
            return ActionResult {
                status: ActionStatus::Completed,
                message: Some(format!("{}: {} of {}", self.name, self.done, self.total)),
                display: Some(DisplayIntent::Toast {
                    title: self.name,
                    detail: Some(format!("{} steps", self.total)),
                }),
            };
        };

        // Everything that did happen is worth saying, because the operator has
        // to know what state the machine is in before they press anything else.
        let how_far = if self.stopped {
            format!("stopped after {} of {}", self.done, self.total)
        } else {
            format!("{} of {} done", self.done, self.total)
        };

        ActionResult {
            status: ActionStatus::Failed,
            message: Some(format!("{}: {how_far}", self.name)),
            display: Some(DisplayIntent::Report {
                kind: how_far,
                title: self.name,
                lines: vec![format!("{step}: {why}")],
            }),
        }
    }
}

#[async_trait]
impl ActionProvider for SequenceProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Nothing is required to run a sequence. Each step is dispatched on its
        // own and is checked against what the operator granted, so a sequence
        // needing permission to run would be asking twice, and a sequence
        // holding permissions of its own would be a way to launder them.
        ProviderCapabilities::new(["run", "list"].map(ActionVerb::new))
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "run" => {
                let run = self.find(&context)?;
                let runner = self.runner().await?;

                info!(
                    sequence = %run.id,
                    steps = run.steps.len(),
                    mode = %run.mode,
                    "running a sequence"
                );

                let outcome = match run.mode {
                    SequenceMode::Sequential => self.in_order(run, &runner, &context.surface).await,
                    SequenceMode::Parallel => {
                        self.all_at_once(run, &runner, &context.surface).await
                    }
                };
                Ok(outcome.into_result())
            }

            "list" => {
                let lines: Vec<String> = self
                    .sequences
                    .iter()
                    .map(|run| format!("{} — {} steps", run.name, run.steps.len()))
                    .collect();

                Ok(ActionResult {
                    status: ActionStatus::Completed,
                    message: Some(format!("{} configured", lines.len())),
                    display: Some(DisplayIntent::Report {
                        kind: "sequences".to_owned(),
                        title: format!("{} configured", lines.len()),
                        lines,
                    }),
                })
            }

            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Stands in until the dispatcher exists, and is never called.
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

/// Builds the action that runs a sequence, for anything that has to name one.
pub fn run_action(id: &SequenceId) -> ActionDefinition {
    let mut params = pushos_domain::action::Params::new();
    params.set("id", id.as_str());
    ActionDefinition::new(
        pushos_domain::action::ActionSelector::new(NAMESPACE, pushos_domain::sequence::RUN),
        params,
    )
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionSelector, ParamValue, Params};
    use pushos_domain::ids::CorrelationId;
    use pushos_domain::sequence::Step;
    use pushos_testkit::FakeActions;

    use super::*;

    fn step(action: &str) -> Step {
        Step::new(ActionDefinition::bare(
            action.parse::<ActionSelector>().expect("well-formed"),
        ))
    }

    fn sequence(id: &str, mode: SequenceMode, steps: Vec<Step>) -> Sequence {
        Sequence {
            id: SequenceId::new(id),
            name: format!("The {id} sequence"),
            description: None,
            mode,
            steps,
        }
    }

    fn start_work(mode: SequenceMode) -> Sequence {
        sequence(
            "start_work",
            mode,
            vec![
                step("shortcut.run"),
                step("app.open"),
                step("media.play_pause"),
            ],
        )
    }

    /// A provider with a runner attached, and the runner held alive.
    struct Rig {
        provider: SequenceProvider,
        actions: FakeActions,
        _runner: Arc<dyn ActionRunner>,
    }

    impl Rig {
        async fn holding(sequences: impl IntoIterator<Item = Sequence>) -> Self {
            let provider = SequenceProvider::new(sequences);
            let actions = FakeActions::new();
            let runner: Arc<dyn ActionRunner> = Arc::new(actions.clone());
            provider.use_actions(&runner).await;
            Self {
                provider,
                actions,
                _runner: runner,
            }
        }

        async fn run(&self, id: &str) -> Result<ActionResult, ActionError> {
            let mut params = Params::new();
            params.set("id", ParamValue::Text(id.into()));
            self.provider
                .execute(ActionContext::new(
                    ActionDefinition::new(ActionSelector::new(NAMESPACE, "run"), params),
                    CorrelationId::generate(),
                    SurfaceContext::empty(),
                ))
                .await
        }
    }

    #[tokio::test]
    async fn one_press_carries_out_every_step_in_the_order_it_was_written() {
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        let result = rig.run("start_work").await.expect("all of it works");

        assert_eq!(
            rig.actions.selectors(),
            ["shortcut.run", "app.open", "media.play_pause"]
        );
        assert_eq!(result.status, ActionStatus::Completed);
    }

    #[tokio::test]
    async fn a_step_that_fails_stops_the_ones_that_come_after_it() {
        // The later steps were written on the assumption the earlier ones
        // happened. Carrying on would be guessing.
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        rig.actions.refuse("app.open");

        let result = rig.run("start_work").await.expect("the run is reported");

        assert_eq!(rig.actions.selectors(), ["shortcut.run"]);
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(
            result.message.is_some_and(|said| said.contains("1 of 3")),
            "the operator has to know what did happen"
        );
    }

    #[tokio::test]
    async fn a_step_that_ran_and_did_not_work_counts_as_a_failure() {
        // A provider that accepted the request and then reported that it
        // failed is not a step that happened.
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        rig.actions.report_failure("app.open");

        let result = rig.run("start_work").await.expect("the run is reported");
        assert_eq!(result.status, ActionStatus::Failed);
        assert_eq!(
            rig.actions.selectors(),
            ["shortcut.run", "app.open"],
            "it ran, it just did not work"
        );
    }

    #[tokio::test]
    async fn an_optional_step_that_fails_does_not_stop_the_rest() {
        let mut run = start_work(SequenceMode::Sequential);
        run.steps[1] = run.steps[1].clone().optional();
        let rig = Rig::holding([run]).await;
        rig.actions.refuse("app.open");

        let result = rig.run("start_work").await.expect("the run is reported");

        assert_eq!(
            rig.actions.selectors(),
            ["shortcut.run", "media.play_pause"],
            "the nicety was skipped and the rest went on"
        );
        assert_eq!(
            result.status,
            ActionStatus::Failed,
            "still worth saying that something did not happen"
        );
    }

    #[tokio::test]
    async fn every_step_runs_when_they_have_nothing_to_do_with_each_other() {
        let rig = Rig::holding([start_work(SequenceMode::Parallel)]).await;
        rig.actions.refuse("app.open");

        let result = rig.run("start_work").await.expect("the run is reported");

        let mut ran = rig.actions.selectors();
        ran.sort();
        assert_eq!(
            ran,
            ["media.play_pause", "shortcut.run"],
            "nothing stops early when everything already started"
        );
        assert_eq!(result.status, ActionStatus::Failed);
        assert!(
            result.message.is_some_and(|said| said.contains("2 of 3")),
            "and it says how much of it worked"
        );
    }

    #[tokio::test]
    async fn a_sequence_may_run_another_one() {
        // Through the dispatcher, like anything else. Configuration has
        // already proved this one cannot come back round to itself.
        let rig = Rig::holding([
            sequence(
                "start_work",
                SequenceMode::Sequential,
                vec![
                    Step::new(run_action(&SequenceId::new("focus"))),
                    step("app.open"),
                ],
            ),
            sequence(
                "focus",
                SequenceMode::Sequential,
                vec![step("shortcut.run")],
            ),
        ])
        .await;

        rig.run("start_work").await.expect("both of them work");
        assert_eq!(
            rig.actions.selectors(),
            ["sequence.run", "app.open"],
            "the inner one goes back through the dispatcher"
        );
    }

    #[tokio::test]
    async fn a_sequence_nothing_declared_is_refused_rather_than_ignored() {
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        let error = rig
            .run("nowhere")
            .await
            .expect_err("there is no such thing");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(rig.actions.ran().is_empty());
    }

    #[tokio::test]
    async fn a_request_with_no_sequence_named_is_refused() {
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        let error = rig
            .provider
            .execute(ActionContext::new(
                ActionDefinition::bare(ActionSelector::new(NAMESPACE, "run")),
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
            .expect_err("nothing said which one");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_sequence_holds_no_permissions_of_its_own() {
        // Each step is dispatched separately and checked against what the
        // operator granted. A sequence that required something would be asking
        // twice; one that held something would be a way to launder it.
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        let capabilities = rig.provider.capabilities();

        for verb in ["run", "list"] {
            assert!(
                capabilities.required_for(&ActionVerb::new(verb)).is_empty(),
                "`{verb}` grants nothing of its own"
            );
        }
    }

    #[tokio::test]
    async fn what_is_configured_can_be_listed_without_running_any_of_it() {
        let rig = Rig::holding([start_work(SequenceMode::Sequential)]).await;
        let result = rig
            .provider
            .execute(ActionContext::new(
                ActionDefinition::bare(ActionSelector::new(NAMESPACE, "list")),
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
            .expect("listing works");

        let Some(DisplayIntent::Report { lines, .. }) = result.display else {
            panic!("a report");
        };
        assert_eq!(lines, ["The start_work sequence — 3 steps"]);
        assert!(rig.actions.ran().is_empty(), "listing runs nothing");
    }

    #[tokio::test]
    async fn a_sequence_with_nothing_to_run_it_says_so() {
        // Only reachable while PushOS is stopping. Saying so beats reporting
        // that six things happened when none did.
        let provider = SequenceProvider::new([start_work(SequenceMode::Sequential)]);
        let mut params = Params::new();
        params.set("id", ParamValue::Text("start_work".into()));

        let error = provider
            .execute(ActionContext::new(
                ActionDefinition::new(ActionSelector::new(NAMESPACE, "run"), params),
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
            .expect_err("there is no dispatcher");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }
}
