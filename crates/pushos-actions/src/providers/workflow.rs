//! Starting and steering workflows from the surface.
//!
//! A pad starts one and returns immediately. A build takes minutes and the
//! surface must stay usable throughout, so nothing here waits for a run.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, DisplayIntent};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName, RunId, WorkflowId};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ProviderCapabilities};
use pushos_domain::run::Run;
use pushos_workflows::{WorkflowEngine, WorkflowError};
use tracing::info;

/// The namespace this provider claims.
pub const NAMESPACE: &str = "workflow";

/// Turns gestures into workflow operations.
#[derive(Debug)]
pub struct WorkflowProvider {
    engine: Arc<WorkflowEngine>,
}

impl WorkflowProvider {
    /// Builds the provider over the engine.
    pub fn new(engine: Arc<WorkflowEngine>) -> Self {
        Self { engine }
    }

    /// Reads the workflow a binding names.
    fn workflow(context: &ActionContext) -> Result<WorkflowId, ActionError> {
        let named = context.params().require_text("target")?;
        if named.trim().is_empty() {
            return Err(invalid("a workflow action needs a workflow to run"));
        }
        Ok(WorkflowId::new(named.trim()))
    }

    /// The run an action acts on.
    ///
    /// Named exactly, or left out to mean the one that most recently needed
    /// something. One approve pad then serves every workflow, the way one
    /// serves every agent.
    async fn run(&self, context: &ActionContext) -> Result<Run, ActionError> {
        if let Some(named) = context.params().text("run") {
            let id = named
                .parse::<RunId>()
                .map_err(|_| invalid("that is not a run identifier"))?;
            return self
                .engine
                .runs()
                .await
                .into_iter()
                .find(|run| run.id == id)
                .ok_or_else(|| invalid("no run by that name is going"));
        }

        // The one waiting on a person first, then whatever is still going.
        let runs = self.engine.runs().await;
        runs.iter()
            .find(|run| run.is_waiting_on_a_person())
            .or_else(|| runs.iter().find(|run| run.is_live()))
            .cloned()
            .ok_or_else(|| invalid("nothing is running"))
    }
}

#[async_trait]
impl ActionProvider for WorkflowProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(["start", "cancel", "approve", "reject"].map(ActionVerb::new))
            // A workflow runs agents and commands on the operator's behalf. Every
            // step it takes is permission-checked again as it happens, but starting
            // one is itself a decision to let it.
            .verb_requiring(ActionVerb::new("start"), [Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "start" => {
                let workflow = Self::workflow(&context)?;
                info!(%workflow, "starting a workflow");

                let run = self
                    .engine
                    .start(&workflow, &context.surface)
                    .await
                    .map_err(into_action_error)?;

                Ok(ActionResult {
                    status: ActionStatus::Started,
                    message: Some(format!("{workflow} started")),
                    // Starting one shows nothing on screen by itself, so the
                    // action is what tells the operator it began.
                    display: Some(DisplayIntent::Toast {
                        title: workflow.to_string(),
                        detail: Some(run.describe()),
                    }),
                })
            }

            "cancel" => {
                let run = self.run(&context).await?;
                let stopped = self
                    .engine
                    .cancel(&run.id)
                    .await
                    .map_err(into_action_error)?;
                Ok(report(&stopped, "stopped"))
            }

            "approve" | "reject" => {
                let allow = context.definition.selector.verb.as_str() == "approve";
                let run = self.run(&context).await?;
                let answered = self
                    .engine
                    .answer(&run.id, allow)
                    .await
                    .map_err(into_action_error)?;
                Ok(report(
                    &answered,
                    if allow { "approved" } else { "refused" },
                ))
            }

            other => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: other.to_owned(),
            }),
        }
    }
}

/// Turns a run into what the surface should say about it.
fn report(run: &Run, did: &str) -> ActionResult {
    ActionResult {
        status: if run.is_live() {
            ActionStatus::Started
        } else {
            ActionStatus::Completed
        },
        message: Some(format!("{} {did}", run.workflow)),
        display: None,
    }
}

fn invalid(reason: &'static str) -> ActionError {
    ActionError::backend(
        reason,
        ErrorClass::Validation,
        std::io::Error::other(reason),
    )
}

