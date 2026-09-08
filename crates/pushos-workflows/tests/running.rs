//! Workflows, run.
//!
//! The specification's own acceptance workflow is here: plan, then build, then
//! tests, then review, with somewhere to go when the tests fail. And the thing
//! that makes it a workflow runtime rather than a script: it survives PushOS
//! stopping in the middle.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pushos_agents::{AgentRoster, AgentSupervisor, SilentObserver as SilentAgents};
use pushos_domain::agent::{AgentDefinition, AgentState};
use pushos_domain::context::SurfaceContext;
use pushos_domain::ids::{AgentId, NodeId, WorkflowId};
use pushos_domain::ports::TerminalStatus;
use pushos_domain::run::{RunState, Waiting};
use pushos_domain::workflow::{Check, Limits, Node, NodeKind, Outcome, Workflow};
use pushos_terminal::TerminalSupervisor;
use pushos_testkit::{
    AgentCall, FakeAgent, FakeRunStore, FakeTerminal, FixedRoot, RecordingTerminalObserver,
};
use pushos_workflows::{SilentObserver, WorkflowCatalogue, WorkflowEngine};

fn node(id: &str, kind: NodeKind) -> (NodeId, Node) {
    let id = NodeId::new(id);
    (id.clone(), Node { id, kind })
}

/// Plan, build, test, review. The specification's example, with a retry.
fn acceptance() -> Workflow {
    Workflow {
        id: WorkflowId::new("ship"),
        name: "Ship".to_owned(),
        description: None,
        start: NodeId::new("plan"),
        nodes: vec![
            node(
                "plan",
                NodeKind::Agent {
                    agent: AgentId::new("architect"),
                    prompt: "Plan the change".to_owned(),
                    next: NodeId::new("build"),
                    on_failure: None,
                },
            ),
            node(
                "build",
                NodeKind::Agent {
                    agent: AgentId::new("builder"),
                    prompt: "Implement it".to_owned(),
                    next: NodeId::new("tests"),
                    on_failure: None,
                },
            ),
            node(
                "tests",
                NodeKind::Terminal {
                    terminal: "tests".to_owned(),
                    program: "cargo".to_owned(),
                    args: vec!["test".to_owned()],
                    next: NodeId::new("review"),
                    on_failure: Some(NodeId::new("retry?")),
                },
            ),
            node(
                "retry?",
                NodeKind::Condition {
                    check: Check::Attempts { limit: 6 },
                    then: NodeId::new("build"),
                    otherwise: NodeId::new("gave_up"),
                },
            ),
            node(
                "review",
                NodeKind::Agent {
                    agent: AgentId::new("reviewer"),
                    prompt: "Review it".to_owned(),
                    next: NodeId::new("shipped"),
                    on_failure: None,
                },
            ),
            node(
                "shipped",
                NodeKind::End {
                    outcome: Outcome::Succeeded,
                },
            ),
            node(
                "gave_up",
                NodeKind::End {
                    outcome: Outcome::Failed,
                },
            ),
        ]
        .into_iter()
        .collect(),
        limits: Limits::DEFAULT,
    }
}

/// An engine over fakes, with the store kept so a second engine can read it.
struct Harness {
    engine: Arc<WorkflowEngine>,
    store: FakeRunStore,
    agent: FakeAgent,
    terminals: FakeTerminal,
}

impl Harness {
    fn new(workflow: Workflow) -> Self {
        Self::over(workflow, FakeRunStore::new())
    }

    /// Builds an engine over a store that may already hold something.
    ///
    /// Handing an existing store to a new engine is what a restart looks like
    /// from the inside.
    fn over(workflow: Workflow, store: FakeRunStore) -> Self {
        let agent = FakeAgent::new("claude");
        let mut roster = AgentRoster::new();
        for role in ["architect", "builder", "reviewer"] {
            roster.define(AgentDefinition::new(role, role));
        }
        roster.install(Arc::new(agent.clone()));

        let agents = Arc::new(AgentSupervisor::new(
            Arc::new(roster),
            Arc::new(SilentAgents),
            Arc::new(FixedRoot::new("/tmp/pushos-flow")),
        ));

        let terminals = FakeTerminal::new(Arc::new(RecordingTerminalObserver::new()));
        let supervisor = Arc::new(TerminalSupervisor::new(
            Arc::new(terminals.clone()),
            Arc::new(FixedRoot::new("/tmp/pushos-flow")),
        ));

        let engine = Arc::new(
            WorkflowEngine::new(
                WorkflowCatalogue::new([workflow]),
                Arc::new(store.clone()),
                Arc::new(SilentObserver),
            )
            .with_agents(agents)
            .with_terminals(supervisor),
        );

        Self {
            engine,
            store,
            agent,
            terminals,
        }
    }

