//! The workflows PushOS knows about, and what makes one usable.
//!
//! A graph is checked once, when configuration is read, so that a run can never
//! reach a step that is not there. Finding that out halfway through a build,
//! after an agent has already changed files, is the expensive way.

use pushos_domain::ids::WorkflowId;
use pushos_domain::workflow::Workflow;

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

#[cfg(test)]
mod tests {
    use pushos_domain::ids::{AgentId, NodeId};
    use pushos_domain::workflow::{Limits, Node, NodeKind, Outcome};

    use super::*;

    fn valid() -> Workflow {
        let work = NodeId::new("work");
        let done = NodeId::new("done");
        Workflow {
            id: WorkflowId::new("build"),
            name: "Build".to_owned(),
            description: None,
            start: work.clone(),
            nodes: vec![
                (
                    work.clone(),
                    Node {
                        id: work,
                        kind: NodeKind::Agent {
                            agent: AgentId::new("builder"),
                            prompt: "Do it".to_owned(),
                            next: done.clone(),
                            on_failure: None,
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

    #[test]
    fn an_empty_catalogue_says_so() {
        let catalogue = WorkflowCatalogue::new([]);
        assert!(catalogue.is_empty());
        assert!(catalogue.all().is_empty());
    }
}
