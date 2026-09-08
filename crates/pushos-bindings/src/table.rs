//! The immutable binding index.
//!
//! A table is built once from validated configuration and then only read. A
//! configuration reload builds a whole new table and swaps it in, so a reader
//! never observes a half-applied change.

use std::cmp::Reverse;
use std::collections::HashMap;

use pushos_domain::binding::{Binding, BindingKey};
use pushos_domain::gesture::Gesture;

use crate::gesture::{GestureInterest, InterestKind};

/// Every binding in force, indexed by the gesture that triggers it.
#[derive(Debug, Default)]
pub struct BindingTable {
    by_key: HashMap<BindingKey, Vec<Binding>>,
    interest: GestureInterest,
}

impl BindingTable {
    /// Builds a table from validated bindings.
    ///
    /// Candidates sharing a key are sorted by precedence, most specific first,
    /// so resolution is a scan of an already-ordered list. Rejecting ambiguity
    /// is the validator's job; this type assumes its input is already sound.
    pub fn new(bindings: impl IntoIterator<Item = Binding>) -> Self {
        let mut by_key: HashMap<BindingKey, Vec<Binding>> = HashMap::new();
        let mut interest = GestureInterest::none();

        for binding in bindings {
            if let Some(kind) = Self::interest_kind(binding.gesture) {
                match kind {
                    InterestKind::DoubleTap => interest.watch_double_tap(binding.control),
                    InterestKind::Hold => interest.watch_hold(binding.control),
                }
            }
            by_key.entry(binding.key()).or_default().push(binding);
        }

        for candidates in by_key.values_mut() {
            candidates.sort_by_key(|binding| Reverse(binding.precedence()));
        }

        Self { by_key, interest }
    }

    /// The bindings registered for a key, most specific first.
    pub fn candidates(&self, key: BindingKey) -> &[Binding] {
        self.by_key.get(&key).map_or(&[], Vec::as_slice)
    }

    /// The controls whose gestures need timers under this configuration.
    pub fn gesture_interest(&self) -> &GestureInterest {
        &self.interest
    }

    /// How many bindings the table holds.
    pub fn len(&self) -> usize {
        self.by_key.values().map(Vec::len).sum()
    }

    /// Whether the table holds no bindings.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Every binding, in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = &Binding> {
        self.by_key.values().flatten()
    }

    const fn interest_kind(gesture: Gesture) -> Option<InterestKind> {
        match gesture {
            Gesture::DoubleTap => Some(InterestKind::DoubleTap),
            Gesture::Hold | Gesture::ShiftHold => Some(InterestKind::Hold),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector};
    use pushos_domain::binding::BindingScope;
    use pushos_domain::controls::{ControlId, PadIndex};

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    fn binding(id: &str, control: ControlId, gesture: Gesture, scope: BindingScope) -> Binding {
        Binding {
            id: id.into(),
            control,
            gesture,
            scope,
            action: ActionDefinition::bare(ActionSelector::new("test", "noop")),
            priority: 0,
            label: None,
        }
    }

    #[test]
    fn candidates_come_back_most_specific_first() {
        let table = BindingTable::new([
            binding("global", pad(0), Gesture::Tap, BindingScope::Global),
            binding(
                "page",
                pad(0),
                Gesture::Tap,
                BindingScope::Page("dev".into()),
            ),
            binding(
                "workspace",
                pad(0),
                Gesture::Tap,
                BindingScope::Workspace("syd".into()),
            ),
        ]);

        let ids: Vec<_> = table
            .candidates(BindingKey::new(pad(0), Gesture::Tap))
            .iter()
            .map(|b| b.id.to_string())
            .collect();
        assert_eq!(ids, ["workspace", "page", "global"]);
    }

    #[test]
    fn priority_breaks_ties_within_one_scope_tier() {
        let mut low = binding("low", pad(1), Gesture::Tap, BindingScope::Global);
        low.priority = 1;
        let mut high = binding("high", pad(1), Gesture::Tap, BindingScope::Global);
        high.priority = 9;

        let table = BindingTable::new([low, high]);
        let winner = &table.candidates(BindingKey::new(pad(1), Gesture::Tap))[0];
        assert_eq!(winner.id.as_str(), "high");
    }

    #[test]
    fn an_unbound_key_yields_no_candidates_rather_than_panicking() {
        let table = BindingTable::new([]);
        assert!(
            table
                .candidates(BindingKey::new(pad(0), Gesture::Tap))
                .is_empty()
        );
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn only_timed_gestures_register_an_interest() {
        let table = BindingTable::new([
            binding("tap", pad(2), Gesture::Tap, BindingScope::Global),
            binding("double", pad(3), Gesture::DoubleTap, BindingScope::Global),
            binding("hold", pad(4), Gesture::Hold, BindingScope::Global),
            binding(
                "shift_hold",
                pad(5),
                Gesture::ShiftHold,
                BindingScope::Global,
            ),
        ]);
        let interest = table.gesture_interest();

        assert!(!interest.wants_double_tap(pad(2)));
        assert!(!interest.wants_hold(pad(2)));
        assert!(interest.wants_double_tap(pad(3)));
        assert!(interest.wants_hold(pad(4)));
        assert!(interest.wants_hold(pad(5)));
    }
}
