//! Keeping an agent inside what its role allows.
//!
//! A role that may only read must not be handed an agent that can write. The
//! app-server takes a sandbox mode when a thread opens, which is the earliest
//! and strongest place to say so: the agent cannot use what it was never
//! given, whatever its own configuration says and whatever it is later asked
//! to do.
//!
//! PushOS never hands out `danger-full-access`. It exists in the protocol and
//! there is no permission that maps to it, because a role saying "write files"
//! is not the same as a role saying "ignore the sandbox", and the difference
//! is the whole point of having one.

use pushos_domain::permissions::{Permission, PermissionSet};

/// What the app-server should let a thread do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sandbox {
    /// Look, do not touch.
    ReadOnly,
    /// Change things inside the directory it was given.
    WorkspaceWrite,
}

impl Sandbox {
    /// The word the app-server uses.
    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }

    /// The narrowest sandbox that still lets a role do its job.
    ///
    /// A role that says nothing about permissions narrows nothing, which is
    /// what an unconfigured role has always meant; it is the roles that *do*
    /// say that must be held to it.
    pub(crate) fn for_role(permissions: Option<&PermissionSet>) -> Self {
        let Some(allowed) = permissions else {
            return Self::WorkspaceWrite;
        };

        // Running a command is how an agent writes when it cannot write
        // directly, so either one needs the same room.
        if allowed.allows(Permission::FilesystemWrite) || allowed.allows(Permission::ShellExecute) {
            Self::WorkspaceWrite
        } else {
            Self::ReadOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowing(permissions: &[Permission]) -> PermissionSet {
        let mut set = PermissionSet::empty();
        for permission in permissions {
            set.grant(*permission);
        }
        set
    }

    #[test]
    fn a_role_that_may_only_read_gets_a_sandbox_that_may_only_read() {
        let reader = allowing(&[Permission::FilesystemRead]);
        assert_eq!(Sandbox::for_role(Some(&reader)), Sandbox::ReadOnly);
        assert_eq!(Sandbox::for_role(Some(&reader)).slug(), "read-only");
    }

    #[test]
    fn a_role_that_may_write_or_run_things_gets_room_to() {
        for permission in [Permission::FilesystemWrite, Permission::ShellExecute] {
            let writer = allowing(&[Permission::FilesystemRead, permission]);
            assert_eq!(
                Sandbox::for_role(Some(&writer)),
                Sandbox::WorkspaceWrite,
                "{permission:?} needs room to work"
            );
        }
    }

    #[test]
    fn running_a_command_counts_as_writing() {
        // An agent that may run anything can write by running something that
        // writes, so a read-only sandbox for it would be a sandbox in name.
        let shell = allowing(&[Permission::ShellExecute]);
        assert_eq!(Sandbox::for_role(Some(&shell)), Sandbox::WorkspaceWrite);
    }

    #[test]
    fn a_role_that_says_nothing_narrows_nothing() {
        assert_eq!(Sandbox::for_role(None), Sandbox::WorkspaceWrite);
    }

    #[test]
    fn an_empty_permission_set_is_not_the_same_as_saying_nothing() {
        // Saying "this role may do nothing" is a decision; leaving it out is
        // not, and the two must not collapse into the same sandbox.
        let nothing = PermissionSet::empty();
        assert_eq!(Sandbox::for_role(Some(&nothing)), Sandbox::ReadOnly);
        assert_ne!(Sandbox::for_role(Some(&nothing)), Sandbox::for_role(None));
    }

    #[test]
    fn nothing_maps_to_ignoring_the_sandbox() {
        // Every permission there is, and still not full access: PushOS has no
        // way to ask for that and should not grow one by accident.
        let everything = allowing(&[
            Permission::FilesystemRead,
            Permission::FilesystemWrite,
            Permission::ShellExecute,
            Permission::Network,
            Permission::GitPush,
            Permission::DeployProduction,
        ]);
        assert_eq!(
            Sandbox::for_role(Some(&everything)),
            Sandbox::WorkspaceWrite
        );
        for sandbox in [Sandbox::ReadOnly, Sandbox::WorkspaceWrite] {
            assert_ne!(sandbox.slug(), "danger-full-access");
        }
    }
}
