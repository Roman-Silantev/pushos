//! Bindings and their precedence rules.
//!
//! Precedence is fixed and total: `workspace+page`, then `workspace`, then
//! `page`, then `global`. Nothing may add a hidden tier, because an operator
//! must be able to predict what a pad does by reading the configuration.

use serde::{Deserialize, Serialize};

use crate::action::ActionDefinition;
use crate::context::SurfaceContext;
use crate::controls::ControlId;
use crate::gesture::Gesture;
use crate::ids::{BindingId, PageId, WorkspaceId};

/// Where a binding applies.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingScope {
    /// Applies everywhere.
    Global,
    /// Applies on one page, in any workspace.
    Page(PageId),
    /// Applies in one workspace, on any page.
    Workspace(WorkspaceId),
    /// Applies only on one page within one workspace.
    WorkspacePage {
        /// The workspace that must be active.
        workspace: WorkspaceId,
        /// The page that must be active.
        page: PageId,
    },
}

impl BindingScope {
    /// How specific the scope is. Higher wins.
    ///
    /// The values are contiguous from zero so that the resolver can compare
    /// them directly, and so that adding a tier is a visible change here.
    pub const fn specificity(&self) -> ScopeSpecificity {
        match self {
            Self::Global => ScopeSpecificity::GLOBAL,
            Self::Page(_) => ScopeSpecificity::PAGE,
            Self::Workspace(_) => ScopeSpecificity::WORKSPACE,
            Self::WorkspacePage { .. } => ScopeSpecificity::WORKSPACE_PAGE,
        }
    }

    /// Whether the scope's conditions are satisfied by the current surface.
    pub fn applies_to(&self, context: &SurfaceContext) -> bool {
        match self {
            Self::Global => true,
            Self::Page(page) => context.page.as_ref() == Some(page),
            Self::Workspace(workspace) => context.workspace.as_ref() == Some(workspace),
            Self::WorkspacePage { workspace, page } => {
                context.workspace.as_ref() == Some(workspace) && context.page.as_ref() == Some(page)
            }
        }
    }
}

/// The precedence rank of a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeSpecificity(u8);

impl ScopeSpecificity {
    /// Rank of [`BindingScope::Global`].
    pub const GLOBAL: Self = Self(0);
    /// Rank of [`BindingScope::Page`].
    pub const PAGE: Self = Self(1);
    /// Rank of [`BindingScope::Workspace`].
    pub const WORKSPACE: Self = Self(2);
    /// Rank of [`BindingScope::WorkspacePage`].
    pub const WORKSPACE_PAGE: Self = Self(3);
}

/// One configured mapping from a physical gesture to an action.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Stable identity, used in logs and by Studio.
    pub id: BindingId,
    /// The control the operator touches.
    pub control: ControlId,
    /// The gesture that triggers it.
    pub gesture: Gesture,
    /// Where the binding applies.
    pub scope: BindingScope,
    /// What to run.
    pub action: ActionDefinition,
    /// Tie-break within a scope tier. Higher wins.
    pub priority: u16,
    /// Optional label shown on the display and in Studio.
    pub label: Option<String>,
}

impl Binding {
    /// The key a binding is indexed under.
    pub const fn key(&self) -> BindingKey {
        BindingKey {
            control: self.control,
            gesture: self.gesture,
        }
    }

    /// The ordering used to pick a winner among applicable bindings.
    ///
    /// Compared as a tuple so that scope always dominates priority, and so that
    /// two bindings can only tie if they are genuinely ambiguous.
    pub const fn precedence(&self) -> (ScopeSpecificity, u16) {
        (self.scope.specificity(), self.priority)
    }
}

/// The lookup key for a binding: what was touched, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingKey {
    /// The control.
    pub control: ControlId,
    /// The gesture.
    pub gesture: Gesture,
}

impl BindingKey {
    /// Builds a key.
    pub const fn new(control: ControlId, gesture: Gesture) -> Self {
        Self { control, gesture }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specificity_follows_the_documented_order() {
        let global = BindingScope::Global;
        let page = BindingScope::Page("development".into());
        let workspace = BindingScope::Workspace("sydclaw".into());
        let both = BindingScope::WorkspacePage {
            workspace: "sydclaw".into(),
            page: "development".into(),
        };

        assert!(global.specificity() < page.specificity());
        assert!(page.specificity() < workspace.specificity());
        assert!(workspace.specificity() < both.specificity());
    }

    #[test]
    fn scopes_only_apply_when_their_conditions_hold() {
        let context = SurfaceContext::empty()
            .on_page("development")
            .in_workspace("sydclaw");

        assert!(BindingScope::Global.applies_to(&context));
        assert!(BindingScope::Page("development".into()).applies_to(&context));
        assert!(BindingScope::Workspace("sydclaw".into()).applies_to(&context));
        assert!(
            BindingScope::WorkspacePage {
                workspace: "sydclaw".into(),
                page: "development".into(),
            }
            .applies_to(&context)
        );

        assert!(!BindingScope::Page("music".into()).applies_to(&context));
        assert!(!BindingScope::Workspace("other".into()).applies_to(&context));
    }

    #[test]
    fn a_page_scope_does_not_apply_when_no_page_is_active() {
        let context = SurfaceContext::empty();
        assert!(!BindingScope::Page("development".into()).applies_to(&context));
        assert!(BindingScope::Global.applies_to(&context));
    }
}
