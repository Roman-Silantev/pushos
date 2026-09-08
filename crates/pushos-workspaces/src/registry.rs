//! Every project PushOS knows about.
//!
//! Read-only: what a project is comes from configuration the operator wrote,
//! and PushOS never edits it. What a project *was doing* is elsewhere.

use pushos_domain::ids::WorkspaceId;
use pushos_domain::workspace::{Workspace, WorkspaceTarget};

/// The configured workspaces, in the order they were written.
///
/// Order matters: `next` and `previous` step through it, and an operator who
/// wrote their projects in an order expects to move through them in that order.
#[derive(Debug, Default)]
pub struct WorkspaceRegistry {
    workspaces: Vec<Workspace>,
}

impl WorkspaceRegistry {
    /// Builds a registry from configured workspaces.
    ///
    /// A repeated identity keeps the first, which is what the configuration
    /// validator has already refused, so reaching here means something else
    /// built the list.
    pub fn new(workspaces: impl IntoIterator<Item = Workspace>) -> Self {
        let mut kept: Vec<Workspace> = Vec::new();
        for workspace in workspaces {
            if !kept.iter().any(|existing| existing.id == workspace.id) {
                kept.push(workspace);
            }
        }
        Self { workspaces: kept }
    }

    /// Whether any project is configured.
    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }

    /// How many are configured.
    pub fn len(&self) -> usize {
        self.workspaces.len()
    }

    /// Every workspace, in configured order.
    pub fn all(&self) -> &[Workspace] {
        &self.workspaces
    }

    /// One workspace by identity.
    pub fn get(&self, id: &WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == *id)
    }

    /// Works out which workspace a target names, given where the operator is.
    ///
    /// `Ok(None)` means "no workspace", which `none` says deliberately.
    pub fn resolve(
        &self,
        target: &WorkspaceTarget,
        current: Option<&WorkspaceId>,
    ) -> Result<Option<&Workspace>, UnknownWorkspace> {
        match target {
            WorkspaceTarget::None => Ok(None),
            WorkspaceTarget::Named(id) => self
                .get(id)
                .map(Some)
                .ok_or_else(|| UnknownWorkspace(id.clone())),
            WorkspaceTarget::Next => Ok(self.step(current, 1)),
            WorkspaceTarget::Previous => Ok(self.step(current, -1)),
        }
    }

    /// Steps through the list, wrapping at either end.
    ///
    /// Stepping from nowhere lands on the first, so a single pad bound to
    /// `next` is enough to get into a project from a cold start.
    fn step(&self, current: Option<&WorkspaceId>, by: isize) -> Option<&Workspace> {
        if self.workspaces.is_empty() {
            return None;
        }

        let Some(here) = current.and_then(|id| self.position(id)) else {
            return self.workspaces.first();
        };

        let count = self.workspaces.len();
        // Rotated rather than added, so stepping back from the first wraps to
        // the last instead of going negative.
        let next = (here + count).saturating_add_signed(by) % count;
        self.workspaces.get(next)
    }

    fn position(&self, id: &WorkspaceId) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| workspace.id == *id)
    }
}

/// A target that named a workspace nothing configures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no workspace called `{0}` is configured")]
pub struct UnknownWorkspace(pub WorkspaceId);

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> WorkspaceRegistry {
        WorkspaceRegistry::new([
            Workspace::new("sydclaw", "Sydclaw", "/tmp/sydclaw"),
            Workspace::new("pushos", "PushOS", "/tmp/pushos"),
            Workspace::new("website", "Website", "/tmp/website"),
        ])
    }

    fn named(id: &str) -> WorkspaceTarget {
        WorkspaceTarget::Named(WorkspaceId::new(id))
    }

    fn resolved<'a>(
        registry: &'a WorkspaceRegistry,
        target: &WorkspaceTarget,
        current: Option<&str>,
    ) -> Option<&'a str> {
        let here = current.map(WorkspaceId::new);
        registry
            .resolve(target, here.as_ref())
            .expect("the target names something configured")
            .map(|workspace| workspace.id.as_str())
    }

    #[test]
    fn a_name_finds_the_workspace_it_names() {
        assert_eq!(
            resolved(&registry(), &named("pushos"), None),
            Some("pushos")
        );
    }

    #[test]
    fn a_name_nothing_configures_is_refused_rather_than_ignored() {
        // Silently staying put would leave the operator pressing a pad that
        // appears to do nothing.
        let error = registry()
            .resolve(&named("nowhere"), None)
            .expect_err("nothing configures it");
        assert_eq!(error.0.as_str(), "nowhere");
    }

    #[test]
    fn next_steps_forward_through_the_configured_order() {
        let registry = registry();
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Next, Some("sydclaw")),
            Some("pushos")
        );
    }

    #[test]
    fn next_wraps_at_the_end_and_previous_wraps_at_the_start() {
        let registry = registry();
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Next, Some("website")),
            Some("sydclaw")
        );
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Previous, Some("sydclaw")),
            Some("website")
        );
    }

    #[test]
    fn stepping_from_nowhere_lands_on_the_first() {
        // One pad bound to `next` is then enough to get into a project from a
        // cold start.
        let registry = registry();
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Next, None),
            Some("sydclaw")
        );
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Previous, None),
            Some("sydclaw")
        );
    }

    #[test]
    fn stepping_from_a_workspace_that_is_gone_lands_on_the_first() {
        // Configuration can be reloaded while the operator is in a project the
        // edit removed.
        let registry = registry();
        assert_eq!(
            resolved(&registry, &WorkspaceTarget::Next, Some("deleted")),
            Some("sydclaw")
        );
    }

    #[test]
    fn none_means_no_workspace_rather_than_no_change() {
        assert_eq!(
            resolved(&registry(), &WorkspaceTarget::None, Some("pushos")),
            None
        );
    }

    #[test]
    fn stepping_with_nothing_configured_finds_nothing() {
        let empty = WorkspaceRegistry::new([]);
        assert!(empty.is_empty());
        assert_eq!(resolved(&empty, &WorkspaceTarget::Next, None), None);
    }

    #[test]
    fn one_workspace_steps_to_itself() {
        let single = WorkspaceRegistry::new([Workspace::new("only", "Only", "/tmp/only")]);
        assert_eq!(
            resolved(&single, &WorkspaceTarget::Next, Some("only")),
            Some("only")
        );
        assert_eq!(
            resolved(&single, &WorkspaceTarget::Previous, Some("only")),
            Some("only")
        );
    }

    #[test]
    fn a_repeated_identity_keeps_the_first() {
        let repeated = WorkspaceRegistry::new([
            Workspace::new("one", "First", "/tmp/first"),
            Workspace::new("one", "Second", "/tmp/second"),
        ]);
        assert_eq!(repeated.len(), 1);
        assert_eq!(repeated.all()[0].name, "First");
    }
}
