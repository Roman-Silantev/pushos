//! Rejection of ambiguous binding sets.
//!
//! Two bindings are ambiguous when they share a control, a gesture, a scope and
//! a priority: nothing in the precedence rules could separate them, so which
//! one fires would depend on iteration order. PushOS refuses that rather than
//! picking one, because an operator must be able to predict the surface by
//! reading the configuration.

use std::collections::HashMap;

use pushos_domain::binding::{Binding, BindingScope};
use pushos_domain::controls::ControlId;
use pushos_domain::gesture::Gesture;
use pushos_domain::ids::BindingId;

/// Checks a binding set for conflicts that precedence cannot resolve.
///
/// Returns every conflict found rather than only the first, so a configuration
/// error can be corrected in one pass.
pub fn find_conflicts(bindings: &[Binding]) -> Vec<BindingConflict> {
    let mut groups: HashMap<(ControlId, Gesture, &BindingScope, u16), Vec<&Binding>> =
        HashMap::new();

    for binding in bindings {
        groups
            .entry((
                binding.control,
                binding.gesture,
                &binding.scope,
                binding.priority,
            ))
            .or_default()
            .push(binding);
    }

    let mut conflicts: Vec<_> = groups
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .map(
            |((control, gesture, scope, priority), group)| BindingConflict {
                control,
                gesture,
                scope: scope.clone(),
                priority,
                bindings: group.iter().map(|binding| binding.id.clone()).collect(),
            },
        )
        .collect();

    // Stable output keeps error messages reproducible across runs.
    conflicts.sort_by_key(|conflict| (conflict.control, conflict.gesture, conflict.priority));
    conflicts
}

/// Two or more bindings that precedence cannot separate.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{control}` + `{gesture}` has {} bindings at the same scope and priority {priority}: {}",
    bindings.len(),
    bindings.iter().map(BindingId::to_string).collect::<Vec<_>>().join(", ")
)]
pub struct BindingConflict {
    /// The contested control.
    pub control: ControlId,
    /// The contested gesture.
    pub gesture: Gesture,
    /// The scope they share.
    pub scope: BindingScope,
    /// The priority they share.
    pub priority: u16,
    /// The bindings involved.
    pub bindings: Vec<BindingId>,
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector};
    use pushos_domain::controls::PadIndex;

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    fn binding(id: &str, control: ControlId, scope: BindingScope, priority: u16) -> Binding {
        Binding {
            id: id.into(),
            control,
            gesture: Gesture::Tap,
            scope,
            action: ActionDefinition::bare(ActionSelector::new("test", "noop")),
            priority,
            label: None,
        }
    }

    #[test]
    fn a_sound_configuration_reports_no_conflicts() {
        let bindings = [
            binding("a", pad(0), BindingScope::Global, 0),
            binding("b", pad(1), BindingScope::Global, 0),
            binding("c", pad(0), BindingScope::Page("dev".into()), 0),
        ];
        assert!(find_conflicts(&bindings).is_empty());
    }

    #[test]
    fn identical_scope_and_priority_is_a_conflict() {
        let bindings = [
            binding("a", pad(0), BindingScope::Global, 0),
            binding("b", pad(0), BindingScope::Global, 0),
        ];
        let conflicts = find_conflicts(&bindings);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].bindings.len(), 2);
        assert!(conflicts[0].to_string().contains("pad.0"));
    }

    #[test]
    fn differing_priority_resolves_an_otherwise_identical_pair() {
        let bindings = [
            binding("a", pad(0), BindingScope::Global, 0),
            binding("b", pad(0), BindingScope::Global, 1),
        ];
        assert!(find_conflicts(&bindings).is_empty());
    }

    #[test]
    fn the_same_control_in_different_workspaces_does_not_conflict() {
        let bindings = [
            binding("a", pad(0), BindingScope::Workspace("one".into()), 0),
            binding("b", pad(0), BindingScope::Workspace("two".into()), 0),
        ];
        assert!(find_conflicts(&bindings).is_empty());
    }

    #[test]
    fn every_conflict_is_reported_not_just_the_first() {
        let bindings = [
            binding("a", pad(0), BindingScope::Global, 0),
            binding("b", pad(0), BindingScope::Global, 0),
            binding("c", pad(5), BindingScope::Global, 0),
            binding("d", pad(5), BindingScope::Global, 0),
        ];
        assert_eq!(find_conflicts(&bindings).len(), 2);
    }
}
