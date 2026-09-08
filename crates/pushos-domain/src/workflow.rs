//! Work that runs itself.
//!
//! A workflow is a graph the operator wrote: plan, then build, then test, then
//! review, with somewhere to go when the tests fail. One pad starts it, and it
//! carries on without anyone watching.
//!
//! The graph is validated once, when configuration is read, so that a run can
//! never reach a step that does not exist. Everything here is a description;
//! nothing here runs anything.
//!
//! A run is in one place at a time. Branches that run at once are deliberately
//! absent rather than pretended at: they would need a run to be in several
//! places, which is a change to what a run is, not another kind of step.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::time::Duration;

use crate::action::ActionDefinition;
use crate::ids::{AgentId, NodeId, WorkflowId};

/// One workflow, as configured.
#[derive(Clone, Debug, PartialEq)]
pub struct Workflow {
    /// Stable identity, referenced by bindings.
    pub id: WorkflowId,
    /// What the operator calls it.
    pub name: String,
    /// What it is for, when a name is not enough.
    pub description: Option<String>,
    /// Where a run begins.
    pub start: NodeId,
    /// Every step, by identity.
    pub nodes: BTreeMap<NodeId, Node>,
    /// What stops it running away.
    pub limits: Limits,
}

impl Workflow {
    /// Reads one step.
    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Everything wrong with this workflow.
    ///
    /// Collected rather than returned one at a time, so a single editing pass
    /// can fix a whole graph. Checked when configuration is read, so that a run
    /// can never reach a step that is not there: finding that out halfway
    /// through a build, after an agent has already changed files, is the
    /// expensive way.
    pub fn problems(&self) -> Vec<WorkflowProblem> {
        let mut problems = Vec::new();

        if self.nodes.is_empty() {
            problems.push(WorkflowProblem::Empty {
                workflow: self.id.clone(),
            });
            return problems;
        }

        if !self.nodes.contains_key(&self.start) {
            problems.push(WorkflowProblem::UnknownStart {
                workflow: self.id.clone(),
                node: self.start.clone(),
            });
        }

        for (id, node) in &self.nodes {
            if node.id != *id {
                problems.push(WorkflowProblem::Mislabelled {
                    workflow: self.id.clone(),
                    node: id.clone(),
                });
            }

            for target in node.successors() {
                if !self.nodes.contains_key(target) {
                    problems.push(WorkflowProblem::UnknownStep {
                        workflow: self.id.clone(),
                        node: id.clone(),
                        target: target.clone(),
                    });
                }
            }
        }

        // A graph with no way out never finishes, and a workflow that never
        // finishes is one nobody can tell has gone wrong.
        if !self
            .nodes
            .values()
            .any(|node| matches!(node.kind, NodeKind::End { .. }))
        {
            problems.push(WorkflowProblem::NoEnd {
                workflow: self.id.clone(),
            });
        }

        let reachable = self.reachable();
        for id in self.nodes.keys() {
            if !reachable.contains(id) {
                problems.push(WorkflowProblem::Unreachable {
                    workflow: self.id.clone(),
                    node: id.clone(),
                });
            }
        }

        problems
    }

    /// Every step the graph can reach from the start.
    ///
    /// Used by validation: a step nothing reaches is almost always a mistake in
    /// the file rather than a deliberate island.
    pub fn reachable(&self) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        let mut pending = vec![self.start.clone()];

        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(node) = self.nodes.get(&id) {
                pending.extend(node.successors().into_iter().cloned());
            }
        }

        seen
    }
}

/// What stops a workflow running for ever.
///
/// A graph with a loop in it is a graph that can loop for ever, and an agent
/// that costs money on every turn is not something to leave unbounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// How many steps one run may take in total.
    pub max_steps: u32,
    /// How long one run may take.
    pub timeout: Duration,
}

impl Limits {
    /// Enough for a long build with a retry, and not enough to run all night.
    pub const DEFAULT: Self = Self {
        max_steps: 200,
        timeout: Duration::from_mins(60),
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One step.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// What the step is called, in the file and on the display.
    pub id: NodeId,
    /// What it does.
    pub kind: NodeKind,
}

/// What a step does.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// Runs an action and goes on when it returns.
    ///
    /// Anything a pad can do, a workflow can do, because both go through the
    /// same dispatcher and the same permissions.
    Action {
        /// What to run.
        action: Box<ActionDefinition>,
        /// Where to go next.
        next: NodeId,
    },

