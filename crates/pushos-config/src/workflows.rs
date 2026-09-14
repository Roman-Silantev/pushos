//! Workflows: graphs of steps, checked so a run can never reach a step that
//! is not there.

use std::collections::HashSet;

use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};

use crate::error::Problem;
use crate::model::ConfigFile;

pub(crate) fn build(
    file: &ConfigFile,
    problems: &mut Vec<Problem>,
) -> Vec<pushos_domain::workflow::Workflow> {
    use pushos_domain::ids::{NodeId, WorkflowId};
    use pushos_domain::workflow::{Limits, Workflow};

    let mut seen = HashSet::with_capacity(file.workflows.len());
    let mut workflows = Vec::with_capacity(file.workflows.len());

    for entry in &file.workflows {
        if !seen.insert(entry.id.clone()) {
            problems.push(Problem::DuplicateWorkflow {
                id: entry.id.clone(),
            });
            continue;
        }

        let before = problems.len();
        let nodes: std::collections::BTreeMap<_, _> = entry
            .nodes
            .iter()
            .filter_map(|node| build_node(&entry.id, node, problems))
            .map(|node| (node.id.clone(), node))
            .collect();

        // A graph missing steps would produce a cascade of "leads nowhere"
        // problems on top of the real one, which buries it.
        if problems.len() > before {
            continue;
        }

        let workflow = Workflow {
            id: WorkflowId::new(&entry.id),
            name: entry.name.clone(),
            description: entry.description.clone(),
            start: NodeId::new(&entry.start),
            nodes,
            limits: Limits {
                max_steps: entry.max_steps.unwrap_or(Limits::DEFAULT.max_steps),
                timeout: entry
                    .timeout_seconds
                    .map_or(Limits::DEFAULT.timeout, std::time::Duration::from_secs),
            },
        };

        let found = workflow.problems();
        if found.is_empty() {
            workflows.push(workflow);
        } else {
            problems.extend(found.into_iter().map(Problem::Workflow));
        }
    }

    workflows
}

/// Builds one step, reporting what it needed and did not have.
fn build_node(
    workflow: &str,
    entry: &crate::model::NodeEntry,
    problems: &mut Vec<Problem>,
) -> Option<pushos_domain::workflow::Node> {
    use pushos_domain::ids::{AgentId, NodeId};
    use pushos_domain::workflow::{Check, Node, NodeKind, Outcome};

    let mut missing = |field: &str| {
        problems.push(Problem::IncompleteStep {
            workflow: workflow.to_owned(),
            node: entry.id.clone(),
            kind: entry.kind.clone(),
            missing: field.to_owned(),
        });
    };

    let kind = match entry.kind.as_str() {
        "agent" => {
            let (Some(agent), Some(prompt), Some(next)) =
                (&entry.agent, &entry.prompt, &entry.next)
            else {
                for (name, present) in [
                    ("agent", entry.agent.is_some()),
                    ("prompt", entry.prompt.is_some()),
                    ("next", entry.next.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };
            NodeKind::Agent {
                agent: AgentId::new(agent),
                prompt: prompt.clone(),
                next: NodeId::new(next),
                on_failure: entry.on_failure.as_deref().map(NodeId::new),
            }
        }

        "terminal" => {
            let (Some(terminal), Some(program), Some(next)) =
                (&entry.terminal, &entry.program, &entry.next)
            else {
                for (name, present) in [
                    ("terminal", entry.terminal.is_some()),
                    ("program", entry.program.is_some()),
                    ("next", entry.next.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };
            NodeKind::Terminal {
                terminal: terminal.clone(),
                program: program.clone(),
                args: entry.args.clone(),
                next: NodeId::new(next),
                on_failure: entry.on_failure.as_deref().map(NodeId::new),
            }
        }

        "action" => {
            let (Some(action), Some(next)) = (&entry.action, &entry.next) else {
                for (name, present) in [
                    ("action", entry.action.is_some()),
                    ("next", entry.next.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };

            let Ok(selector) = action.parse::<ActionSelector>() else {
                missing("action written as provider.verb");
                return None;
            };

            let mut params = Params::new();
            for (key, value) in &entry.params {
                params.set(key, value.clone());
            }
            if let Some(target) = &entry.target {
                params.set("target", ParamValue::Text(target.as_str().into()));
            }

            NodeKind::Action {
                action: Box::new(ActionDefinition::new(selector, params)),
                next: NodeId::new(next),
            }
        }

        "approval" => {
            let (Some(question), Some(approve), Some(reject)) =
                (&entry.question, &entry.approve, &entry.reject)
            else {
                for (name, present) in [
                    ("question", entry.question.is_some()),
                    ("approve", entry.approve.is_some()),
                    ("reject", entry.reject.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };
            NodeKind::Approval {
                question: question.clone(),
                approve: NodeId::new(approve),
                reject: NodeId::new(reject),
            }
        }

        "condition" => {
            let (Some(check), Some(then), Some(otherwise)) =
                (&entry.check, &entry.then, &entry.otherwise)
            else {
                for (name, present) in [
                    ("check", entry.check.is_some()),
                    ("then", entry.then.is_some()),
                    ("otherwise", entry.otherwise.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };

            let check = match check.as_str() {
                "succeeded" => Check::Succeeded,
                "failed" => Check::Failed,
                "attempts" => {
                    let Some(limit) = entry.limit else {
                        missing("limit");
                        return None;
                    };
                    Check::Attempts { limit }
                }
                _ => {
                    missing("check of `succeeded`, `failed` or `attempts`");
                    return None;
                }
            };

            NodeKind::Condition {
                check,
                then: NodeId::new(then),
                otherwise: NodeId::new(otherwise),
            }
        }

        "delay" => {
            let (Some(seconds), Some(next)) = (entry.seconds, &entry.next) else {
                for (name, present) in [
                    ("seconds", entry.seconds.is_some()),
                    ("next", entry.next.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };
            NodeKind::Delay {
                duration: std::time::Duration::from_secs(seconds),
                next: NodeId::new(next),
            }
        }

        "emit" => {
            let (Some(message), Some(next)) = (&entry.message, &entry.next) else {
                for (name, present) in [
                    ("message", entry.message.is_some()),
                    ("next", entry.next.is_some()),
                ] {
                    if !present {
                        missing(name);
                    }
                }
                return None;
            };
            NodeKind::Emit {
                message: message.clone(),
                next: NodeId::new(next),
            }
        }

        "end" => {
            let outcome = match entry.outcome.as_deref().unwrap_or("succeeded") {
                "succeeded" => Outcome::Succeeded,
                "failed" => Outcome::Failed,
                "cancelled" => Outcome::Cancelled,
                _ => {
                    missing("outcome of `succeeded`, `failed` or `cancelled`");
                    return None;
                }
            };
            NodeKind::End { outcome }
        }

        other => {
            problems.push(Problem::UnknownStepKind {
                workflow: workflow.to_owned(),
                node: entry.id.clone(),
                kind: other.to_owned(),
            });
            return None;
        }
    };

    Some(Node {
        id: NodeId::new(&entry.id),
        kind,
    })
}
