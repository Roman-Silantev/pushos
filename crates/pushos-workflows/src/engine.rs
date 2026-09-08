//! Running workflows, and remembering where they got to.
//!
//! One rule holds the whole thing together: a step is written down before the
//! step after it runs. A crash therefore loses at most the step that was in
//! flight, never one already reported as taken, and what a restart reads is
//! never ahead of what actually happened.
//!
//! Steps that wait, which is most of the interesting ones, do not hold a task
//! open. The run is parked with a note saying what it is waiting for, and
//! something outside tells the engine when that thing is done. That is what
//! lets a run survive a restart: there is nothing to resume except a fact.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use pushos_agents::AgentSupervisor;
use pushos_domain::agent::{AgentState, AgentTarget};
use pushos_domain::context::SurfaceContext;
use pushos_domain::ids::{NodeId, RunId, SessionId, WorkflowId};
use pushos_domain::ports::{ActionRunner, RunStore, TerminalStatus};
use pushos_domain::run::{Run, RunState, Transition, Waiting};
use pushos_domain::terminal::TerminalTarget;
use pushos_domain::workflow::{Check, NodeKind, Outcome, Workflow};
use pushos_terminal::{OpenTerminal, TerminalSupervisor};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::catalogue::WorkflowCatalogue;

/// Told whenever a run moves, so the surface can show it.
pub trait RunObserver: Send + Sync + std::fmt::Debug {
    /// Reports where a run is now.
    fn observe(&self, run: &Run);
}

/// An observer that watches nothing.
#[derive(Debug, Clone, Copy)]
pub struct SilentObserver;

impl RunObserver for SilentObserver {
    fn observe(&self, _run: &Run) {}
}

/// Runs workflows and keeps track of where they are.
pub struct WorkflowEngine {
    catalogue: WorkflowCatalogue,
    store: Arc<dyn RunStore>,
    /// How a workflow runs anything a pad could run.
    ///
    /// Weak because the dispatcher owns the provider that owns this engine.
    /// Holding it strongly would be a cycle nothing ever breaks.
    actions: Mutex<Weak<dyn ActionRunner>>,
    agents: Option<Arc<AgentSupervisor>>,
    terminals: Option<Arc<TerminalSupervisor>>,
    observer: Arc<dyn RunObserver>,
    runs: Mutex<HashMap<RunId, Run>>,
}

impl std::fmt::Debug for WorkflowEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowEngine")
            .field("workflows", &self.catalogue.len())
            .finish_non_exhaustive()
    }
}

impl WorkflowEngine {
    /// Builds an engine over the configured workflows.
    pub fn new(
        catalogue: WorkflowCatalogue,
        store: Arc<dyn RunStore>,
        observer: Arc<dyn RunObserver>,
    ) -> Self {
        Self {
            catalogue,
            store,
            actions: Mutex::new(Weak::<NoActions>::new()),
            agents: None,
            terminals: None,
            observer,
            runs: Mutex::new(HashMap::new()),
        }
    }

    /// Lets workflows drive agents.
    #[must_use]
    pub fn with_agents(mut self, agents: Arc<AgentSupervisor>) -> Self {
        self.agents = Some(agents);
        self
    }

    /// Lets workflows run terminals.
    #[must_use]
    pub fn with_terminals(mut self, terminals: Arc<TerminalSupervisor>) -> Self {
        self.terminals = Some(terminals);
        self
    }

    /// Lets workflows run anything a pad can run.
    ///
    /// Given after construction, because the dispatcher this points at contains
    /// the provider that points back here.
    pub async fn use_actions(&self, actions: &Arc<dyn ActionRunner>) {
        *self.actions.lock().await = Arc::downgrade(actions);
    }

    /// The workflows PushOS knows about.
    pub fn catalogue(&self) -> &WorkflowCatalogue {
        &self.catalogue
    }

    /// Every run, live and recently finished.
    pub async fn runs(&self) -> Vec<Run> {
        let mut runs: Vec<Run> = self.runs.lock().await.values().cloned().collect();
        runs.sort_by_key(|run| (run.started_at, run.id.to_string()));
        runs
    }

