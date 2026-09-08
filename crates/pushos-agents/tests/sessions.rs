//! Agent supervision.
//!
//! The specification's acceptance for this phase is that three separate Push
//! controls can operate three concurrent agent sessions. That is what most of
//! these check, through the same port the real adapter implements.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use pushos_agents::{AgentRoster, AgentSupervisor};
use pushos_domain::agent::{AgentDefinition, AgentState, AgentTarget};
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::{ProviderName, SessionId};
use pushos_domain::ports::{AgentCapabilities, AgentEvent, AgentObserver, StopReason};
use pushos_testkit::{AgentCall, FakeAgent};

/// Collects everything reported, and feeds it back into the supervisor the way
/// the runtime does.
#[derive(Debug)]
struct Recorder {
    seen: Mutex<Vec<(SessionId, AgentEvent)>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }

    fn events(&self) -> Vec<(SessionId, AgentEvent)> {
        self.seen.lock().expect("readable").clone()
    }
}

impl AgentObserver for Recorder {
    fn observe(&self, session: &SessionId, event: AgentEvent) {
        self.seen
            .lock()
            .expect("writable")
            .push((session.clone(), event));
    }
}

/// A supervisor with three roles and one provider behind them.
fn supervise(roles: &[&str]) -> (Arc<AgentSupervisor>, FakeAgent, Arc<Recorder>) {
    let agent = FakeAgent::new("claude");
    let mut roster = AgentRoster::new();

    for role in roles {
        let mut definition = AgentDefinition::new(*role, *role);
        definition.objective = format!("act as the {role}");
        roster.define(definition);
    }
    roster.install(Arc::new(agent.clone()));

    let recorder = Recorder::new();
    let supervisor = Arc::new(AgentSupervisor::new(
        Arc::new(roster),
        recorder.clone(),
        "/tmp/project",
    ));
    (supervisor, agent, recorder)
}

#[tokio::test]
async fn three_controls_operate_three_concurrent_sessions() {
    let (supervisor, agent, _) = supervise(&["architect", "builder", "reviewer"]);

    // Three separate controls, each bound to a different role, pressed in turn.
    let architect = supervisor
        .prompt(&AgentTarget::role("architect"), "plan the queue rewrite")
        .await
        .expect("the architect starts and takes the prompt");
    let builder = supervisor
        .prompt(&AgentTarget::role("builder"), "implement the plan")
        .await
        .expect("the builder starts and takes the prompt");
    let reviewer = supervisor
        .prompt(&AgentTarget::role("reviewer"), "review what changed")
        .await
        .expect("the reviewer starts and takes the prompt");

    // Three distinct sessions, all live at once.
    let ids: Vec<_> = [&architect, &builder, &reviewer]
        .map(|s| s.id.to_string())
        .into();
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert_eq!(supervisor.sessions().await.len(), 3);

    // Each got its own prompt, and nobody got somebody else's.
    let prompts: Vec<_> = agent
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            AgentCall::Prompted { session, text } => Some((session, text)),
            _ => None,
        })
        .collect();
    assert_eq!(prompts.len(), 3);
    assert!(
        prompts
            .iter()
            .any(|(s, t)| s == architect.id.as_str() && t.contains("plan"))
    );
    assert!(
        prompts
            .iter()
            .any(|(s, t)| s == builder.id.as_str() && t.contains("implement"))
    );
    assert!(
        prompts
            .iter()
            .any(|(s, t)| s == reviewer.id.as_str() && t.contains("review"))
    );
}

