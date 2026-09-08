//! Least-privilege capabilities.
//!
//! A permission is granted, never assumed. When two policies disagree the
//! stricter one wins, so adding a policy layer can only narrow access.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A capability that must be granted before an action may use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Permission {
    /// Read files.
    FilesystemRead,
    /// Write files.
    FilesystemWrite,
    /// Run a subprocess.
    ShellExecute,
    /// Reach the network.
    Network,
    /// Create commits.
    GitCommit,
    /// Push to a remote.
    GitPush,
    /// Merge branches.
    GitMerge,
    /// Deploy to a preview environment.
    DeployPreview,
    /// Deploy to production.
    DeployProduction,
    /// Launch or focus an application.
    ApplicationLaunch,
    /// Control media playback.
    MediaControl,
    /// Run an Apple Shortcut.
    ShortcutsExecute,
    /// Send something outside the machine.
    ExternalSend,
    /// Read financial data.
    FinancialRead,
    /// Write financial data.
    FinancialWrite,
}

impl Permission {
    /// Whether the permission may cause an effect that cannot be undone from
    /// PushOS, and therefore always needs deliberate confirmation.
    pub const fn is_irreversible(self) -> bool {
        matches!(
            self,
            Self::DeployProduction | Self::ExternalSend | Self::FinancialWrite | Self::GitPush
        )
    }
}

/// A set of granted capabilities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeSet<Permission>);

impl PermissionSet {
    /// A set granting nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether the capability is granted.
    pub fn allows(&self, permission: Permission) -> bool {
        self.0.contains(&permission)
    }

    /// Grants a capability.
    pub fn grant(&mut self, permission: Permission) {
        self.0.insert(permission);
    }

    /// The strictest reading of two policies: only what both allow.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }

    /// Checks a requirement, naming the missing capability on failure.
    pub fn require(&self, permission: Permission) -> Result<(), PermissionDenied> {
        if self.allows(permission) {
            Ok(())
        } else {
            Err(PermissionDenied { permission })
        }
    }

    /// Iterates the granted capabilities in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        self.0.iter().copied()
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<T: IntoIterator<Item = Permission>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// An action asked for a capability it was not granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("permission `{permission:?}` is not granted")]
pub struct PermissionDenied {
    /// The capability that was missing.
    pub permission: Permission,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_granted_by_default() {
        let set = PermissionSet::empty();
        assert!(!set.allows(Permission::ShellExecute));
        assert!(set.require(Permission::ShellExecute).is_err());
    }

    #[test]
    fn the_stricter_policy_wins_when_two_are_combined() {
        let broad = PermissionSet::from_iter([
            Permission::ShellExecute,
            Permission::Network,
            Permission::MediaControl,
        ]);
        let narrow = PermissionSet::from_iter([Permission::MediaControl, Permission::Network]);

        let effective = broad.intersect(&narrow);
        assert!(effective.allows(Permission::MediaControl));
        assert!(effective.allows(Permission::Network));
        assert!(!effective.allows(Permission::ShellExecute));
    }

    #[test]
    fn irreversible_capabilities_are_marked() {
        assert!(Permission::DeployProduction.is_irreversible());
        assert!(Permission::FinancialWrite.is_irreversible());
        assert!(!Permission::MediaControl.is_irreversible());
        assert!(!Permission::FilesystemRead.is_irreversible());
    }
}
