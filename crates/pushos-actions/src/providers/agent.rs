//! Driving agents from the surface.
//!
//! The provider translates a gesture into one supervisor call and nothing more.
//! It holds no session state, knows no provider names, and cannot start work
//! the operator did not ask for.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_agents::AgentSupervisor;
use pushos_agents::Session;
use pushos_domain::action::{ActionContext, ActionResult, ActionStatus, DisplayIntent};
use pushos_domain::agent::AgentTarget;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, ProviderCapabilities};
use tracing::info;

/// The namespace this provider claims.
pub const NAMESPACE: &str = "agent";

/// Turns gestures into agent operations.
#[derive(Debug)]
pub struct AgentProvider {
    supervisor: Arc<AgentSupervisor>,
}

impl AgentProvider {
    /// Builds the provider over a supervisor.
    pub fn new(supervisor: Arc<AgentSupervisor>) -> Self {
        Self { supervisor }
    }

    /// Reads the target a binding names.
    ///
    /// An absent target means the selected session, which is what makes a
    /// single "approve" pad useful across every agent on the surface.
    fn target(context: &ActionContext) -> Result<AgentTarget, ActionError> {
        let written = context.params().text("target").unwrap_or("selected");
        written
            .parse()
            .map_err(|error: pushos_domain::agent::MalformedTarget| {
                ActionError::backend(error.to_string(), ErrorClass::Validation, error)
            })
    }

    /// The words to send. A prompt with nothing to say is refused rather than
    /// waking an agent for no reason.
    fn text(context: &ActionContext) -> Result<&str, ActionError> {
        let text = context.params().require_text("text")?;
        if text.trim().is_empty() {
            return Err(ActionError::backend(
                "there is nothing to send",
                ErrorClass::Validation,
                std::io::Error::other("empty prompt"),
            ));
        }
        Ok(text)
    }
}

#[async_trait]
impl ActionProvider for AgentProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            [
                "start", "prompt", "cancel", "approve", "reject", "select", "stop",
            ]
            .map(ActionVerb::new),
        )
        // Agents run commands and change files on the operator's behalf, so the
        // namespace as a whole is gated rather than each verb separately.
        .requiring([Permission::ShellExecute])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        let target = Self::target(&context)?;
        let verb = context.definition.selector.verb.to_string();

        let session = match verb.as_str() {
            "start" => self.supervisor.resolve(&target).await,
            "prompt" => {
                let text = Self::text(&context)?;
                info!(%target, "prompting an agent");
                self.supervisor.prompt(&target, text).await
            }
            "cancel" => self.supervisor.cancel(&target).await,
            "approve" => self.supervisor.answer(&target, true).await,
            "reject" => self.supervisor.answer(&target, false).await,
            "stop" => self.supervisor.stop(&target).await,
            "select" => {
                let session = self.supervisor.resolve(&target).await;
                if let Ok(session) = &session {
                    self.supervisor.select(&session.id).await;
                }
                session
            }
            other => {
                return Err(ActionError::UnknownVerb {
                    provider: self.name(),
                    verb: other.to_owned(),
                });
            }
        };

        let session = session.map_err(into_action_error)?;
        Ok(describe(&session, &verb))
    }
}

/// Turns the outcome into something the surface can show.
fn describe(session: &Session, verb: &str) -> ActionResult {
    let headline = format!("{} {}", session.agent, session.state);

    // A session waiting on a decision is the one case that earns the display.
    // An agent working quietly gets a line, not a takeover.
    if let Some(approval) = &session.pending_approval {
        return ActionResult {
            status: ActionStatus::Waiting,
            message: Some(headline),
            display: Some(DisplayIntent::Prompt {
                question: approval.question.clone(),
                choices: approval
                    .options
                    .iter()
                    .map(|option| option.label.clone())
                    .collect(),
            }),
        };
    }

    let status = if session.state.is_busy() {
        ActionStatus::Started
    } else if session.state == pushos_domain::agent::AgentState::Failed {
        ActionStatus::Failed
    } else {
        ActionStatus::Completed
    };

    let message = match (verb, session.last_message.as_deref()) {
        ("stop", _) => format!("{} stopped", session.agent),
        (_, Some(said)) => format!("{headline}: {said}"),
        (_, None) => headline,
    };

    ActionResult {
        status,
        message: Some(message),
        display: None,
    }
}

/// Keeps an agent failure's classification as it crosses into the action layer.
fn into_action_error(error: pushos_domain::ports::AgentError) -> ActionError {
    let class = error.class();
    ActionError::backend(error.to_string(), class, error)
}

#[cfg(test)]
mod tests {
    use pushos_agents::{AgentRoster, SilentObserver};
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::agent::{AgentDefinition, AgentState};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::ids::CorrelationId;
    use pushos_domain::ports::{AgentEvent, ApprovalId, ApprovalOption};
    use pushos_testkit::{AgentCall, FakeAgent};

    use super::*;

    fn supervisor(agent: &FakeAgent) -> Arc<AgentSupervisor> {
        let mut roster = AgentRoster::new();
        let mut builder = AgentDefinition::new("builder", "Builder");
        builder.objective = "build".to_owned();
        roster.define(builder);
        roster.install(Arc::new(agent.clone()));

        Arc::new(AgentSupervisor::new(
            Arc::new(roster),
            Arc::new(SilentObserver),
            Arc::new(pushos_testkit::FixedRoot::new("/tmp/project")),
        ))
    }