#[tokio::test]
async fn three_sessions_report_independently_without_interfering() {
    let (supervisor, agent, _) = supervise(&["architect", "builder", "reviewer"]);

    let architect = supervisor
        .resolve(&AgentTarget::role("architect"))
        .await
        .expect("starts");
    let builder = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");
    let reviewer = supervisor
        .resolve(&AgentTarget::role("reviewer"))
        .await
        .expect("starts");

    agent.move_to(&architect.id, AgentState::Working);
    agent.ask_permission(&builder.id, "write to src/main.rs?");
    agent.finish(&reviewer.id, StopReason::Completed);

    // Feed the reports back in, as the runtime's observer does.
    for (session, event) in [
        (
            &architect.id,
            AgentEvent::StateChanged {
                state: AgentState::Working,
            },
        ),
        (
            &builder.id,
            AgentEvent::AskedPermission {
                request: pushos_domain::ports::ApprovalId(format!("{}-request", builder.id)),
                question: "write to src/main.rs?".to_owned(),
                options: vec![pushos_domain::ports::ApprovalOption {
                    id: "allow".to_owned(),
                    label: "Allow".to_owned(),
                    allows: true,
                }],
            },
        ),
        (
            &reviewer.id,
            AgentEvent::Ended {
                reason: StopReason::Completed,
            },
        ),
    ] {
        supervisor.record(session, &event).await;
    }

    let sessions = supervisor.sessions().await;
    let state_of = |id: &SessionId| {
        sessions
            .iter()
            .find(|s| s.id == *id)
            .map(|s| s.state)
            .expect("session is known")
    };

    assert_eq!(state_of(&architect.id), AgentState::Working);
    assert_eq!(state_of(&builder.id), AgentState::WaitingApproval);
    assert_eq!(state_of(&reviewer.id), AgentState::Completed);
}

#[tokio::test]
async fn a_role_that_is_already_running_is_reused_rather_than_started_again() {
    let (supervisor, agent, _) = supervise(&["builder"]);

    let first = supervisor
        .prompt(&AgentTarget::role("builder"), "one")
        .await
        .expect("starts");
    let second = supervisor
        .prompt(&AgentTarget::role("builder"), "two")
        .await
        .expect("reuses");

    assert_eq!(
        first.id, second.id,
        "a second prompt must not open a second builder"
    );
    assert_eq!(
        agent
            .calls()
            .iter()
            .filter(|c| matches!(c, AgentCall::Started { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn starting_a_session_selects_it_so_the_next_gesture_needs_no_target() {
    let (supervisor, _, _) = supervise(&["builder"]);

    let started = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");
    let selected = supervisor.selected().await.expect("something is selected");
    assert_eq!(selected.id, started.id);

    // An unqualified gesture now lands on it.
    let prompted = supervisor
        .prompt(&AgentTarget::Selected, "carry on")
        .await
        .expect("prompts");
    assert_eq!(prompted.id, started.id);
}

#[tokio::test]
async fn approving_answers_the_question_the_session_actually_asked() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    let session = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    supervisor
        .record(
            &session.id,
            &AgentEvent::AskedPermission {
                request: pushos_domain::ports::ApprovalId("r1".to_owned()),
                question: "run `cargo test`?".to_owned(),
                options: vec![
                    pushos_domain::ports::ApprovalOption {
                        id: "yes".to_owned(),
                        label: "Allow".to_owned(),
                        allows: true,
                    },
                    pushos_domain::ports::ApprovalOption {
                        id: "no".to_owned(),
                        label: "Reject".to_owned(),
                        allows: false,
                    },
                ],
            },
        )
        .await;

    supervisor
        .answer(&AgentTarget::Selected, true)
        .await
        .expect("answers");

    assert!(agent.calls().contains(&AgentCall::Answered {
        session: session.id.to_string(),
        option: "yes".to_owned(),
    }));
}

#[tokio::test]
async fn refusing_chooses_the_answer_that_refuses() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    let session = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    supervisor
        .record(
            &session.id,
            &AgentEvent::AskedPermission {
                request: pushos_domain::ports::ApprovalId("r1".to_owned()),
                question: "force push?".to_owned(),
                options: vec![
                    pushos_domain::ports::ApprovalOption {
                        id: "yes".to_owned(),
                        label: "Allow".to_owned(),
                        allows: true,
                    },
                    pushos_domain::ports::ApprovalOption {
                        id: "no".to_owned(),
                        label: "Reject".to_owned(),
                        allows: false,
                    },
                ],
            },
        )
        .await;

    supervisor
        .answer(&AgentTarget::Selected, false)
        .await
        .expect("answers");
    assert!(agent.calls().contains(&AgentCall::Answered {
        session: session.id.to_string(),
        option: "no".to_owned(),
    }));
}

#[tokio::test]
async fn answering_when_nothing_was_asked_is_refused_rather_than_sent() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    let error = supervisor
        .answer(&AgentTarget::Selected, true)
        .await
        .expect_err("nothing was asked");
    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(
        !agent
            .calls()
            .iter()
            .any(|call| matches!(call, AgentCall::Answered { .. })),
        "an answer to nothing must not reach the provider"
    );
}

#[tokio::test]
async fn cancelling_a_provider_that_cannot_be_interrupted_says_so() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    agent.set_capabilities(AgentCapabilities {
        cancel: false,
        ..AgentCapabilities::minimal()
    });
    supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    let error = supervisor
        .cancel(&AgentTarget::Selected)
        .await
        .expect_err("it cannot be interrupted");
    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(
        !agent
            .calls()
            .iter()
            .any(|call| matches!(call, AgentCall::Cancelled { .. }))
    );
}

#[tokio::test]
async fn cancelling_a_provider_that_can_be_interrupted_reaches_it() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    let session = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    supervisor
        .cancel(&AgentTarget::Selected)
        .await
        .expect("interrupts");
    assert!(agent.calls().contains(&AgentCall::Cancelled {
        session: session.id.to_string()
    }));
}

