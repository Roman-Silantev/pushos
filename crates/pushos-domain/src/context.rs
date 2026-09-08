//! The context that gives a control its current meaning.

use serde::{Deserialize, Serialize};

use crate::ids::{PageId, SessionId, WorkspaceId};

/// The surface state that binding resolution and action execution read.
///
/// This is an immutable snapshot. Readers never hold a lock on live state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceContext {
    /// The page currently in effect.
    pub page: Option<PageId>,
    /// The workspace currently in effect.
    pub workspace: Option<WorkspaceId>,
    /// The session the operator most recently selected.
    pub session: Option<SessionId>,
    /// Whether Shift was held when the gesture completed.
    pub shift_held: bool,
}

impl SurfaceContext {
    /// A context with nothing selected.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Returns a copy on the given page.
    #[must_use]
    pub fn on_page(mut self, page: impl Into<PageId>) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Returns a copy in the given workspace.
    #[must_use]
    pub fn in_workspace(mut self, workspace: impl Into<WorkspaceId>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Returns a copy with the given shift state.
    #[must_use]
    pub fn with_shift(mut self, held: bool) -> Self {
        self.shift_held = held;
        self
    }
}

/// The context a workspace restores when the operator returns to it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMemory {
    /// The page that was in effect when the workspace was last left.
    pub last_page: Option<PageId>,
    /// The session that was selected.
    pub selected_session: Option<SessionId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_compose_without_clobbering_each_other() {
        let context = SurfaceContext::empty()
            .on_page("development")
            .in_workspace("sydclaw")
            .with_shift(true);

        assert_eq!(
            context.page.as_ref().map(PageId::as_str),
            Some("development")
        );
        assert_eq!(
            context.workspace.as_ref().map(WorkspaceId::as_str),
            Some("sydclaw")
        );
        assert!(context.shift_held);
        assert!(context.session.is_none());
    }
}