fn into_action_error(error: WorkflowError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::{CorrelationId, NodeId};
    use pushos_domain::workflow::{Limits, Node, NodeKind, Outcome, Workflow};
    use pushos_testkit::FakeRunStore;
    use pushos_workflows::{SilentObserver, WorkflowCatalogue};

    use super::*;

    fn asking() -> Workflow {
        let ask = NodeId::new("ask");
        let done = NodeId::new("done");
        Workflow {
            id: WorkflowId::new("ship"),
            name: "Ship".to_owned(),
            description: None,
            start: ask.clone(),
            nodes: vec![
                (
                    ask.clone(),
                    Node {
                        id: ask,
                        kind: NodeKind::Approval {
                            question: "Ship it?".to_owned(),
                            approve: done.clone(),
                            reject: done.clone(),
                        },
                    },
                ),
                (
                    done.clone(),
                    Node {
                        id: done,
                        kind: NodeKind::End {
                            outcome: Outcome::Succeeded,
                        },
                    },
                ),
            ]
            .into_iter()
            .collect(),
            limits: Limits::DEFAULT,
        }
    }

    struct Fixture {
        provider: WorkflowProvider,
        engine: Arc<WorkflowEngine>,
    }

    impl Fixture {
        fn new() -> Self {
            let engine = Arc::new(WorkflowEngine::new(
                WorkflowCatalogue::new([asking()]),
                Arc::new(FakeRunStore::new()),
                Arc::new(SilentObserver),
            ));
            Self {
                provider: WorkflowProvider::new(Arc::clone(&engine)),
                engine,
            }
        }

        async fn run(&self, verb: &str, params: Params) -> Result<ActionResult, ActionError> {
            let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
            self.provider
                .execute(ActionContext::new(
                    definition,
                    CorrelationId::generate(),
                    SurfaceContext::empty(),
                ))
                .await
        }

        /// Waits until the engine has a run matching, then returns it.
        async fn settles(&self, matching: impl Fn(&Run) -> bool) -> Run {
            for _ in 0..200 {
                if let Some(run) = self.engine.runs().await.into_iter().find(&matching) {
                    return run;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("nothing settled");
        }
    }

    fn target(name: &str) -> Params {
        let mut params = Params::new();
        params.set("target", ParamValue::Text(name.into()));
        params
    }

    #[tokio::test]
    async fn starting_a_workflow_returns_at_once_and_says_it_began() {
        // A build takes minutes; the surface has to stay usable.
        let fixture = Fixture::new();
        let result = fixture
            .run("start", target("ship"))
            .await
            .expect("the workflow is configured");

        assert_eq!(result.status, ActionStatus::Started);
        assert!(matches!(
            result.display,
            Some(DisplayIntent::Toast { ref title, .. }) if title == "ship"
        ));
    }

    #[tokio::test]
    async fn one_approve_pad_answers_whichever_run_is_asking() {
        // A binding with no run named acts on whatever needs a person, which is
        // what makes a single pad useful across every workflow.
        let fixture = Fixture::new();
        fixture
            .run("start", target("ship"))
            .await
            .expect("configured");
        fixture.settles(Run::is_waiting_on_a_person).await;

        fixture
            .run("approve", Params::new())
            .await
            .expect("something is asking");

        let finished = fixture.settles(|run| !run.is_live()).await;
        assert_eq!(finished.outcome(), Some(Outcome::Succeeded));
    }

    #[tokio::test]
    async fn stopping_a_run_ends_it() {
        let fixture = Fixture::new();
        fixture
            .run("start", target("ship"))
            .await
            .expect("configured");
        fixture.settles(Run::is_waiting_on_a_person).await;

        let result = fixture
            .run("cancel", Params::new())
            .await
            .expect("something is running");
        assert_eq!(result.status, ActionStatus::Completed);

        let stopped = fixture.settles(|run| !run.is_live()).await;
        assert_eq!(stopped.outcome(), Some(Outcome::Cancelled));
    }

    #[tokio::test]
    async fn acting_with_nothing_running_says_so() {
        let fixture = Fixture::new();
        let error = fixture
            .run("approve", Params::new())
            .await
            .expect_err("nothing is running");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn starting_a_workflow_nothing_configures_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("start", target("nowhere"))
            .await
            .expect_err("nothing configures it");
        assert!(error.to_string().contains("nowhere"), "{error}");
    }

    #[tokio::test]
    async fn starting_without_naming_a_workflow_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("start", Params::new())
            .await
            .expect_err("a workflow is needed");
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_verb_the_provider_does_not_have_is_refused() {
        let fixture = Fixture::new();
        let error = fixture
            .run("rewind", Params::new())
            .await
            .expect_err("there is no such verb");
        assert!(matches!(error, ActionError::UnknownVerb { .. }));
    }

    #[test]
    fn starting_a_workflow_needs_the_permission_its_steps_would_need() {
        // Answering one does not: saying yes to something already permitted is
        // not itself a new capability.
        let fixture = Fixture::new();
        let capabilities = fixture.provider.capabilities();

        assert_eq!(
            capabilities.required_for(&ActionVerb::new("start")),
            [Permission::ShellExecute]
        );
        assert!(
            capabilities
                .required_for(&ActionVerb::new("approve"))
                .is_empty()
        );
    }
}
