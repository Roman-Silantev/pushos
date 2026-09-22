//! Deterministic binding resolution.
//!
//! Given a gesture and the current surface, exactly one binding wins, or none
//! does. Precedence is `workspace+page`, then `workspace`, then `page`, then
//! `global`; within a tier a binding narrowed to a force band beats one that
//! accepts any force, and the configured priority breaks what is left.

use pushos_domain::binding::{Binding, BindingKey};
use pushos_domain::context::SurfaceContext;
use pushos_domain::gesture::{ForceBand, Gesture};

use crate::gesture::GestureEvent;
use crate::table::BindingTable;

/// Picks the binding that a gesture should trigger.
///
/// The resolver holds no state of its own. It reads a table and a context, both
/// supplied by the caller, which keeps resolution trivially testable and free
/// of hidden precedence.
#[derive(Debug, Clone, Copy, Default)]
pub struct BindingResolver;

impl BindingResolver {
    /// Builds a resolver.
    pub const fn new() -> Self {
        Self
    }

    /// Resolves a recognised gesture against the current surface.
    ///
    /// Returns `None` when the control has no binding that applies here, which
    /// is a normal outcome rather than an error.
    pub fn resolve<'table>(
        self,
        table: &'table BindingTable,
        event: &GestureEvent,
        context: &SurfaceContext,
    ) -> Option<&'table Binding> {
        self.resolve_struck(
            table,
            event.control.into(),
            event.gesture,
            event.detail.force(),
            context,
        )
    }

    /// Resolves a control and gesture directly, without a gesture event.
    ///
    /// Used by Studio and the display to preview what a control currently
    /// does. Force-banded bindings are skipped, because a preview describes
    /// what an ordinary press does rather than what one particular strike
    /// would have done.
    pub fn resolve_gesture<'table>(
        self,
        table: &'table BindingTable,
        control: ControlLookup,
        gesture: Gesture,
        context: &SurfaceContext,
    ) -> Option<&'table Binding> {
        self.resolve_struck(table, control, gesture, None, context)
    }

    /// Resolves a gesture that was struck with a known force.
    ///
    /// Candidates are already ordered most specific first, so the first one
    /// whose scope applies and whose band matches is the winner. A binding
    /// with no band accepts any strike, which is what every binding written
    /// before force existed means.
    pub fn resolve_struck<'table>(
        self,
        table: &'table BindingTable,
        control: ControlLookup,
        gesture: Gesture,
        struck: Option<ForceBand>,
        context: &SurfaceContext,
    ) -> Option<&'table Binding> {
        Self::keys_for(gesture)
            .into_iter()
            .flatten()
            .flat_map(|configured| table.candidates(BindingKey::new(control.0, configured)))
            .find(|binding| binding.scope.applies_to(context) && binding.accepts_force(struck))
    }

    /// The binding keys that a produced gesture may satisfy, tried in order.
    ///
    /// A directional turn first looks for a binding written for that direction,
    /// then falls back to the general `turn` binding. This is the only widening
    /// rule in the system, and keeping it here means the table stays a plain
    /// index.
    const fn keys_for(produced: Gesture) -> [Option<Gesture>; 2] {
        match produced {
            Gesture::TurnLeft => [Some(Gesture::TurnLeft), Some(Gesture::Turn)],
            Gesture::TurnRight => [Some(Gesture::TurnRight), Some(Gesture::Turn)],
            other => [Some(other), None],
        }
    }
}

/// A control being looked up, wrapped so the two identifier arguments of
/// [`BindingResolver::resolve_gesture`] cannot be transposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlLookup(pub pushos_domain::controls::ControlId);

impl From<pushos_domain::controls::ControlId> for ControlLookup {
    fn from(control: pushos_domain::controls::ControlId) -> Self {
        Self(control)
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector};
    use pushos_domain::binding::BindingScope;
    use pushos_domain::controls::{ControlId, EncoderId, PadIndex};

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    fn binding(id: &str, control: ControlId, gesture: Gesture, scope: BindingScope) -> Binding {
        Binding {
            id: id.into(),
            control,
            gesture,
            force: None,
            scope,
            action: ActionDefinition::bare(ActionSelector::new("test", id)),
            priority: 0,
            label: None,
        }
    }