    /// Starts a workflow.
    ///
    /// Returns as soon as the run is written down and under way; a pad must not
    /// wait for a build to finish.
    pub async fn start(
        self: &Arc<Self>,
        workflow: &WorkflowId,
        surface: &SurfaceContext,
    ) -> Result<Run, WorkflowError> {
        let definition = self
            .catalogue
            .get(workflow)
            .ok_or_else(|| WorkflowError::Unknown {
                workflow: workflow.clone(),
            })?;

        let run = Run::beginning(
            definition.id.clone(),
            definition.start.clone(),
            surface.workspace.clone(),
            SystemTime::now(),
        );

        // Written before anything runs, so a crash between here and the first
        // step leaves a run that can be picked up rather than nothing at all.
        self.remember(&run).await;
        self.store
            .append(&Transition {
                run: run.id,
                from: None,
                to: run.at.clone(),
                note: Some("started".to_owned()),
                at: run.started_at,
            })
            .await
            .unwrap_or_else(|error| warn!(%error, "a workflow step was not written down"));

        info!(%workflow, run = %run.id, "workflow started");
        self.drive(run.id);
        Ok(run)
    }

    /// Picks up every run that was in flight when PushOS stopped.
    ///
    /// A run parked on a step that waits is resumed where it was. One that was
    /// mid-step is re-entered: the step is run again, because what PushOS knows
    /// is that it started and not that it finished.
    pub async fn resume(self: &Arc<Self>) {
        let unfinished = match self.store.unfinished().await {
            Ok(runs) => runs,
            Err(error) => {
                warn!(%error, "could not read what was running");
                return;
            }
        };

        if unfinished.is_empty() {
            return;
        }
        info!(
            runs = unfinished.len(),
            "picking up workflows that were running"
        );

        for mut run in unfinished {
            if self.catalogue.get(&run.workflow).is_none() {
                // The workflow was removed from configuration while it was
                // running. Ending it is honest; leaving it is a run nothing can
                // ever advance.
                warn!(run = %run.id, workflow = %run.workflow, "the workflow is gone; ending the run");
                run.state = RunState::Finished {
                    outcome: Outcome::Failed,
                };
                run.note = Some("the workflow was removed from configuration".to_owned());
                self.remember(&run).await;
                continue;
            }

            let waiting = match &run.state {
                RunState::Waiting(waiting) => Some(waiting.clone()),
                _ => None,
            };
            let id = run.id;
            self.runs.lock().await.insert(id, run);

            match waiting {
                // Time passes whether or not PushOS is running.
                Some(Waiting::Delay { until }) => self.clone().sleep_until(id, until),
                // The operator was asked something and never answered. Ask
                // again by leaving it parked; the display will show it.
                Some(Waiting::Approval { .. }) => {}
                // Whatever it was waiting for is gone with the process, so the
                // step is entered again.
                Some(Waiting::Agent { .. } | Waiting::Terminal { .. }) | None => self.drive(id),
            }
        }
    }

    /// Stops a run.
    pub async fn cancel(&self, run: &RunId) -> Result<Run, WorkflowError> {
        let mut current = self
            .get(run)
            .await
            .ok_or(WorkflowError::NoSuchRun { run: *run })?;
        if !current.is_live() {
            return Ok(current);
        }

        current.state = RunState::Finished {
            outcome: Outcome::Cancelled,
        };
        current.note = Some("stopped".to_owned());
        current.updated_at = SystemTime::now();
        self.remember(&current).await;
        info!(run = %run, "workflow cancelled");
        Ok(current)
    }

    /// Answers the question a run is waiting on.
    pub async fn answer(self: &Arc<Self>, run: &RunId, allow: bool) -> Result<Run, WorkflowError> {
        let current = self
            .get(run)
            .await
            .ok_or(WorkflowError::NoSuchRun { run: *run })?;
        let RunState::Waiting(Waiting::Approval { .. }) = &current.state else {
            return Err(WorkflowError::NotWaiting { run: *run });
        };

        let node = self
            .node(&current)
            .ok_or(WorkflowError::NoSuchRun { run: *run })?;
        let NodeKind::Approval {
            approve, reject, ..
        } = &node
        else {
            return Err(WorkflowError::NotWaiting { run: *run });
        };

        let to = if allow {
            approve.clone()
        } else {
            reject.clone()
        };
        let answer = if allow { "approved" } else { "refused" };
        self.step_to(run, to, Some(answer.to_owned()), allow).await;
        self.drive(*run);
        self.get(run)
            .await
            .ok_or(WorkflowError::NoSuchRun { run: *run })
    }

