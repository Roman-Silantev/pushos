//! Rejection of ambiguous binding sets.
//!
//! Two bindings are ambiguous when they share a control, a gesture, a force
//! band, a scope and a priority: nothing in the precedence rules could
//! separate them, so which one fires would depend on iteration order. PushOS
//! refuses that rather than picking one, because an operator must be able to
//! predict the surface by reading the configuration.
//!
//! Two more refusals concern force. A band on a control that cannot report one
//! would never match, and an action that stops work must not be reachable only
//! by hitting a pad hard. Both are caught here rather than discovered on the
//! hardware.

use std::collections::HashMap;

use pushos_domain::binding::{Binding, BindingScope};
use pushos_domain::controls::ControlId;
use pushos_domain::gesture::{ForceBand, Gesture};
use pushos_domain::ids::BindingId;

/// What a group of bindings would have to agree on to be ambiguous.
///
/// Every part of the precedence rules, so two bindings landing in one group
/// are ones nothing could separate.
type Contested<'a> = (ControlId, Gesture, Option<ForceBand>, &'a BindingScope, u16);

/// Checks a binding set for conflicts that precedence cannot resolve.
///
/// Returns every conflict found rather than only the first, so a configuration
/// error can be corrected in one pass.
pub fn find_conflicts(bindings: &[Binding]) -> Vec<BindingConflict> {
    let mut groups: HashMap<Contested<'_>, Vec<&Binding>> = HashMap::new();

    for binding in bindings {
        groups
            .entry((
                binding.control,
                binding.gesture,
                binding.force,
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
            |((control, gesture, force, scope, priority), group)| BindingConflict {
                control,
                gesture,
                force,
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

/// Checks every binding that narrows itself to a force band.
///
/// Returns each objection rather than only the first, so a configuration can
/// be corrected in one pass.
pub fn find_force_faults(bindings: &[Binding]) -> Vec<ForceFault> {
    let mut faults: Vec<ForceFault> = bindings
        .iter()
        .filter_map(|binding| {
            let band = binding.force?;
            if !matches!(binding.control, ControlId::Pad(_)) {
                return Some(ForceFault::NotAPad {
                    binding: binding.id.clone(),
                    control: binding.control,
                    force: band,
                });
            }
            if !binding.gesture.carries_force() {
                return Some(ForceFault::NoStrike {
                    binding: binding.id.clone(),
                    gesture: binding.gesture,
                    force: band,
                });
            }
            if binding.action.selector.halts_work() {
                return Some(ForceFault::HaltsWork {
                    binding: binding.id.clone(),
                    action: binding.action.selector.to_string(),
                    force: band,
                });
            }
            None
        })
        .collect();

    faults.sort_by(|left, right| left.binding().cmp(right.binding()));
    faults
}

/// A binding whose force band could never work, or should never exist.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ForceFault {
    /// A band on a control that reports no usable force.
    #[error(
        "binding `{binding}` asks for a `{force}` strike on `{control}`, but only pads \
         report how hard they were hit"
    )]
    NotAPad {
        /// The offending binding.
        binding: BindingId,
        /// The control it names.
        control: ControlId,
        /// The band it asked for.
        force: ForceBand,
    },
    /// A band on a gesture with no strike behind it.
    #[error(
        "binding `{binding}` asks for a `{force}` strike on `{gesture}`, which happens \
         after the strike is over; band `press`, `tap` or `double_tap` instead"
    )]
    NoStrike {
        /// The offending binding.
        binding: BindingId,
        /// The gesture it names.
        gesture: Gesture,
        /// The band it asked for.
        force: ForceBand,
    },
    /// A band standing alone in front of something that stops work.
    #[error(
        "binding `{binding}` would run `{action}` when a pad is struck `{force}`; how hard \
         a pad was hit must not be the only thing standing in front of an action that \
         stops or discards work"
    )]
    HaltsWork {
        /// The offending binding.
        binding: BindingId,
        /// The action it would have run.
        action: String,
        /// The band it asked for.
        force: ForceBand,
    },
}

impl ForceFault {
    /// The binding the fault is about.
    pub const fn binding(&self) -> &BindingId {
        match self {
            Self::NotAPad { binding, .. }
            | Self::NoStrike { binding, .. }
            | Self::HaltsWork { binding, .. } => binding,
        }
    }
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
    /// The force band they share, when they are banded.
    pub force: Option<ForceBand>,
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
    use pushos_domain::controls::{ButtonId, PadIndex};

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    fn binding(id: &str, control: ControlId, scope: BindingScope, priority: u16) -> Binding {
        Binding {
            id: id.into(),
            control,
            gesture: Gesture::Tap,
            force: None,
            scope,
            action: ActionDefinition::bare(ActionSelector::new("test", "noop")),
            priority,
            label: None,
        }
    }