    /// Waits until a run is where the test expects, then returns it.
    async fn settles<F>(&self, matching: F) -> pushos_domain::run::Run
    where
        F: Fn(&pushos_domain::run::Run) -> bool,
    {
        for _ in 0..400 {
            if let Some(run) = self.engine.runs().await.into_iter().find(&matching) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "the run never settled; it is at {:?}",
            self.engine.runs().await
        );
    }

    /// Tells the engine the agent it is waiting for has finished.
    async fn agent_finishes(&self, state: AgentState) {
        let run = self
            .settles(|run| matches!(run.state, RunState::Waiting(Waiting::Agent { .. })))
            .await;
        let RunState::Waiting(Waiting::Agent {
            session: Some(session),
        }) = run.state
        else {
            panic!("expected a session");
        };
        self.engine.agent_finished(&session, state).await;
    }

    /// Tells the engine the command it is waiting for has exited.
    async fn tests_exit(&self, code: i32) {
        self.settles(|run| matches!(run.state, RunState::Waiting(Waiting::Terminal { .. })))
            .await;
        self.engine
            .terminal_finished("tests", TerminalStatus::Exited { code: Some(code) })
            .await;
    }
}

#[tokio::test]
async fn the_acceptance_workflow_runs_from_end_to_end() {
    let harness = Harness::new(acceptance());

    let run = harness
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("the workflow is configured");

    harness.agent_finishes(AgentState::Completed).await; // plan
    harness.agent_finishes(AgentState::Completed).await; // build
    harness.tests_exit(0).await;
    harness.agent_finishes(AgentState::Completed).await; // review

    let finished = harness.settles(|run| !run.is_live()).await;
    assert_eq!(finished.outcome(), Some(Outcome::Succeeded));
    assert_eq!(finished.id, run.id);

    assert_eq!(
        harness.store.steps_of(&run.id),
        ["plan", "build", "tests", "review", "shipped"],
        "and every step was written down as it happened"
    );
    let asked: Vec<String> = harness
        .agent
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            AgentCall::Started { agent } => Some(agent),
            _ => None,
        })
        .collect();
    assert_eq!(asked, ["architect", "builder", "reviewer"]);
}

#[tokio::test]
async fn failing_tests_send_the_work_back_and_the_second_pass_ships() {
    // The branch is the point: a workflow that can only go forward is a script.
    let harness = Harness::new(acceptance());
    let run = harness
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("configured");

    harness.agent_finishes(AgentState::Completed).await; // plan
    harness.agent_finishes(AgentState::Completed).await; // build
    harness.tests_exit(1).await;

    harness.agent_finishes(AgentState::Completed).await; // build, again
    harness.tests_exit(0).await;
    harness.agent_finishes(AgentState::Completed).await; // review

    let finished = harness.settles(|run| !run.is_live()).await;
    assert_eq!(finished.outcome(), Some(Outcome::Succeeded));

    let steps = harness.store.steps_of(&run.id);
    assert_eq!(steps.iter().filter(|step| *step == "build").count(), 2);
    assert!(steps.contains(&"retry?".to_owned()), "{steps:?}");
}

#[tokio::test]
async fn a_run_survives_pushos_stopping_in_the_middle() {
    // The claim the phase rests on. The first engine gets as far as the tests
    // and then goes away; a second engine over the same store picks it up.
    let first = Harness::new(acceptance());
    let run = first
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("configured");

    first.agent_finishes(AgentState::Completed).await; // plan
    first.agent_finishes(AgentState::Completed).await; // build
    first
        .settles(|run| matches!(run.state, RunState::Waiting(Waiting::Terminal { .. })))
        .await;

    let kept = first.store.run(&run.id).expect("it was written down");
    assert_eq!(kept.at, NodeId::new("tests"));
    drop(first);

    // A new process, over what the old one wrote.
    let second = Harness::over(acceptance(), FakeRunStore::new());
    second.store.preload(&kept);
    second.engine.resume().await;

    // The step that was in flight is entered again, so the tests run once more.
    second.tests_exit(0).await;
    second.agent_finishes(AgentState::Completed).await; // review

    let finished = second.settles(|run| !run.is_live()).await;
    assert_eq!(finished.outcome(), Some(Outcome::Succeeded));
    assert_eq!(finished.id, run.id, "the same run, not a new one");
}

