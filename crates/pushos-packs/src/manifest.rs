//! The file at the top of a pack.
//!
//! Mirrors the TOML exactly and does nothing else. Turning it into something
//! PushOS can act on is a separate, fallible step, so a manifest that says
//! something impossible is refused with a message about the manifest rather
//! than half-applied.

use pushos_domain::ids::PackId;
use pushos_domain::pack::{Contents, Pack, Requirements};
use pushos_domain::permissions::Permission;
use serde::{Deserialize, Serialize};

use crate::error::PackError;

/// What a `pack.toml` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// What the pack is called on the command line.
    pub(crate) id: String,
    /// What it is called in a listing.
    pub(crate) name: String,
    /// Which version of it this is.
    pub(crate) version: String,
    /// What it is for.
    pub(crate) description: Option<String>,
    /// Who made it.
    pub(crate) author: Option<String>,
    /// Under what terms.
    pub(crate) license: Option<String>,
    /// The oldest PushOS it was written for.
    pub(crate) minimum_pushos_version: Option<String>,
    /// What it needs before it can be installed.
    #[serde(default)]
    pub(crate) requires: RequiresSection,
}

/// What a pack asks the operator for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequiresSection {
    /// Capabilities the pack cannot work without.
    #[serde(default)]
    pub(crate) permissions: Vec<String>,
    /// Capabilities it would use but works without.
    #[serde(default)]
    pub(crate) optional_permissions: Vec<String>,
    /// Agent providers it expects to be able to start.
    #[serde(default)]
    pub(crate) providers: Vec<String>,
}

impl Manifest {
    /// Turns a parsed manifest into a pack, or says why it cannot.
    pub(crate) fn into_pack(self, adds: Contents) -> Result<Pack, PackError> {
        if !is_usable(&self.id) {
            // The identity becomes a directory name under the operator's
            // configuration, so it has to be a name and nothing else.
            return Err(PackError::invalid(
                &self.id,
                "the id is not a usable name; use letters, digits, `-` or `_`",
            ));
        }
        if self.name.trim().is_empty() {
            return Err(PackError::invalid(&self.id, "the pack has no name"));
        }
        if self.version.trim().is_empty() {
            return Err(PackError::invalid(&self.id, "the pack has no version"));
        }

        let permissions = parse_permissions(&self.id, &self.requires.permissions)?;
        let optional = parse_permissions(&self.id, &self.requires.optional_permissions)?;

        Ok(Pack {
            id: PackId::new(&self.id),
            name: self.name,
            version: self.version,
            description: self.description,
            author: self.author,
            license: self.license,
            requires: Requirements {
                permissions,
                optional,
                providers: self.requires.providers,
                minimum_version: self.minimum_pushos_version,
            },
            adds,
        })
    }
}

/// Reads a list of capability names, naming the one that is wrong.
fn parse_permissions(pack: &str, written: &[String]) -> Result<Vec<Permission>, PackError> {
    written
        .iter()
        .map(|name| {
            name.parse::<Permission>().map_err(|_| {
                PackError::invalid(pack, format!("`{name}` is not a capability PushOS has"))
            })
        })
        .collect()
}

/// Whether a name can safely be a directory under the configuration root.
fn is_usable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            id: "seo".to_owned(),
            name: "SEO Growth Pack".to_owned(),
            version: "1.0.0".to_owned(),
            ..Manifest::default()
        }
    }

    #[test]
    fn a_manifest_becomes_a_pack() {
        let pack = manifest()
            .into_pack(Contents::default())
            .expect("a complete manifest");
        assert_eq!(pack.id.as_str(), "seo");
        assert_eq!(pack.version, "1.0.0");
    }

    #[test]
    fn an_id_that_could_escape_the_packs_directory_is_refused() {
        // The identity becomes a directory name under the operator's own
        // configuration, so this is the difference between installing a pack
        // and writing anywhere.
        for bad in ["..", "a/b", "", "with space", "../../etc"] {
            let mut manifest = manifest();
            manifest.id = bad.to_owned();
            assert!(
                manifest.into_pack(Contents::default()).is_err(),
                "`{bad}` should be refused"
            );
        }
    }

    #[test]
    fn a_capability_pushos_does_not_have_is_reported_by_name() {
        let mut manifest = manifest();
        manifest.requires.permissions = vec!["everything".to_owned()];

        let error = manifest
            .into_pack(Contents::default())
            .expect_err("there is no such capability");
        assert!(error.to_string().contains("everything"), "{error}");
    }

    #[test]
    fn required_and_optional_capabilities_stay_apart() {
        // An operator agreeing to a pack should be able to tell what it cannot
        // work without from what it would merely like.
        let mut manifest = manifest();
        manifest.requires.permissions = vec!["shell.execute".to_owned()];
        manifest.requires.optional_permissions = vec!["network".to_owned()];

        let pack = manifest.into_pack(Contents::default()).expect("valid");
        assert_eq!(pack.requires.permissions, [Permission::ShellExecute]);
        assert_eq!(pack.requires.optional, [Permission::Network]);
    }

    #[test]
    fn a_pack_with_no_name_or_version_is_refused() {
        for spoil in [
            |m: &mut Manifest| m.name = "  ".to_owned(),
            |m: &mut Manifest| m.version = String::new(),
        ] {
            let mut manifest = manifest();
            spoil(&mut manifest);
            assert!(manifest.into_pack(Contents::default()).is_err());
        }
    }
}
