//! Installable packs: a working set of agents, workflows, pages and bindings.
//!
//! A pack is a directory of ordinary PushOS configuration with a manifest on
//! the front. Installing one copies it in; removing one deletes what was
//! copied and nothing else. There is no package database, no lock file and
//! nothing PushOS does to a pack that an operator could not do with `cp`.
//!
//! What the manifest adds beyond the files is consent. A pack declares what it
//! needs before it can be installed, PushOS shows that, and the operator says
//! yes to a list they read. A pack cannot grant itself anything.

use std::fmt;

use crate::ids::PackId;
use crate::permissions::Permission;

/// A pack, as its manifest describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pack {
    /// What it is called on the command line, and the directory it installs to.
    pub id: PackId,
    /// What it is called in a listing.
    pub name: String,
    /// Which version of it this is.
    pub version: String,
    /// What it is for.
    pub description: Option<String>,
    /// Who made it.
    pub author: Option<String>,
    /// Under what terms.
    pub license: Option<String>,
    /// What it needs before it can be installed.
    pub requires: Requirements,
    /// What it contributes to the surface.
    pub adds: Contents,
}

impl Pack {
    /// One line for a listing.
    pub fn summary(&self) -> String {
        match &self.description {
            Some(description) => description
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or(&self.name)
                .to_owned(),
            None => self.name.clone(),
        }
    }
}

/// What a pack needs before it can be installed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requirements {
    /// Capabilities the pack cannot work without.
    ///
    /// Shown to the operator before anything is written. A pack cannot grant
    /// itself these; the installer writes them once the operator has agreed.
    pub permissions: Vec<Permission>,
    /// Capabilities it would use but works without.
    pub optional: Vec<Permission>,
    /// Agent providers it expects to be able to start.
    ///
    /// Not a hard requirement: a role names providers in preference order, and
    /// a missing one is reported rather than fatal.
    pub providers: Vec<String>,
    /// The oldest PushOS this pack was written for.
    pub minimum_version: Option<String>,
}

impl Requirements {
    /// Whether the pack asks for anything at all.
    pub fn is_empty(&self) -> bool {
        self.permissions.is_empty() && self.optional.is_empty() && self.providers.is_empty()
    }

    /// Whether anything here would let a pack act outside PushOS.
    ///
    /// What an operator should read twice before agreeing. A pack that can
    /// deploy, send or spend is a different proposition from one that draws
    /// pages.
    pub fn is_far_reaching(&self) -> bool {
        self.permissions
            .iter()
            .any(|permission| permission.is_irreversible())
    }
}

/// What a pack contributes, counted.
///
/// Shown before installation, so the operator knows the size of what they are
/// agreeing to rather than only its name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    /// Agent roles.
    pub agents: usize,
    /// Workflows.
    pub workflows: usize,
    /// Pages.
    pub pages: usize,
    /// Bindings.
    pub bindings: usize,
    /// Spoken phrases.
    pub phrases: usize,
    /// Note sources.
    pub note_sources: usize,
}

impl Contents {
    /// Whether the pack contributes anything at all.
    pub const fn is_empty(&self) -> bool {
        self.agents == 0
            && self.workflows == 0
            && self.pages == 0
            && self.bindings == 0
            && self.phrases == 0
            && self.note_sources == 0
    }

    /// Each part that is not zero, as a person would read it.
    pub fn described(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (count, one, many) in [
            (self.agents, "agent", "agents"),
            (self.workflows, "workflow", "workflows"),
            (self.pages, "page", "pages"),
            (self.bindings, "binding", "bindings"),
            (self.phrases, "spoken phrase", "spoken phrases"),
            (self.note_sources, "note source", "note sources"),
        ] {
            if count > 0 {
                lines.push(format!("{count} {}", if count == 1 { one } else { many }));
            }
        }
        lines
    }
}

impl fmt::Display for Contents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let described = self.described();
        if described.is_empty() {
            f.write_str("nothing")
        } else {
            f.write_str(&described.join(", "))
        }
    }
}

/// Whether an installed pack is in force.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PackState {
    /// Loaded with the rest of the configuration.
    #[default]
    Enabled,
    /// Installed and ignored.
    ///
    /// Kept rather than deleted, so turning a pack off does not cost the
    /// operator whatever they changed inside it.
    Disabled,
}

impl PackState {
    /// How it reads in a listing.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    /// Whether PushOS loads this pack's files.
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// A pack that is on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// What it is.
    pub pack: Pack,
    /// Whether it is in force.
    pub state: PackState,
    /// Where it lives.
    pub root: std::path::PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Pack {
        Pack {
            id: PackId::new("seo"),
            name: "SEO Growth Pack".to_owned(),
            version: "1.0.0".to_owned(),
            description: None,
            author: None,
            license: None,
            requires: Requirements::default(),
            adds: Contents::default(),
        }
    }

    #[test]
    fn a_pack_with_no_description_is_summarised_by_its_name() {
        assert_eq!(pack().summary(), "SEO Growth Pack");
    }

    #[test]
    fn a_multi_line_description_is_summarised_by_its_first_line() {
        // Manifests use triple-quoted strings, so the first line is usually
        // blank and the listing has one line to give it.
        let mut pack = pack();
        pack.description = Some("\n  Audits and content work.\n  More detail here.\n".to_owned());
        assert_eq!(pack.summary(), "Audits and content work.");
    }

    #[test]
    fn contents_read_as_a_person_would_say_them() {
        let contents = Contents {
            agents: 3,
            workflows: 1,
            pages: 1,
            bindings: 8,
            ..Contents::default()
        };
        assert_eq!(
            contents.to_string(),
            "3 agents, 1 workflow, 1 page, 8 bindings"
        );
    }

    #[test]
    fn a_pack_that_adds_nothing_says_so() {
        assert!(Contents::default().is_empty());
        assert_eq!(Contents::default().to_string(), "nothing");
    }

    #[test]
    fn a_pack_asking_for_something_irreversible_is_marked_as_such() {
        // What an operator should read twice: a pack that can deploy or send is
        // a different proposition from one that draws pages.
        let mut requires = Requirements {
            permissions: vec![Permission::FilesystemRead],
            ..Requirements::default()
        };
        assert!(!requires.is_far_reaching());

        requires.permissions.push(Permission::DeployProduction);
        assert!(requires.is_far_reaching());
    }

    #[test]
    fn a_pack_that_asks_for_nothing_says_so() {
        assert!(Requirements::default().is_empty());
    }

    #[test]
    fn a_disabled_pack_is_not_loaded_but_is_still_there() {
        assert!(PackState::Enabled.is_enabled());
        assert!(!PackState::Disabled.is_enabled());
        assert_eq!(PackState::Disabled.describe(), "disabled");
    }
}
