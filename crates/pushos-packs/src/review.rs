//! What an operator is shown before a pack is installed.
//!
//! Nothing is written until they have seen this. A pack that asks for the
//! ability to deploy should look different on screen from one that draws a
//! page, and the difference should be visible without reading TOML.

use pushos_domain::pack::Pack;
use pushos_domain::permissions::{Permission, PermissionSet};

/// A pack, what it adds, and what it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// The pack under review.
    pub pack: Pack,
    /// Capabilities it needs that the operator has not already granted.
    ///
    /// The list that matters: what installing would change about what PushOS
    /// is permitted to do. Anything already granted is not news.
    pub granting: Vec<Permission>,
    /// Capabilities it needs that are already granted.
    pub already: Vec<Permission>,
    /// Agent providers it expects that are not configured.
    pub missing_providers: Vec<String>,
}

impl Review {
    /// Works out what installing this pack would mean.
    pub fn of(pack: Pack, granted: &PermissionSet, providers: &[String]) -> Self {
        let granting = pack
            .requires
            .permissions
            .iter()
            .copied()
            .filter(|permission| !granted.allows(*permission))
            .collect();
        let already = pack
            .requires
            .permissions
            .iter()
            .copied()
            .filter(|permission| granted.allows(*permission))
            .collect();

        let missing_providers = pack
            .requires
            .providers
            .iter()
            .filter(|wanted| !providers.iter().any(|had| had == *wanted))
            .cloned()
            .collect();

        Self {
            pack,
            granting,
            already,
            missing_providers,
        }
    }

    /// Whether installing would widen what PushOS is permitted to do.
    pub fn widens_permissions(&self) -> bool {
        !self.granting.is_empty()
    }

    /// Whether anything being granted cannot be undone from PushOS.
    ///
    /// The one an operator should stop and read. A pack that can deploy, send
    /// or spend is a different proposition from one that draws pages.
    pub fn grants_something_irreversible(&self) -> bool {
        self.granting
            .iter()
            .any(|permission| permission.is_irreversible())
    }

    /// Whether the pack would work as installed, or only partly.
    ///
    /// A missing agent provider is not a refusal: a role names providers in
    /// preference order and PushOS reports a missing one rather than failing.
    pub fn works_fully(&self) -> bool {
        self.missing_providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::PackId;
    use pushos_domain::pack::{Contents, Requirements};

    use super::*;

    fn pack(permissions: Vec<Permission>, providers: Vec<String>) -> Pack {
        Pack {
            id: PackId::new("seo"),
            name: "SEO".to_owned(),
            version: "1.0.0".to_owned(),
            description: None,
            author: None,
            license: None,
            requires: Requirements {
                permissions,
                providers,
                ..Requirements::default()
            },
            adds: Contents::default(),
        }
    }

    fn granted(permissions: &[Permission]) -> PermissionSet {
        let mut set = PermissionSet::empty();
        for permission in permissions {
            set.grant(*permission);
        }
        set
    }

    #[test]
    fn a_review_separates_what_is_new_from_what_is_already_allowed() {
        // What an operator needs to decide is what would change, not the whole
        // list again.
        let review = Review::of(
            pack(
                vec![Permission::ShellExecute, Permission::Network],
                Vec::new(),
            ),
            &granted(&[Permission::Network]),
            &[],
        );

        assert_eq!(review.granting, [Permission::ShellExecute]);
        assert_eq!(review.already, [Permission::Network]);
        assert!(review.widens_permissions());
    }

    #[test]
    fn a_pack_asking_for_nothing_new_changes_nothing() {
        let review = Review::of(
            pack(vec![Permission::Network], Vec::new()),
            &granted(&[Permission::Network]),
            &[],
        );
        assert!(!review.widens_permissions());
    }

    #[test]
    fn something_that_cannot_be_undone_is_called_out() {
        let review = Review::of(
            pack(vec![Permission::DeployProduction], Vec::new()),
            &granted(&[]),
            &[],
        );
        assert!(review.grants_something_irreversible());
    }

    #[test]
    fn a_permission_already_granted_is_not_called_out_again() {
        // It is not news, and burying the new thing in a list of old ones is
        // how a review stops being read.
        let review = Review::of(
            pack(vec![Permission::DeployProduction], Vec::new()),
            &granted(&[Permission::DeployProduction]),
            &[],
        );
        assert!(!review.grants_something_irreversible());
        assert!(!review.widens_permissions());
    }

    #[test]
    fn a_missing_agent_provider_is_reported_rather_than_refused() {
        // A role names providers in preference order, so one missing is worth
        // saying and is not a reason to refuse the pack.
        let review = Review::of(
            pack(Vec::new(), vec!["claude".to_owned(), "codex".to_owned()]),
            &granted(&[]),
            &["claude".to_owned()],
        );
        assert_eq!(review.missing_providers, ["codex"]);
        assert!(!review.works_fully());
    }
}