#[tokio::test]
async fn a_run_waiting_on_the_operator_is_still_waiting_after_a_restart() {
    let flow = Workflow {
        id: WorkflowId::new("ship"),
        name: "Ship".to_owned(),
        description: None,
        start: NodeId::new("ask"),
        nodes: vec![
            node(
                "ask",
                NodeKind::Approval {
                    question: "Ship it?".to_owned(),
                    approve: NodeId::new("yes"),
                    reject: NodeId::new("no"),
                },
            ),
            node(
                "yes",
                NodeKind::End {
                    outcome: Outcome::Succeeded,
                },
            ),
            node(
                "no",
                NodeKind::End {
                    outcome: Outcome::Cancelled,
                },
            ),
        ]
        .into_iter()
        .collect(),
        limits: Limits::DEFAULT,
    };

    let first = Harness::new(flow.clone());
    let run = first
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("configured");
    let asked = first
        .settles(|run| matches!(run.state, RunState::Waiting(Waiting::Approval { .. })))
        .await;

    let second = Harness::over(flow, FakeRunStore::new());
    second.store.preload(&asked);
    second.engine.resume().await;

    // Still asking, and the answer still works.
    let still = second.settles(|r| r.id == run.id).await;
    assert!(matches!(
        still.state,
        RunState::Waiting(Waiting::Approval { .. })
    ));

    second
        .engine
        .answer(&run.id, true)
        .await
        .expect("it is waiting");
    let finished = second.settles(|run| !run.is_live()).await;
    assert_eq!(finished.outcome(), Some(Outcome::Succeeded));
}

#[tokio::test]
async fn a_run_stops_when_it_is_told_to() {
    let harness = Harness::new(acceptance());
    let run = harness
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("configured");

    harness
        .settles(|run| matches!(run.state, RunState::Waiting(Waiting::Agent { .. })))
        .await;

    let stopped = harness.engine.cancel(&run.id).await.expect("it is running");
    assert_eq!(stopped.outcome(), Some(Outcome::Cancelled));
    assert_eq!(
        stopped.outcome().map(Outcome::status_color),
        Some(pushos_domain::color::StatusColor::Idle),
        "the operator getting what they asked for is not a failure"
    );
}

#[tokio::test]
async fn a_workflow_that_loops_for_ever_is_stopped_by_its_own_limits() {
    // Without this an agent that keeps failing would spend money all night.
    let looping = Workflow {
        id: WorkflowId::new("spin"),
        name: "Spin".to_owned(),
        description: None,
        start: NodeId::new("round"),
        nodes: vec![
            node(
                "round",
                NodeKind::Emit {
                    message: "again".to_owned(),
                    next: NodeId::new("round"),
                },
            ),
            node(
                "never",
                NodeKind::End {
                    outcome: Outcome::Succeeded,
                },
            ),
        ]
        .into_iter()
        .collect(),
        limits: Limits {
            max_steps: 12,
            timeout: Duration::from_secs(60),
        },
    };

    let harness = Harness::new(looping);
    harness
        .engine
        .start(&WorkflowId::new("spin"), &SurfaceContext::empty())
        .await
        .expect("configured");

    let finished = harness.settles(|run| !run.is_live()).await;
    assert_eq!(finished.outcome(), Some(Outcome::Exhausted));
    assert_eq!(finished.steps, 12);
}

#[tokio::test]
async fn starting_a_workflow_nothing_configures_is_refused() {
    let harness = Harness::new(acceptance());
    let error = harness
        .engine
        .start(&WorkflowId::new("nowhere"), &SurfaceContext::empty())
        .await
        .expect_err("nothing configures it");
    assert!(error.to_string().contains("nowhere"));
}

#[tokio::test]
async fn a_terminal_step_runs_the_program_rather_than_typing_at_a_shell() {
    // The terminal's exit status is only the command's if the terminal is the
    // command.
    let harness = Harness::new(acceptance());
    harness
        .engine
        .start(&WorkflowId::new("ship"), &SurfaceContext::empty())
        .await
        .expect("configured");

    harness.agent_finishes(AgentState::Completed).await;
    harness.agent_finishes(AgentState::Completed).await;
    harness
        .settles(|run| matches!(run.state, RunState::Waiting(Waiting::Terminal { .. })))
        .await;

    let opened = harness
        .terminals
        .open_ids()
        .into_iter()
        .find_map(|id| harness.terminals.spec(&id))
        .expect("a terminal was opened");
    assert_eq!(opened.program, "cargo");
    assert_eq!(opened.args, ["test"]);
    assert_eq!(opened.name, "tests", "and a pad can watch it by name");
}

#[tokio::test]
async fn a_run_started_in_a_project_stays_in_it() {
    let harness = Harness::new(acceptance());
    let surface = SurfaceContext::empty().in_workspace("sydclaw");

    let run = harness
        .engine
        .start(&WorkflowId::new("ship"), &surface)
        .await
        .expect("configured");

    assert_eq!(
        run.workspace.as_ref().map(ToString::to_string),
        Some("sydclaw".to_owned())
    );
    let _ = SystemTime::now();
}