    /// Asks a role to do something, and waits for it to finish the turn.
    ///
    /// Distinct from an action because the action returns as soon as the agent
    /// has been asked, and a workflow wants the answer.
    Agent {
        /// The role to ask.
        agent: AgentId,
        /// What to ask it.
        prompt: String,
        /// Where to go when it finishes.
        next: NodeId,
        /// Where to go when it fails, if not the same place.
        on_failure: Option<NodeId>,
    },

    /// Runs a program in a terminal and waits for it to exit.
    ///
    /// Program and arguments stay separate, as everywhere else in PushOS, and
    /// the terminal runs the program itself rather than a shell it is typed
    /// into: that way the terminal's exit status is the program's.
    Terminal {
        /// What the terminal is called, so a pad can watch it.
        terminal: String,
        /// The program to run.
        program: String,
        /// Its arguments.
        args: Vec<String>,
        /// Where to go when it exits cleanly.
        next: NodeId,
        /// Where to go when it does not.
        on_failure: Option<NodeId>,
    },

    /// Asks the operator, and waits.
    ///
    /// The one node that stops for a person. A workflow that would do something
    /// irreversible should put one of these in front of it.
    Approval {
        /// What to ask.
        question: String,
        /// Where to go when the answer is yes.
        approve: NodeId,
        /// Where to go when it is no.
        reject: NodeId,
    },

    /// Branches on what has happened so far.
    Condition {
        /// What to test.
        check: Check,
        /// Where to go when it holds.
        then: NodeId,
        /// Where to go when it does not.
        otherwise: NodeId,
    },

    /// Waits.
    Delay {
        /// How long.
        duration: Duration,
        /// Where to go afterwards.
        next: NodeId,
    },

    /// Says something, on the display and in the audit trail.
    Emit {
        /// What to say.
        message: String,
        /// Where to go afterwards.
        next: NodeId,
    },

    /// Stops.
    End {
        /// How the run ended.
        outcome: Outcome,
    },
}

impl NodeKind {
    /// Every step this one can lead to.
    pub fn successors(&self) -> Vec<&NodeId> {
        match self {
            Self::Action { next, .. } | Self::Delay { next, .. } | Self::Emit { next, .. } => {
                vec![next]
            }

            Self::Agent {
                next, on_failure, ..
            }
            | Self::Terminal {
                next, on_failure, ..
            } => {
                let mut found = vec![next];
                found.extend(on_failure.iter());
                found
            }

            Self::Approval {
                approve, reject, ..
            } => vec![approve, reject],
            Self::Condition {
                then, otherwise, ..
            } => vec![then, otherwise],
            Self::End { .. } => Vec::new(),
        }
    }

    /// What the step is, in one word, for the display.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Action { .. } => "action",
            Self::Agent { .. } => "agent",
            Self::Terminal { .. } => "terminal",
            Self::Approval { .. } => "approval",
            Self::Condition { .. } => "condition",
            Self::Delay { .. } => "delay",
            Self::Emit { .. } => "emit",
            Self::End { .. } => "end",
        }
    }

    /// Whether the step waits for something outside the workflow.
    ///
    /// A waiting step is where a run sits when PushOS stops, and is what makes
    /// resuming worth doing at all.
    pub const fn waits(&self) -> bool {
        matches!(
            self,
            Self::Agent { .. } | Self::Terminal { .. } | Self::Approval { .. } | Self::Delay { .. }
        )
    }
}

impl Node {
    /// Every step this one can lead to.
    pub fn successors(&self) -> Vec<&NodeId> {
        self.kind.successors()
    }
}

/// What a condition tests.
///
/// Deliberately small. A workflow is a thing an operator reads on a pad legend,
/// not a programming language, and an expression grammar here would be the
/// beginning of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// The step before this one succeeded.
    Succeeded,
    /// It did not.
    Failed,
    /// This run has been round fewer than `limit` times.
    ///
    /// What makes a retry loop terminate.
    Attempts {
        /// How many times round is still allowed.
        limit: u32,
    },
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded => f.write_str("succeeded"),
            Self::Failed => f.write_str("failed"),
            Self::Attempts { limit } => write!(f, "attempts under {limit}"),
        }
    }
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// It did what it set out to do.
    Succeeded,
    /// It did not.
    Failed,
    /// The operator stopped it.
    Cancelled,
    /// It ran out of steps or time.
    Exhausted,
}