    async fn run(
        supervisor: &Arc<AgentSupervisor>,
        verb: &str,
        params: Params,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        AgentProvider::new(Arc::clone(supervisor))
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    fn targeting(target: &str) -> Params {
        let mut params = Params::new();
        params.set("target", ParamValue::from(target));
        params
    }

    #[tokio::test]
    async fn a_pad_bound_to_a_role_starts_it() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);

        let result = run(&supervisor, "start", targeting("role:builder"))
            .await
            .expect("starts");
        assert_eq!(result.status, ActionStatus::Started);
        assert_eq!(supervisor.sessions().await.len(), 1);
    }

    #[tokio::test]
    async fn a_prompt_reaches_the_provider_verbatim() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);

        let mut params = targeting("role:builder");
        params.set("text", ParamValue::from("carry on with the queue"));
        run(&supervisor, "prompt", params).await.expect("prompts");

        assert!(agent.calls().iter().any(|call| matches!(
            call,
            AgentCall::Prompted { text, .. } if text == "carry on with the queue"
        )));
    }

    #[tokio::test]
    async fn a_prompt_with_nothing_to_say_wakes_nobody() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);

        let mut params = targeting("role:builder");
        params.set("text", ParamValue::from("   "));
        let error = run(&supervisor, "prompt", params)
            .await
            .expect_err("nothing to send");

        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(
            agent.calls().is_empty(),
            "no session should have been opened"
        );
    }

    #[tokio::test]
    async fn a_binding_with_no_target_acts_on_the_selection() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);

        run(&supervisor, "start", targeting("role:builder"))
            .await
            .expect("starts");
        // No target at all: the selected session is what a single approve pad
        // needs in order to be useful across every agent.
        let result = run(&supervisor, "start", Params::new())
            .await
            .expect("resolves");
        assert_eq!(result.status, ActionStatus::Started);
        assert_eq!(
            supervisor.sessions().await.len(),
            1,
            "it reused the selection"
        );
    }

    #[tokio::test]
    async fn a_session_waiting_for_a_decision_asks_the_operator() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);
        let session = supervisor
            .resolve(&AgentTarget::role("builder"))
            .await
            .expect("starts");

        supervisor
            .record(
                &session.id,
                &AgentEvent::AskedPermission {
                    request: ApprovalId("r1".to_owned()),
                    question: "run `cargo test`?".to_owned(),
                    options: vec![ApprovalOption {
                        id: "yes".to_owned(),
                        label: "Allow".to_owned(),
                        allows: true,
                    }],
                },
            )
            .await;

        let result = run(&supervisor, "start", Params::new())
            .await
            .expect("resolves");
        assert_eq!(result.status, ActionStatus::Waiting);
        assert!(matches!(result.display, Some(DisplayIntent::Prompt { .. })));
    }

    #[tokio::test]
    async fn approving_reaches_the_provider_and_clears_the_question() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);
        let session = supervisor
            .resolve(&AgentTarget::role("builder"))
            .await
            .expect("starts");

        supervisor
            .record(
                &session.id,
                &AgentEvent::AskedPermission {
                    request: ApprovalId("r1".to_owned()),
                    question: "?".to_owned(),
                    options: vec![
                        ApprovalOption {
                            id: "yes".to_owned(),
                            label: "Allow".to_owned(),
                            allows: true,
                        },
                        ApprovalOption {
                            id: "no".to_owned(),
                            label: "Reject".to_owned(),
                            allows: false,
                        },
                    ],
                },
            )
            .await;

        run(&supervisor, "approve", Params::new())
            .await
            .expect("approves");
        assert!(agent.calls().iter().any(|call| matches!(
            call,
            AgentCall::Answered { option, .. } if option == "yes"
        )));
    }

    #[tokio::test]
    async fn a_malformed_target_is_refused_before_anything_starts() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);

        let error = run(&supervisor, "start", targeting("just-a-name"))
            .await
            .expect_err("that is not a target");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(agent.calls().is_empty());
    }

    #[tokio::test]
    async fn a_provider_failure_keeps_its_classification() {
        let agent = FakeAgent::new("claude");
        agent.fail_with(ErrorClass::Permission);
        let supervisor = supervisor(&agent);

        let error = run(&supervisor, "start", targeting("role:builder"))
            .await
            .expect_err("the provider refused");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[tokio::test]
    async fn a_failed_session_is_reported_as_a_failed_action() {
        let agent = FakeAgent::new("claude");
        let supervisor = supervisor(&agent);
        let session = supervisor
            .resolve(&AgentTarget::role("builder"))
            .await
            .expect("starts");

        supervisor
            .record(
                &session.id,
                &AgentEvent::StateChanged {
                    state: AgentState::Failed,
                },
            )
            .await;

        let result = run(
            &supervisor,
            "start",
            targeting("session:".to_owned().as_str()),
        )
        .await
        .err();
        assert!(
            result.is_some(),
            "an empty session identifier is not a target"
        );

        let mut params = Params::new();
        params.set(
            "target",
            ParamValue::from(format!("session:{}", session.id).as_str()),
        );
        let result = run(&supervisor, "start", params).await.expect("resolves");
        assert_eq!(result.status, ActionStatus::Failed);
    }

    #[test]
    fn driving_agents_is_a_declared_capability() {
        let agent = FakeAgent::new("claude");
        let provider = AgentProvider::new(supervisor(&agent));
        assert_eq!(
            provider.capabilities().required_permissions(),
            [Permission::ShellExecute]
        );
    }
}
