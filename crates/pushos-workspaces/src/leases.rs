//! Who is working in which tree.
//!
//! The one rule this exists to keep: two coding agents must never be handed the
//! same working tree at the same time. Everything else about worktrees is
//! convenience; this is correctness, because two agents editing one checkout
//! produce a conflict nobody can untangle after the fact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pushos_domain::ids::{AgentId, WorkspaceId};

/// Who holds a tree: one role in one project.
///
/// A role has one live session at a time, so keying on the role gives the
/// guarantee that matters while letting the tree outlive any one session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Holder(String);

impl Holder {
    /// The role working in a project.
    pub fn new(workspace: &WorkspaceId, agent: &AgentId) -> Self {
        Self(format!("{workspace}/{agent}"))
    }

    /// How it reads in a message and in a branch name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The trees currently spoken for.
#[derive(Debug, Default)]
pub struct WorktreeLeases {
    held: HashMap<Holder, PathBuf>,
}

impl WorktreeLeases {
    /// Builds a book with nothing let out.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whoever holds a tree, if anyone does.
    pub fn holder(&self, path: &Path) -> Option<&Holder> {
        self.held
            .iter()
            .find(|(_, held)| held.as_path() == path)
            .map(|(session, _)| session)
    }

    /// Whether a tree is free to hand out.
    pub fn is_free(&self, path: &Path) -> bool {
        self.holder(path).is_none()
    }

    /// The tree a holder has, if it has one.
    pub fn held_by(&self, session: &Holder) -> Option<&Path> {
        self.held.get(session).map(PathBuf::as_path)
    }

    /// Records that a holder has a tree.
    ///
    /// One that already had one gives that up first: a role works in one place,
    /// and holding two would make the count wrong for both.
    pub fn take(&mut self, session: &Holder, path: impl Into<PathBuf>) {
        self.held.insert(session.clone(), path.into());
    }

    /// Gives back whatever a holder had.
    ///
    /// The tree itself is left alone: it may hold work nobody has committed,
    /// and deleting that to tidy up would be the worst thing PushOS could do.
    pub fn release(&mut self, session: &Holder) -> Option<PathBuf> {
        self.held.remove(session)
    }

    /// How many trees are spoken for.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether anything is spoken for.
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(agent: &str) -> Holder {
        Holder::new(&WorkspaceId::new("project"), &AgentId::new(agent))
    }

    #[test]
    fn a_tree_nobody_holds_is_free() {
        let leases = WorktreeLeases::new();
        assert!(leases.is_free(Path::new("/trees/one")));
        assert!(leases.is_empty());
    }

    #[test]
    fn a_tree_that_is_taken_is_not_free_and_names_who_has_it() {
        // The whole point: the next agent must be able to find out.
        let mut leases = WorktreeLeases::new();
        leases.take(&holder("s1"), "/trees/one");

        assert!(!leases.is_free(Path::new("/trees/one")));
        assert_eq!(leases.holder(Path::new("/trees/one")), Some(&holder("s1")));
        assert!(leases.is_free(Path::new("/trees/two")));
    }

    #[test]
    fn releasing_frees_the_tree_for_the_next_holder() {
        let mut leases = WorktreeLeases::new();
        leases.take(&holder("s1"), "/trees/one");

        assert_eq!(
            leases.release(&holder("s1")),
            Some(PathBuf::from("/trees/one"))
        );
        assert!(leases.is_free(Path::new("/trees/one")));
    }

    #[test]
    fn releasing_something_never_held_changes_nothing() {
        let mut leases = WorktreeLeases::new();
        leases.take(&holder("s1"), "/trees/one");

        assert_eq!(leases.release(&holder("s2")), None);
        assert_eq!(leases.len(), 1);
    }

    #[test]
    fn a_session_holds_one_tree_at_a_time() {
        let mut leases = WorktreeLeases::new();
        leases.take(&holder("s1"), "/trees/one");
        leases.take(&holder("s1"), "/trees/two");

        assert_eq!(leases.len(), 1);
        assert_eq!(leases.held_by(&holder("s1")), Some(Path::new("/trees/two")));
        assert!(
            leases.is_free(Path::new("/trees/one")),
            "the tree it gave up is free again"
        );
    }

    #[test]
    fn a_session_asking_again_gets_what_it_already_has() {
        let mut leases = WorktreeLeases::new();
        leases.take(&holder("s1"), "/trees/one");
        assert_eq!(leases.held_by(&holder("s1")), Some(Path::new("/trees/one")));
    }
}