impl Outcome {
    /// The light that communicates this outcome.
    pub const fn status_color(self) -> crate::color::StatusColor {
        use crate::color::StatusColor;
        match self {
            Self::Succeeded => StatusColor::Complete,
            Self::Failed | Self::Exhausted => StatusColor::Failed,
            Self::Cancelled => StatusColor::Idle,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded => f.write_str("succeeded"),
            Self::Failed => f.write_str("failed"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Exhausted => f.write_str("exhausted"),
        }
    }
}

/// Something wrong with a configured workflow.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowProblem {
    /// It has no steps at all.
    #[error("workflow `{workflow}` has no steps")]
    Empty {
        /// The workflow.
        workflow: WorkflowId,
    },

    /// It starts at a step that does not exist.
    #[error("workflow `{workflow}` starts at `{node}`, which is not one of its steps")]
    UnknownStart {
        /// The workflow.
        workflow: WorkflowId,
        /// The step it named.
        node: NodeId,
    },

    /// A step leads somewhere that does not exist.
    #[error(
        "workflow `{workflow}`: step `{node}` leads to `{target}`, which is not one of its steps"
    )]
    UnknownStep {
        /// The workflow.
        workflow: WorkflowId,
        /// The step that leads there.
        node: NodeId,
        /// Where it leads.
        target: NodeId,
    },

    /// A step is filed under a name that is not its own.
    #[error("workflow `{workflow}`: step `{node}` is filed under another name")]
    Mislabelled {
        /// The workflow.
        workflow: WorkflowId,
        /// The name it is filed under.
        node: NodeId,
    },

    /// Nothing in it ever ends.
    #[error("workflow `{workflow}` has no ending step, so a run could never finish")]
    NoEnd {
        /// The workflow.
        workflow: WorkflowId,
    },

    /// A step nothing leads to.
    #[error("workflow `{workflow}`: nothing leads to step `{node}`")]
    Unreachable {
        /// The workflow.
        workflow: WorkflowId,
        /// The step.
        node: NodeId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: NodeKind) -> (NodeId, Node) {
        let id = NodeId::new(id);
        (id.clone(), Node { id, kind })
    }

    fn workflow(nodes: Vec<(NodeId, Node)>, start: &str) -> Workflow {
        Workflow {
            id: WorkflowId::new("build"),
            name: "Build".to_owned(),
            description: None,
            start: NodeId::new(start),
            nodes: nodes.into_iter().collect(),
            limits: Limits::DEFAULT,
        }
    }

    /// The specification's own example: plan, build, test, review.
    fn chain() -> Workflow {
        workflow(
            vec![
                node(
                    "plan",
                    NodeKind::Agent {
                        agent: AgentId::new("architect"),
                        prompt: "Plan it".to_owned(),
                        next: NodeId::new("build"),
                        on_failure: None,
                    },
                ),
                node(
                    "build",
                    NodeKind::Agent {
                        agent: AgentId::new("builder"),
                        prompt: "Build it".to_owned(),
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
                        on_failure: Some(NodeId::new("stop")),
                    },
                ),
                node(
                    "review",
                    NodeKind::Agent {
                        agent: AgentId::new("reviewer"),
                        prompt: "Review it".to_owned(),
                        next: NodeId::new("done"),
                        on_failure: None,
                    },
                ),
                node(
                    "done",
                    NodeKind::End {
                        outcome: Outcome::Succeeded,
                    },
                ),
                node(
                    "stop",
                    NodeKind::End {
                        outcome: Outcome::Failed,
                    },
                ),
            ],
            "plan",
        )
    }

    #[test]
    fn a_chain_reaches_every_step_in_it() {
        assert_eq!(chain().reachable().len(), 6);
    }

    #[test]
    fn a_step_nothing_leads_to_is_not_reachable() {
        let flow = workflow(
            vec![
                node(
                    "start",
                    NodeKind::End {
                        outcome: Outcome::Succeeded,
                    },
                ),
                node(
                    "orphan",
                    NodeKind::End {
                        outcome: Outcome::Failed,
                    },
                ),
            ],
            "start",
        );

        let reached = flow.reachable();
        assert!(reached.contains(&NodeId::new("start")));
        assert!(!reached.contains(&NodeId::new("orphan")));
    }

    #[test]
    fn a_loop_does_not_make_reachability_run_for_ever() {
        // A retry loop is the point of conditions, so the walk has to cope.
        let flow = workflow(
            vec![
                node(
                    "tests",
                    NodeKind::Terminal {
                        terminal: "tests".to_owned(),
                        program: "cargo".to_owned(),
                        args: vec!["test".to_owned()],
                        next: NodeId::new("done"),
                        on_failure: Some(NodeId::new("again")),
                    },
                ),
                node(
                    "again",
                    NodeKind::Condition {
                        check: Check::Attempts { limit: 3 },
                        then: NodeId::new("tests"),
                        otherwise: NodeId::new("give_up"),
                    },
                ),
                node(
                    "done",
                    NodeKind::End {
                        outcome: Outcome::Succeeded,
                    },
                ),
                node(
                    "give_up",
                    NodeKind::End {
                        outcome: Outcome::Failed,
                    },
                ),
            ],
            "tests",
        );

        assert_eq!(flow.reachable().len(), 4);
    }

    #[test]
    fn a_workflow_that_holds_together_has_nothing_wrong_with_it() {
        assert!(chain().problems().is_empty());
    }

    #[test]
    fn a_step_that_leads_nowhere_real_is_reported_with_both_names() {
        // The message has to say which step and where it points, or the
        // operator is left searching the file.
        let mut broken = chain();
        broken.nodes.insert(
            NodeId::new("plan"),
            Node {
                id: NodeId::new("plan"),
                kind: NodeKind::Emit {
                    message: "hi".to_owned(),
                    next: NodeId::new("nowhere"),
                },
            },
        );

        let said = broken
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(said.contains("`plan`"), "{said}");
        assert!(said.contains("`nowhere`"), "{said}");
    }

    #[test]
    fn a_workflow_that_starts_nowhere_is_refused() {
        let mut broken = chain();
        broken.start = NodeId::new("missing");
        assert!(
            broken
                .problems()
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::UnknownStart { .. }))
        );
    }

    #[test]
    fn a_workflow_with_no_ending_is_refused() {
        // It could never finish, and a run nobody can tell has gone wrong is
        // worse than one that fails.
        let endless = workflow(
            vec![node(
                "round",
                NodeKind::Emit {
                    message: "again".to_owned(),
                    next: NodeId::new("round"),
                },
            )],
            "round",
        );
        assert!(
            endless
                .problems()
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::NoEnd { .. }))
        );
    }

    #[test]
    fn a_step_nothing_leads_to_is_reported_as_the_mistake_it_usually_is() {
        let mut orphaned = chain();
        orphaned.nodes.insert(
            NodeId::new("stray"),
            Node {
                id: NodeId::new("stray"),
                kind: NodeKind::End {
                    outcome: Outcome::Failed,
                },
            },
        );
        assert!(
            orphaned
                .problems()
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::Unreachable { .. }))
        );
    }

    #[test]
    fn an_empty_workflow_is_reported_once_rather_than_as_everything_at_once() {
        let empty = workflow(Vec::new(), "start");
        assert_eq!(empty.problems().len(), 1);
    }

    #[test]
    fn a_step_that_can_fail_leads_both_ways() {
        let kind = NodeKind::Terminal {
            terminal: "tests".to_owned(),
            program: "cargo".to_owned(),
            args: vec!["test".to_owned()],
            next: NodeId::new("review"),
            on_failure: Some(NodeId::new("fix")),
        };
        let successors: Vec<String> = kind.successors().iter().map(ToString::to_string).collect();
        assert_eq!(successors, ["review", "fix"]);
    }

    #[test]
    fn an_end_leads_nowhere() {
        let kind = NodeKind::End {
            outcome: Outcome::Succeeded,
        };
        assert!(kind.successors().is_empty());
        assert!(!kind.waits());
    }

    #[test]
    fn the_steps_that_wait_are_the_ones_worth_resuming() {
        // These are where a run sits when PushOS stops.
        assert!(
            NodeKind::Approval {
                question: "Ship it?".to_owned(),
                approve: NodeId::new("a"),
                reject: NodeId::new("b"),
            }
            .waits()
        );
        assert!(
            NodeKind::Delay {
                duration: Duration::from_secs(1),
                next: NodeId::new("a"),
            }
            .waits()
        );
        assert!(
            !NodeKind::Emit {
                message: "hello".to_owned(),
                next: NodeId::new("a"),
            }
            .waits()
        );
    }

    #[test]
    fn an_outcome_the_operator_chose_is_not_shown_as_a_failure() {
        use crate::color::StatusColor;
        assert_eq!(Outcome::Cancelled.status_color(), StatusColor::Idle);
        assert_eq!(Outcome::Failed.status_color(), StatusColor::Failed);
        assert_eq!(Outcome::Exhausted.status_color(), StatusColor::Failed);
        assert_eq!(Outcome::Succeeded.status_color(), StatusColor::Complete);
    }

    #[test]
    fn the_default_limits_allow_real_work_and_not_a_runaway() {
        let limits = Limits::default();
        assert!(limits.max_steps >= 50);
        assert!(limits.timeout >= Duration::from_mins(10));
        assert!(limits.timeout <= Duration::from_hours(4));
    }
}
