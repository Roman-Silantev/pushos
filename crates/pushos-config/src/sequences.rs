//! Named runs of several actions, and the proof that none can reach itself.

use std::collections::HashSet;

use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
use pushos_domain::ids::SequenceId;
use pushos_domain::sequence::{Sequence, SequenceMode, Step};

use crate::error::Problem;
use crate::model::ConfigFile;

/// Builds the named runs of several actions, and proves none can reach itself.
///
/// The reachability check is the reason this is done here rather than at the
/// moment a pad is pressed. A sequence that runs itself is an endless run of
/// whatever it contains, and the only safe time to discover that is while
/// reading the file.
pub(crate) fn build(file: &ConfigFile, problems: &mut Vec<Problem>) -> Vec<Sequence> {
    let mut built: Vec<Sequence> = Vec::new();

    for entry in &file.sequences {
        if built.iter().any(|kept| kept.id.as_str() == entry.id) {
            problems.push(Problem::DuplicateSequence {
                id: entry.id.clone(),
            });
            continue;
        }

        let written = entry.mode.as_deref();
        let Some(mode) = written.map_or(Some(SequenceMode::default()), SequenceMode::parse) else {
            problems.push(Problem::UnknownSequenceMode {
                id: entry.id.clone(),
                mode: written.unwrap_or_default().to_owned(),
            });
            continue;
        };

        if entry.steps.is_empty() {
            problems.push(Problem::EmptySequence {
                id: entry.id.clone(),
            });
            continue;
        }

        let mut steps = Vec::with_capacity(entry.steps.len());
        let mut sound = true;
        for (at, step) in entry.steps.iter().enumerate() {
            match step.action.parse::<ActionSelector>() {
                Ok(selector) => {
                    let mut params = Params::new();
                    for (key, value) in &step.params {
                        params.set(key, value.clone());
                    }
                    if let Some(target) = &step.target {
                        params.set("target", ParamValue::Text(target.as_str().into()));
                    }
                    steps.push(Step {
                        action: ActionDefinition::new(selector, params),
                        optional: step.optional,
                    });
                }
                Err(source) => {
                    sound = false;
                    problems.push(Problem::SequenceAction {
                        id: entry.id.clone(),
                        step: at + 1,
                        source,
                    });
                }
            }
        }

        if sound {
            built.push(Sequence {
                id: SequenceId::new(&entry.id),
                name: entry.name.clone(),
                description: entry.description.clone(),
                mode,
                steps,
            });
        }
    }

    check_sequences_terminate(&built, problems);
    built
}

/// Reports any sequence that names one that is not there, or reaches itself.
fn check_sequences_terminate(built: &[Sequence], problems: &mut Vec<Problem>) {
    let known: HashSet<&str> = built.iter().map(|run| run.id.as_str()).collect();

    for run in built {
        for (at, step) in run.steps.iter().enumerate() {
            if let Some(called) = step.calls()
                && !known.contains(called.as_str())
            {
                problems.push(Problem::UnknownSequence {
                    id: run.id.to_string(),
                    step: at + 1,
                    missing: called.to_string(),
                });
            }
        }
    }

    for run in built {
        if let Some(through) = reaches_itself(run, built) {
            problems.push(Problem::RecursiveSequence {
                id: run.id.to_string(),
                through,
            });
        }
    }
}

/// The sequence on the way back round to `from`, if there is a way back.
///
/// A breadth-first walk of what each sequence runs. The graph is a handful of
/// entries written by hand, so the plain walk is the readable one and the cost
/// of it does not matter.
fn reaches_itself(from: &Sequence, all: &[Sequence]) -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut frontier: Vec<(&str, String)> = from
        .calls()
        .map(|called| {
            let named = called.to_string();
            (
                all.iter()
                    .find(|run| run.id.as_str() == named)
                    .map_or("", |run| run.id.as_str()),
                named,
            )
        })
        .collect();

    while let Some((next, blamed)) = frontier.pop() {
        if next.is_empty() {
            // Named something that does not exist, which is reported elsewhere.
            continue;
        }
        if next == from.id.as_str() {
            return Some(blamed);
        }
        if !seen.insert(next) {
            continue;
        }
        let Some(run) = all.iter().find(|run| run.id.as_str() == next) else {
            continue;
        };
        for called in run.calls() {
            let named = called.to_string();
            let resolved = all
                .iter()
                .find(|other| other.id.as_str() == named)
                .map_or("", |other| other.id.as_str());
            // Blamed on the first hop out of `from`, because that is the line
            // in the file the operator has to change.
            frontier.push((resolved, blamed.clone()));
        }
    }

    None
}
