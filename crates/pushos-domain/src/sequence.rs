//! Several actions behind one gesture.
//!
//! A sequence is the smallest thing that is worth a name: "start work" is six
//! actions, and an operator should press one pad rather than six. It is not a
//! workflow. A workflow is durable, has branches and approvals, and survives a
//! restart mid-run; a sequence is a handful of actions that either happen now
//! or report why they did not.
//!
//! Sequences carry no logic of their own. Each step is an ordinary
//! [`ActionDefinition`], carried out through the same dispatcher a finger uses,
//! so a step is subject to the same permissions and reports the same result as
//! it would from its own pad.

use crate::action::ActionDefinition;
use crate::ids::SequenceId;

/// Several actions, run together under one name.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    /// Stable identity, referred to by bindings.
    pub id: SequenceId,
    /// What the operator calls it.
    pub name: String,
    /// What it is for, when a name is not enough.
    pub description: Option<String>,
    /// Whether the steps happen one after another or all at once.
    pub mode: SequenceMode,
    /// The steps, in the order they were written.
    pub steps: Vec<Step>,
}

impl Sequence {
    /// How many steps have to work for the sequence to have worked.
    pub fn required(&self) -> usize {
        self.steps.iter().filter(|step| !step.optional).count()
    }

    /// Every sequence this one runs directly, by identity.
    ///
    /// Used to prove a sequence cannot reach itself. A press that started an
    /// endless run of itself would take the surface with it, and the place to
    /// find that out is when the file is read.
    pub fn calls(&self) -> impl Iterator<Item = SequenceId> + '_ {
        self.steps.iter().filter_map(Step::calls)
    }
}

/// How the steps of a sequence relate to each other in time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SequenceMode {
    /// One after another. A step that fails stops the ones after it.
    ///
    /// The default, because the reason to name several actions is usually that
    /// the later ones only make sense once the earlier ones have happened.
    #[default]
    Sequential,
    /// All at once, waiting for all of them.
    ///
    /// For steps that have nothing to do with each other, where doing them in
    /// order would only be slower.
    Parallel,
}

impl SequenceMode {
    /// The stable textual form used in configuration.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Parallel => "parallel",
        }
    }

    /// Reads the configured form, if it is one PushOS knows.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "sequential" => Some(Self::Sequential),
            "parallel" => Some(Self::Parallel),
            _ => None,
        }
    }
}

impl std::fmt::Display for SequenceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.slug())
    }
}

/// One action within a sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// What to do.
    pub action: ActionDefinition,
    /// Whether the rest should carry on if this one fails.
    ///
    /// A sequence is usually a chain of things that depend on each other, so
    /// the default is no. Marking a step optional is how an operator says that
    /// this one is a nicety: the playlist that did not start is no reason to
    /// abandon a workspace that opened.
    pub optional: bool,
}

impl Step {
    /// Builds a required step.
    pub fn new(action: ActionDefinition) -> Self {
        Self {
            action,
            optional: false,
        }
    }

    /// Marks the step as one whose failure the rest can carry on past.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// The sequence this step runs, when it runs one.
    pub fn calls(&self) -> Option<SequenceId> {
        if self.action.selector.provider.as_str() != NAMESPACE
            || self.action.selector.verb.as_str() != RUN
        {
            return None;
        }
        self.action.params.text("id").map(SequenceId::new)
    }
}

/// The namespace sequences are addressed in.
pub const NAMESPACE: &str = "sequence";

/// The verb that runs one.
pub const RUN: &str = "run";

#[cfg(test)]
mod tests {
    use crate::action::{ActionSelector, Params};

    use super::*;

    fn step(action: &str) -> Step {
        Step::new(ActionDefinition::bare(
            action.parse::<ActionSelector>().expect("well-formed"),
        ))
    }

    fn calling(id: &str) -> Step {
        let mut params = Params::new();
        params.set("id", id);
        Step::new(ActionDefinition::new(
            ActionSelector::new(NAMESPACE, RUN),
            params,
        ))
    }

    fn sequence(steps: Vec<Step>) -> Sequence {
        Sequence {
            id: SequenceId::new("start_work"),
            name: "Start work".to_owned(),
            description: None,
            mode: SequenceMode::Sequential,
            steps,
        }
    }

    #[test]
    fn every_mode_round_trips_through_its_written_form() {
        for mode in [SequenceMode::Sequential, SequenceMode::Parallel] {
            assert_eq!(SequenceMode::parse(mode.slug()), Some(mode));
        }
        assert_eq!(SequenceMode::parse("eventually"), None);
    }

    #[test]
    fn running_things_in_order_is_what_a_sequence_does_unless_told_otherwise() {
        assert_eq!(SequenceMode::default(), SequenceMode::Sequential);
    }

    #[test]
    fn an_optional_step_is_not_one_the_sequence_depends_on() {
        let run = sequence(vec![
            step("media.play_pause"),
            step("app.open").optional(),
            step("shell.run"),
        ]);
        assert_eq!(run.required(), 2);
    }

    #[test]
    fn a_step_that_runs_another_sequence_says_which_one() {
        let run = sequence(vec![step("media.play_pause"), calling("focus")]);
        assert_eq!(run.calls().collect::<Vec<_>>(), [SequenceId::new("focus")]);
    }

    #[test]
    fn a_step_that_runs_anything_else_calls_no_sequence() {
        // Including a verb in the same namespace that is not `run`, so listing
        // what is configured can never be mistaken for running it.
        let run = sequence(vec![step("media.play_pause"), step("sequence.list")]);
        assert_eq!(run.calls().count(), 0);
    }
}