    /// Tells the engine an agent session has stopped working.
    pub async fn agent_finished(self: &Arc<Self>, session: &SessionId, state: AgentState) {
        if state.is_busy() {
            return;
        }

        let waiting = self
            .find(|run| {
                matches!(
                    &run.state,
                    RunState::Waiting(Waiting::Agent { session: Some(held) }) if held == session
                )
            })
            .await;

        let Some(run) = waiting else { return };
        let succeeded = state == AgentState::Completed;
        self.leave_wait(&run, succeeded, format!("agent {state}"))
            .await;
    }

    /// Tells the engine a terminal has exited.
    pub async fn terminal_finished(self: &Arc<Self>, terminal: &str, status: TerminalStatus) {
        if status.is_live() {
            return;
        }

        let waiting = self
            .find(|run| {
                matches!(
                    &run.state,
                    RunState::Waiting(Waiting::Terminal { terminal: held }) if held == terminal
                )
            })
            .await;

        let Some(run) = waiting else { return };
        let succeeded = status.succeeded();
        self.leave_wait(&run, succeeded, format!("{terminal} {status:?}"))
            .await;
    }

    /// Moves a parked run on, one way or the other.
    async fn leave_wait(self: &Arc<Self>, run: &Run, succeeded: bool, note: String) {
        let Some(kind) = self.node(run) else { return };

        let (next, on_failure) = match &kind {
            NodeKind::Agent {
                next, on_failure, ..
            }
            | NodeKind::Terminal {
                next, on_failure, ..
            } => (next.clone(), on_failure.clone()),
            _ => return,
        };

        let to = if succeeded {
            next
        } else {
            on_failure.unwrap_or(next)
        };
        self.step_to(&run.id, to, Some(note), succeeded).await;
        self.drive(run.id);
    }

    /// Runs a run forward on its own task.
    fn drive(self: &Arc<Self>, id: RunId) {
        let engine = Arc::clone(self);
        tokio::spawn(async move { engine.advance(id).await });
    }

    /// Takes steps until the run waits, finishes, or runs out.
    async fn advance(self: Arc<Self>, id: RunId) {
        loop {
            let Some(run) = self.get(&id).await else {
                return;
            };
            if !run.is_live() {
                return;
            }

            let Some(workflow) = self.catalogue.get(&run.workflow).cloned() else {
                self.finish(&id, Outcome::Failed, "the workflow is gone")
                    .await;
                return;
            };

            if let Some(reason) = exhausted(&run, &workflow) {
                self.finish(&id, Outcome::Exhausted, reason).await;
                return;
            }

            let Some(node) = workflow.node(&run.at).map(|node| node.kind.clone()) else {
                self.finish(&id, Outcome::Failed, "the step is not in the workflow")
                    .await;
                return;
            };

            match self.execute(&run, &node).await {
                Step::Go {
                    to,
                    note,
                    succeeded,
                } => self.step_to(&id, to, note, succeeded).await,
                Step::Wait(waiting, saying) => {
                    self.park(&id, waiting, saying).await;
                    return;
                }
                Step::Finish(outcome, why) => {
                    self.finish(&id, outcome, &why).await;
                    return;
                }
            }
        }
    }