    fn struck(id: &str, control: ControlId, gesture: Gesture, force: ForceBand) -> Binding {
        Binding {
            id: id.into(),
            control,
            gesture,
            force: Some(force),
            scope: BindingScope::Global,
            action: ActionDefinition::bare(ActionSelector::new("test", "noop")),
            priority: 0,
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
    fn two_bands_on_one_pad_and_gesture_do_not_conflict() {
        let bindings = [
            struck("soft", pad(0), Gesture::Tap, ForceBand::Soft),
            struck("hard", pad(0), Gesture::Tap, ForceBand::Hard),
        ];
        assert!(
            find_conflicts(&bindings).is_empty(),
            "different bands are different events"
        );
    }

    #[test]
    fn the_same_band_twice_on_one_pad_is_a_conflict() {
        let bindings = [
            struck("one", pad(0), Gesture::Tap, ForceBand::Hard),
            struck("two", pad(0), Gesture::Tap, ForceBand::Hard),
        ];
        assert_eq!(find_conflicts(&bindings).len(), 1);
    }

    #[test]
    fn a_banded_and_an_unbanded_binding_are_separated_by_precedence() {
        let bindings = [
            binding("plain", pad(0), BindingScope::Global, 0),
            struck("hard", pad(0), Gesture::Tap, ForceBand::Hard),
        ];
        assert!(find_conflicts(&bindings).is_empty());
    }

    #[test]
    fn a_sound_set_of_banded_bindings_has_no_force_faults() {
        let bindings = [
            struck("soft", pad(0), Gesture::Tap, ForceBand::Soft),
            struck("press", pad(1), Gesture::Press, ForceBand::Hard),
            struck("double", pad(2), Gesture::DoubleTap, ForceBand::Firm),
            binding("plain", pad(3), BindingScope::Global, 0),
        ];
        assert!(find_force_faults(&bindings).is_empty());
    }

    #[test]
    fn a_band_on_a_control_that_cannot_report_force_is_refused() {
        let bindings = [struck(
            "play",
            ControlId::Button(ButtonId::Play),
            Gesture::Tap,
            ForceBand::Hard,
        )];
        let faults = find_force_faults(&bindings);
        assert_eq!(faults.len(), 1);
        assert!(matches!(faults[0], ForceFault::NotAPad { .. }));
        assert!(faults[0].to_string().contains("only pads"));
    }

    #[test]
    fn a_band_on_a_gesture_with_no_strike_behind_it_is_refused() {
        for gesture in [Gesture::Hold, Gesture::Release, Gesture::PressHarder] {
            let bindings = [struck("late", pad(0), gesture, ForceBand::Hard)];
            let faults = find_force_faults(&bindings);
            assert_eq!(faults.len(), 1, "{gesture} should be refused a band");
            assert!(matches!(faults[0], ForceFault::NoStrike { .. }));
        }
    }

    #[test]
    fn a_band_may_not_be_all_that_stands_in_front_of_stopping_work() {
        for selector in [
            ActionSelector::new("session", "interrupt"),
            ActionSelector::new("session", "put_away"),
            ActionSelector::new("agent", "stop"),
            ActionSelector::new("agent", "cancel"),
            ActionSelector::new("workflow", "cancel"),
            ActionSelector::new("shell", "run"),
        ] {
            let mut binding = struck("stop", pad(0), Gesture::Tap, ForceBand::Hard);
            binding.action = ActionDefinition::bare(selector.clone());
            let faults = find_force_faults(&[binding]);
            assert_eq!(faults.len(), 1, "{selector} should be refused a band");
            assert!(matches!(faults[0], ForceFault::HaltsWork { .. }));
        }
    }

    #[test]
    fn an_unbanded_binding_may_still_stop_work() {
        let mut binding = binding("stop", pad(0), BindingScope::Global, 0);
        binding.action = ActionDefinition::bare(ActionSelector::new("session", "interrupt"));
        assert!(
            find_force_faults(&[binding]).is_empty(),
            "a pad bound plainly to interrupt is the operator's business"
        );
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