#[tokio::test]
async fn a_finished_session_makes_way_for_a_new_one_for_the_same_role() {
    let (supervisor, agent, _) = supervise(&["builder"]);

    let first = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");
    supervisor
        .record(
            &first.id,
            &AgentEvent::Ended {
                reason: StopReason::Completed,
            },
        )
        .await;

    let second = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts again");
    assert_ne!(
        first.id, second.id,
        "a finished session must not take new work"
    );
    assert_eq!(
        agent
            .calls()
            .iter()
            .filter(|c| matches!(c, AgentCall::Started { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn a_role_with_no_provider_fails_before_anything_is_started() {
    let mut roster = AgentRoster::new();
    roster.define(AgentDefinition::new("builder", "Builder"));

    let supervisor = AgentSupervisor::new(Arc::new(roster), Recorder::new(), "/tmp/project");

    let error = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect_err("nothing can run it");
    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(supervisor.sessions().await.is_empty());
}

#[tokio::test]
async fn stopping_a_session_forgets_it() {
    let (supervisor, agent, _) = supervise(&["builder"]);
    let session = supervisor
        .resolve(&AgentTarget::role("builder"))
        .await
        .expect("starts");

    supervisor
        .stop(&AgentTarget::Selected)
        .await
        .expect("stops");
    assert!(supervisor.sessions().await.is_empty());
    assert!(agent.calls().contains(&AgentCall::Stopped {
        session: session.id.to_string()
    }));
}

#[tokio::test]
async fn every_state_change_pushos_makes_is_reported() {
    let (supervisor, _, recorder) = supervise(&["builder"]);

    supervisor
        .prompt(&AgentTarget::role("builder"), "go")
        .await
        .expect("prompts");

    let states: Vec<_> = recorder
        .events()
        .into_iter()
        .filter_map(|(_, event)| match event {
            AgentEvent::StateChanged { state } => Some(state),
            _ => None,
        })
        .collect();

    assert!(
        states.contains(&AgentState::Starting),
        "opening was reported"
    );
    assert!(
        states.contains(&AgentState::Queued),
        "the prompt was reported"
    );
}