    /// Carries out one step.
    async fn execute(self: &Arc<Self>, run: &Run, node: &NodeKind) -> Step {
        match node {
            NodeKind::Emit { message, next } => {
                info!(run = %run.id, message, "workflow says");
                Step::Go {
                    to: next.clone(),
                    note: Some(message.clone()),
                    succeeded: true,
                }
            }

            NodeKind::End { outcome } => Step::Finish(*outcome, outcome.to_string()),

            NodeKind::Condition {
                check,
                then,
                otherwise,
            } => {
                let holds = match check {
                    Check::Succeeded => run.last_succeeded,
                    Check::Failed => !run.last_succeeded,
                    Check::Attempts { limit } => run.steps < *limit,
                };
                Step::Go {
                    to: if holds {
                        then.clone()
                    } else {
                        otherwise.clone()
                    },
                    note: Some(format!("{check}: {holds}")),
                    succeeded: run.last_succeeded,
                }
            }

            NodeKind::Delay { duration, next } => {
                let until = SystemTime::now() + *duration;
                let _ = next;
                self.clone().sleep_until(run.id, until);
                Step::Wait(Waiting::Delay { until }, None)
            }

            NodeKind::Approval { question, .. } => Step::Wait(
                Waiting::Approval {
                    question: question.clone(),
                },
                Some(question.clone()),
            ),

            NodeKind::Action { action, next } => {
                let Some(actions) = self.actions.lock().await.upgrade() else {
                    return Step::Finish(Outcome::Failed, "nothing can run actions".to_owned());
                };

                let mut surface = SurfaceContext::empty();
                surface.workspace = run.workspace.clone();

                match actions.run((**action).clone(), surface).await {
                    Ok(result) => Step::Go {
                        to: next.clone(),
                        note: result.message,
                        succeeded: result.status != pushos_domain::action::ActionStatus::Failed,
                    },
                    Err(error) => Step::Go {
                        to: next.clone(),
                        note: Some(error.to_string()),
                        succeeded: false,
                    },
                }
            }

            NodeKind::Agent {
                agent,
                prompt,
                on_failure,
                next,
            } => {
                let Some(agents) = &self.agents else {
                    return Self::cannot("no agents are configured", next, on_failure.as_ref());
                };

                let target = AgentTarget::Role {
                    agent: agent.clone(),
                    workspace: run.workspace.clone(),
                };

                match agents.prompt(&target, prompt).await {
                    Ok(session) => Step::Wait(
                        Waiting::Agent {
                            session: Some(session.id),
                        },
                        Some(format!("{agent} working")),
                    ),
                    Err(error) => Self::cannot(&error.to_string(), next, on_failure.as_ref()),
                }
            }

            NodeKind::Terminal {
                terminal,
                program,
                args,
                next,
                on_failure,
            } => {
                let Some(terminals) = &self.terminals else {
                    return Self::cannot("no terminals are available", next, on_failure.as_ref());
                };

                // A step that says "run the tests" means run them now, so a
                // terminal left over from last time is ended rather than reused.
                let named = TerminalTarget::Named(terminal.clone());
                if terminals.close(&named).await.is_ok() {
                    debug!(terminal, "ended the previous run before starting another");
                }

                let request = OpenTerminal {
                    name: terminal.clone(),
                    program: Some(program.clone()),
                    args: args.clone(),
                    cwd: None,
                    workspace: run.workspace.clone(),
                };

                match terminals.open(request).await {
                    Ok(opened) => Step::Wait(
                        Waiting::Terminal {
                            terminal: opened.name,
                        },
                        Some(format!("running {program}")),
                    ),
                    Err(error) => Self::cannot(&error.to_string(), next, on_failure.as_ref()),
                }
            }
        }
    }

    /// A step that could not even start.
    fn cannot(why: &str, next: &NodeId, on_failure: Option<&NodeId>) -> Step {
        Step::Go {
            to: on_failure.unwrap_or(next).clone(),
            note: Some(why.to_owned()),
            succeeded: false,
        }
    }

    /// Wakes a run when its delay is over.
    fn sleep_until(self: Arc<Self>, id: RunId, until: SystemTime) {
        let remaining = until
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);

