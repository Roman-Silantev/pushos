//! What an agent's tool does, as far as its role's permissions are concerned.
//!
//! A role that lists its permissions is saying what its agent may do, not what
//! PushOS may do on its behalf: the markets role reads and searches, and never
//! runs a command. Every agent describes each tool it wants to use in its own
//! words, so this is the shared vocabulary those words are sorted into, and
//! the one place that says which permission each kind of use needs.

use crate::permissions::{Permission, PermissionSet};

/// What a tool call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolUse {
    /// Reads or searches files.
    Reading,
    /// Creates, changes, moves or deletes files.
    Writing,
    /// Runs a command or code.
    Running,
    /// Reaches the network for data.
    Fetching,
    /// Anything else: thinking, planning, switching mode, or a tool the agent
    /// did not describe.
    Other,
}

impl ToolUse {
    /// The permission this use needs, if any one does.
    ///
    /// `None` for a use no permission describes. Those are not refused on a
    /// role's behalf; they reach the operator as questions, as they always
    /// have, because refusing what cannot be named would be guessing.
    pub const fn needs(self) -> Option<Permission> {
        match self {
            Self::Reading => Some(Permission::FilesystemRead),
            Self::Writing => Some(Permission::FilesystemWrite),
            Self::Running => Some(Permission::ShellExecute),
            Self::Fetching => Some(Permission::Network),
            Self::Other => None,
        }
    }

    /// Whether a role with these permissions may do this.
    ///
    /// A role that lists no permissions narrows nothing, and leaves the agent's
    /// own settings in charge.
    pub fn allowed_by(self, permissions: Option<&PermissionSet>) -> bool {
        match (permissions, self.needs()) {
            (Some(permissions), Some(needed)) => permissions.allows(needed),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn researcher() -> PermissionSet {
        [Permission::FilesystemRead, Permission::Network]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_role_may_do_what_it_lists_and_nothing_else_a_permission_describes() {
        let researcher = researcher();
        assert!(ToolUse::Reading.allowed_by(Some(&researcher)));
        assert!(ToolUse::Fetching.allowed_by(Some(&researcher)));
        assert!(!ToolUse::Running.allowed_by(Some(&researcher)));
        assert!(!ToolUse::Writing.allowed_by(Some(&researcher)));
    }

    #[test]
    fn a_role_listing_nothing_may_do_nothing_a_permission_describes() {
        let nothing = PermissionSet::empty();
        for use_ in [
            ToolUse::Reading,
            ToolUse::Writing,
            ToolUse::Running,
            ToolUse::Fetching,
        ] {
            assert!(!use_.allowed_by(Some(&nothing)), "{use_:?}");
        }
    }

    #[test]
    fn a_role_that_does_not_say_narrows_nothing() {
        assert!(ToolUse::Running.allowed_by(None));
        assert!(ToolUse::Writing.allowed_by(None));
    }

    #[test]
    fn a_use_no_permission_describes_is_left_to_the_operator() {
        assert!(ToolUse::Other.allowed_by(Some(&PermissionSet::empty())));
    }
}