    fn resolve<'t>(
        table: &'t BindingTable,
        control: ControlId,
        gesture: Gesture,
        context: &SurfaceContext,
    ) -> Option<&'t Binding> {
        BindingResolver::new().resolve_gesture(table, control.into(), gesture, context)
    }

    #[test]
    fn the_most_specific_applicable_scope_wins() {
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
            binding(
                "both",
                pad(0),
                Gesture::Tap,
                BindingScope::WorkspacePage {
                    workspace: "syd".into(),
                    page: "dev".into(),
                },
            ),
        ]);

        let full = SurfaceContext::empty().on_page("dev").in_workspace("syd");
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &full)
                .unwrap()
                .id
                .as_str(),
            "both"
        );

        let page_only = SurfaceContext::empty().on_page("dev");
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &page_only)
                .unwrap()
                .id
                .as_str(),
            "page"
        );

        let workspace_only = SurfaceContext::empty().in_workspace("syd");
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &workspace_only)
                .unwrap()
                .id
                .as_str(),
            "workspace"
        );

        let nothing = SurfaceContext::empty();
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &nothing)
                .unwrap()
                .id
                .as_str(),
            "global"
        );
    }

    #[test]
    fn a_more_specific_binding_for_another_context_is_skipped_entirely() {
        let table = BindingTable::new([
            binding("global", pad(0), Gesture::Tap, BindingScope::Global),
            binding(
                "other_page",
                pad(0),
                Gesture::Tap,
                BindingScope::Page("music".into()),
            ),
        ]);

        let context = SurfaceContext::empty().on_page("dev");
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &context)
                .unwrap()
                .id
                .as_str(),
            "global"
        );
    }

    #[test]
    fn a_gesture_with_no_binding_resolves_to_nothing() {
        let table = BindingTable::new([binding("tap", pad(0), Gesture::Tap, BindingScope::Global)]);
        assert!(resolve(&table, pad(0), Gesture::Hold, &SurfaceContext::empty()).is_none());
        assert!(resolve(&table, pad(1), Gesture::Tap, &SurfaceContext::empty()).is_none());
    }

    #[test]
    fn a_directional_turn_prefers_its_own_binding_over_the_general_one() {
        let encoder = ControlId::Encoder(EncoderId::Track(0));
        let table = BindingTable::new([
            binding("any", encoder, Gesture::Turn, BindingScope::Global),
            binding("left", encoder, Gesture::TurnLeft, BindingScope::Global),
        ]);
        let context = SurfaceContext::empty();

        assert_eq!(
            resolve(&table, encoder, Gesture::TurnLeft, &context)
                .unwrap()
                .id
                .as_str(),
            "left"
        );
        assert_eq!(
            resolve(&table, encoder, Gesture::TurnRight, &context)
                .unwrap()
                .id
                .as_str(),
            "any"
        );
    }

    #[test]
    fn a_directional_binding_never_answers_the_opposite_direction() {
        let encoder = ControlId::Encoder(EncoderId::Track(0));
        let table = BindingTable::new([binding(
            "left",
            encoder,
            Gesture::TurnLeft,
            BindingScope::Global,
        )]);
        assert!(
            resolve(
                &table,
                encoder,
                Gesture::TurnRight,
                &SurfaceContext::empty()
            )
            .is_none()
        );
    }

    fn struck<'t>(
        table: &'t BindingTable,
        control: ControlId,
        gesture: Gesture,
        force: ForceBand,
        context: &SurfaceContext,
    ) -> Option<&'t Binding> {
        BindingResolver::new().resolve_struck(table, control.into(), gesture, Some(force), context)
    }

    fn banded(id: &str, control: ControlId, force: ForceBand) -> Binding {
        let mut binding = binding(id, control, Gesture::Tap, BindingScope::Global);
        binding.force = Some(force);
        binding
    }

    #[test]
    fn a_banded_binding_answers_only_its_own_band() {
        let table = BindingTable::new([banded("hard", pad(0), ForceBand::Hard)]);
        let context = SurfaceContext::empty();

        assert_eq!(
            struck(&table, pad(0), Gesture::Tap, ForceBand::Hard, &context)
                .unwrap()
                .id
                .as_str(),
            "hard"
        );
        assert!(struck(&table, pad(0), Gesture::Tap, ForceBand::Soft, &context).is_none());
        assert!(struck(&table, pad(0), Gesture::Tap, ForceBand::Firm, &context).is_none());
    }

    #[test]
    fn a_band_wins_over_the_plain_binding_it_narrows() {
        let table = BindingTable::new([
            binding("plain", pad(0), Gesture::Tap, BindingScope::Global),
            banded("hard", pad(0), ForceBand::Hard),
        ]);
        let context = SurfaceContext::empty();

        assert_eq!(
            struck(&table, pad(0), Gesture::Tap, ForceBand::Hard, &context)
                .unwrap()
                .id
                .as_str(),
            "hard"
        );
        assert_eq!(
            struck(&table, pad(0), Gesture::Tap, ForceBand::Firm, &context)
                .unwrap()
                .id
                .as_str(),
            "plain",
            "an ordinary strike still gets the binding that was always there"
        );
    }

    #[test]
    fn a_page_binding_beats_a_global_band_however_hard_the_pad_was_hit() {
        let mut page = binding(
            "page",
            pad(0),
            Gesture::Tap,
            BindingScope::Page("dev".into()),
        );
        page.force = None;
        let table = BindingTable::new([banded("global_hard", pad(0), ForceBand::Hard), page]);

        let context = SurfaceContext::empty().on_page("dev");
        assert_eq!(
            struck(&table, pad(0), Gesture::Tap, ForceBand::Hard, &context)
                .unwrap()
                .id
                .as_str(),
            "page",
            "scope dominates force; the operator moved to that page on purpose"
        );
    }

    #[test]
    fn an_unbanded_binding_still_answers_a_gesture_that_carries_no_force() {
        // Every binding written before force existed, and every encoder turn.
        let table =
            BindingTable::new([binding("plain", pad(0), Gesture::Tap, BindingScope::Global)]);
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &SurfaceContext::empty())
                .unwrap()
                .id
                .as_str(),
            "plain"
        );
    }

    #[test]
    fn a_preview_describes_the_ordinary_press_rather_than_a_banded_one() {
        let table = BindingTable::new([
            binding("plain", pad(0), Gesture::Tap, BindingScope::Global),
            banded("hard", pad(0), ForceBand::Hard),
        ]);
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &SurfaceContext::empty())
                .unwrap()
                .id
                .as_str(),
            "plain",
            "the display must not advertise what only a hard strike would do"
        );
    }

    #[test]
    fn three_bands_on_one_pad_each_answer_their_own_strike() {
        let table = BindingTable::new([
            banded("soft", pad(0), ForceBand::Soft),
            banded("firm", pad(0), ForceBand::Firm),
            banded("hard", pad(0), ForceBand::Hard),
        ]);
        let context = SurfaceContext::empty();

        for (band, expected) in [
            (ForceBand::Soft, "soft"),
            (ForceBand::Firm, "firm"),
            (ForceBand::Hard, "hard"),
        ] {
            assert_eq!(
                struck(&table, pad(0), Gesture::Tap, band, &context)
                    .unwrap()
                    .id
                    .as_str(),
                expected
            );
        }
    }

    #[test]
    fn a_scoped_binding_outranks_a_higher_priority_global_one() {
        let mut global = binding("global", pad(0), Gesture::Tap, BindingScope::Global);
        global.priority = u16::MAX;
        let page = binding(
            "page",
            pad(0),
            Gesture::Tap,
            BindingScope::Page("dev".into()),
        );

        let table = BindingTable::new([global, page]);
        let context = SurfaceContext::empty().on_page("dev");
        assert_eq!(
            resolve(&table, pad(0), Gesture::Tap, &context)
                .unwrap()
                .id
                .as_str(),
            "page"
        );
    }
}