        tokio::spawn(async move {
            tokio::time::sleep(remaining).await;

            let Some(run) = self.get(&id).await else {
                return;
            };
            let RunState::Waiting(Waiting::Delay { .. }) = run.state else {
                return;
            };
            let Some(NodeKind::Delay { next, .. }) = self.node(&run) else {
                return;
            };

            self.step_to(&id, next, Some("waited".to_owned()), run.last_succeeded)
                .await;
            self.drive(id);
        });
    }

    /// Moves a run to its next step, writing it down first.
    async fn step_to(&self, id: &RunId, to: NodeId, note: Option<String>, succeeded: bool) {
        let Some(mut run) = self.get(id).await else {
            return;
        };
        let from = run.at.clone();
        let at = SystemTime::now();

        // Written before the step it leads to runs. That ordering is the whole
        // of what durability means here.
        self.store
            .append(&Transition {
                run: *id,
                from: Some(from),
                to: to.clone(),
                note: note.clone(),
                at,
            })
            .await
            .unwrap_or_else(|error| warn!(%error, "a workflow step was not written down"));

        run.at = to;
        run.state = RunState::Running;
        run.steps += 1;
        run.last_succeeded = succeeded;
        run.note = note;
        run.updated_at = at;
        self.remember(&run).await;
    }

    /// Parks a run on something outside itself.
    async fn park(&self, id: &RunId, waiting: Waiting, note: Option<String>) {
        let Some(mut run) = self.get(id).await else {
            return;
        };
        debug!(run = %id, at = %run.at, "workflow waiting");

        run.state = RunState::Waiting(waiting);
        if note.is_some() {
            run.note = note;
        }
        run.updated_at = SystemTime::now();
        self.remember(&run).await;
    }

    /// Ends a run.
    async fn finish(&self, id: &RunId, outcome: Outcome, note: &str) {
        let Some(mut run) = self.get(id).await else {
            return;
        };

        run.state = RunState::Finished { outcome };
        run.note = Some(note.to_owned());
        run.updated_at = SystemTime::now();
        info!(run = %id, %outcome, "workflow finished");
        self.remember(&run).await;
    }

    /// Writes a run down and tells whoever is watching.
    async fn remember(&self, run: &Run) {
        self.runs.lock().await.insert(run.id, run.clone());
        if let Err(error) = self.store.record(run).await {
            warn!(%error, run = %run.id, "a workflow run was not written down");
        }
        self.observer.observe(run);
    }

    async fn get(&self, id: &RunId) -> Option<Run> {
        self.runs.lock().await.get(id).cloned()
    }

    async fn find(&self, matching: impl Fn(&Run) -> bool) -> Option<Run> {
        self.runs
            .lock()
            .await
            .values()
            .find(|run| matching(run))
            .cloned()
    }

    /// The step a run is on.
    fn node(&self, run: &Run) -> Option<NodeKind> {
        self.catalogue
            .get(&run.workflow)?
            .node(&run.at)
            .map(|node| node.kind.clone())
    }
}

/// What one step decided.
enum Step {
    /// Go on to another step.
    Go {
        to: NodeId,
        note: Option<String>,
        succeeded: bool,
    },
    /// Stop and wait for something outside.
    Wait(Waiting, Option<String>),
    /// End the run.
    Finish(Outcome, String),
}

/// Whether a run has used itself up, and how.
fn exhausted(run: &Run, workflow: &Workflow) -> Option<&'static str> {
    if run.steps >= workflow.limits.max_steps {
        return Some("took too many steps");
    }

    let elapsed = SystemTime::now()
        .duration_since(run.started_at)
        .unwrap_or(Duration::ZERO);
    if elapsed >= workflow.limits.timeout {
        return Some("took too long");
    }

    None
}

/// A stand-in so the engine can hold a weak reference before it has a runner.
#[derive(Debug)]
struct NoActions;

#[async_trait::async_trait]
impl ActionRunner for NoActions {
    async fn run(
        &self,
        _definition: pushos_domain::action::ActionDefinition,
        _surface: SurfaceContext,
    ) -> Result<pushos_domain::action::ActionResult, pushos_domain::error::ActionError> {
        unreachable!("nothing holds one of these")
    }
}

/// Why a workflow could not be started or moved.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkflowError {
    /// Nothing configures a workflow by that name.
    #[error("no workflow called `{workflow}` is configured")]
    Unknown {
        /// The name that was given.
        workflow: WorkflowId,
    },

    /// PushOS does not know that run.
    #[error("run `{run}` is not one PushOS knows about")]
    NoSuchRun {
        /// The run that was named.
        run: RunId,
    },

    /// The run is not waiting to be answered.
    #[error("run `{run}` is not waiting for an answer")]
    NotWaiting {
        /// The run that was named.
        run: RunId,
    },
}

impl WorkflowError {
    /// Classifies the failure.
    pub const fn class(&self) -> pushos_domain::error::ErrorClass {
        pushos_domain::error::ErrorClass::Validation
    }
}
