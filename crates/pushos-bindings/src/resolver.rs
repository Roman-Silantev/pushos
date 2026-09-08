//! Deterministic binding resolution.
//!
//! Given a gesture and the current surface, exactly one binding wins, or none
//! does. Precedence is `workspace+page`, then `workspace`, then `page`, then
//! `global`, with the configured priority breaking ties inside a tier.

use pushos_domain::binding::{Binding, BindingKey};
use pushos_domain::context::SurfaceContext;
use pushos_domain::gesture::Gesture;

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
        self.resolve_gesture(table, event.control.into(), event.gesture, context)
    }

    /// Resolves a control and gesture directly, without a gesture event.
    ///
    /// Used by Studio to preview what a control currently does.
    pub fn resolve_gesture<'table>(
        self,
        table: &'table BindingTable,
        control: ControlLookup,
        gesture: Gesture,
        context: &SurfaceContext,
    ) -> Option<&'table Binding> {
        Self::keys_for(gesture)
            .into_iter()
            .flatten()
            .flat_map(|configured| table.candidates(BindingKey::new(control.0, configured)))
            .find(|binding| binding.scope.applies_to(context))
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
