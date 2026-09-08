//! The workflows PushOS knows about, and what makes one usable.
//!
//! A graph is checked once, when configuration is read, so that a run can never
//! reach a step that is not there. Finding that out halfway through a build,
//! after an agent has already changed files, is the expensive way.

use std::collections::HashSet;

use pushos_domain::ids::{NodeId, WorkflowId};
use pushos_domain::workflow::{NodeKind, Workflow};

/// Every workflow, in the order they were configured.
#[derive(Debug, Default)]
pub struct WorkflowCatalogue {
    workflows: Vec<Workflow>,
}

impl WorkflowCatalogue {
    /// Builds a catalogue, keeping the first of any repeated identity.
    pub fn new(workflows: impl IntoIterator<Item = Workflow>) -> Self {
        let mut kept: Vec<Workflow> = Vec::new();
        for workflow in workflows {
            if !kept.iter().any(|existing| existing.id == workflow.id) {
                kept.push(workflow);
            }
        }
        Self { workflows: kept }
    }

    /// Every workflow, in configured order.
    pub fn all(&self) -> &[Workflow] {
        &self.workflows
    }

    /// One workflow by identity.
    pub fn get(&self, id: &WorkflowId) -> Option<&Workflow> {
        self.workflows.iter().find(|workflow| workflow.id == *id)
    }

    /// Whether any workflow is configured.
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
    }

    /// How many are configured.
    pub fn len(&self) -> usize {
        self.workflows.len()
    }
}

/// Everything wrong with one workflow.
///
/// Collected rather than returned one at a time, so a single editing pass can
/// fix a whole graph.
pub fn problems_with(workflow: &Workflow) -> Vec<WorkflowProblem> {
    let mut problems = Vec::new();

    if workflow.nodes.is_empty() {
        problems.push(WorkflowProblem::Empty {
            workflow: workflow.id.clone(),
        });
        return problems;
    }

    if !workflow.nodes.contains_key(&workflow.start) {
        problems.push(WorkflowProblem::UnknownStart {
            workflow: workflow.id.clone(),
            node: workflow.start.clone(),
        });
    }

    for (id, node) in &workflow.nodes {
        if node.id != *id {
            problems.push(WorkflowProblem::Mislabelled {
                workflow: workflow.id.clone(),
                node: id.clone(),
            });
        }

        for target in node.successors() {
            if !workflow.nodes.contains_key(target) {
                problems.push(WorkflowProblem::UnknownStep {
                    workflow: workflow.id.clone(),
                    node: id.clone(),
                    target: target.clone(),
                });
            }
        }
    }

    // A graph with no way out never finishes, and a workflow that never
    // finishes is a workflow nobody can tell has gone wrong.
    if !workflow
        .nodes
        .values()
        .any(|node| matches!(node.kind, NodeKind::End { .. }))
    {
        problems.push(WorkflowProblem::NoEnd {
            workflow: workflow.id.clone(),
        });
    }

    let reachable: HashSet<NodeId> = workflow.reachable();
    for id in workflow.nodes.keys() {
        if !reachable.contains(id) {
            problems.push(WorkflowProblem::Unreachable {
                workflow: workflow.id.clone(),
                node: id.clone(),
            });
        }
    }

    problems
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
    use pushos_domain::ids::AgentId;
    use pushos_domain::workflow::{Limits, Node, Outcome};

    use super::*;

    fn node(id: &str, kind: NodeKind) -> (NodeId, Node) {
        let id = NodeId::new(id);
        (id.clone(), Node { id, kind })
    }

    fn agent(next: &str) -> NodeKind {
        NodeKind::Agent {
            agent: AgentId::new("builder"),
            prompt: "Do it".to_owned(),
            next: NodeId::new(next),
            on_failure: None,
        }
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

    fn valid() -> Workflow {
        workflow(
            vec![
                node("work", agent("done")),
                node(
                    "done",
                    NodeKind::End {
                        outcome: Outcome::Succeeded,
                    },
                ),
            ],
            "work",
        )
    }

    #[test]
    fn a_workflow_that_holds_together_has_nothing_wrong_with_it() {
        assert!(problems_with(&valid()).is_empty());
    }

    #[test]
    fn a_step_that_leads_nowhere_real_is_reported_with_both_names() {
        // The message has to say which step and where it points, or the
        // operator is left searching the file.
        let broken = workflow(vec![node("work", agent("nowhere"))], "work");
        let found = problems_with(&broken);

        let message = found
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(message.contains("`work`"), "{message}");
        assert!(message.contains("`nowhere`"), "{message}");
    }

    #[test]
    fn a_workflow_that_starts_nowhere_is_refused() {
        let broken = workflow(
            vec![node(
                "done",
                NodeKind::End {
                    outcome: Outcome::Succeeded,
                },
            )],
            "missing",
        );
        assert!(
            problems_with(&broken)
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::UnknownStart { .. }))
        );
    }

    #[test]
    fn a_workflow_with_no_ending_is_refused() {
        // It could never finish, and a run nobody can tell has gone wrong is
        // worse than one that fails.
        let endless = workflow(vec![node("work", agent("work"))], "work");
        assert!(
            problems_with(&endless)
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::NoEnd { .. }))
        );
    }

    #[test]
    fn a_step_nothing_leads_to_is_reported_as_the_mistake_it_usually_is() {
        let mut orphaned = valid();
        let (id, node) = node(
            "stray",
            NodeKind::End {
                outcome: Outcome::Failed,
            },
        );
        orphaned.nodes.insert(id, node);

        assert!(
            problems_with(&orphaned)
                .iter()
                .any(|problem| matches!(problem, WorkflowProblem::Unreachable { .. }))
        );
    }

    #[test]
    fn an_empty_workflow_is_reported_once_rather_than_as_everything_at_once() {
        let empty = workflow(Vec::new(), "start");
        let found = problems_with(&empty);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_repeated_identity_keeps_the_first() {
        let catalogue = WorkflowCatalogue::new([valid(), valid()]);
        assert_eq!(catalogue.len(), 1);
    }

    #[test]
    fn a_catalogue_finds_what_it_was_given() {
        let catalogue = WorkflowCatalogue::new([valid()]);
        assert!(catalogue.get(&WorkflowId::new("build")).is_some());
        assert!(catalogue.get(&WorkflowId::new("ship")).is_none());
    }
}
