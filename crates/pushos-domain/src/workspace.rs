//! What a project means to the rest of PushOS.
//!
//! A workspace is the answer to "which project am I in". It says where work
//! happens, which application opens it, who fills each role, and what the
//! surface should look like on arrival. One pad selects it, and everything
//! downstream follows: bindings scoped to it come into force, agents start in
//! its repository, terminals open there.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use crate::ids::{AgentId, PageId, ProviderName, WorkspaceId};

/// One project, and everything PushOS needs to know about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// Stable identity, referenced by bindings and targets.
    pub id: WorkspaceId,
    /// What the operator calls it, shown on the display.
    pub name: String,
    /// Where work happens: the repository or project directory.
    pub root: PathBuf,
    /// What it is, when a name is not enough.
    pub description: Option<String>,
    /// The page to show on arrival, when nothing was remembered.
    pub home_page: Option<PageId>,
    /// Applications that can open it, by the name a binding uses.
    pub apps: BTreeMap<String, String>,
    /// Which provider fills each role here.
    ///
    /// The same role can be one agent in one project and another elsewhere,
    /// which is the point of naming a role rather than a provider.
    pub roles: BTreeMap<AgentId, ProviderName>,
    /// Environment given to everything started here.
    pub env: BTreeMap<String, String>,
    /// Whether each agent gets its own working tree.
    ///
    /// Two coding agents editing one tree at the same time produce a mess
    /// nobody can untangle, so a project that runs several says so here.
    pub isolate_agents: bool,
}

impl Workspace {
    /// Builds a workspace rooted at a directory.
    pub fn new(
        id: impl Into<WorkspaceId>,
        name: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            root: root.into(),
            description: None,
            home_page: None,
            apps: BTreeMap::new(),
            roles: BTreeMap::new(),
            env: BTreeMap::new(),
            isolate_agents: false,
        }
    }

    /// The provider this workspace wants for a role, when it names one.
    pub fn provider_for(&self, agent: &AgentId) -> Option<&ProviderName> {
        self.roles.get(agent)
    }

    /// Where an application should be opened, by the name a binding uses.
    pub fn app(&self, name: &str) -> Option<&str> {
        self.apps.get(name).map(String::as_str)
    }
}

/// Which workspace an action acts on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceTarget {
    /// One specific workspace.
    Named(WorkspaceId),
    /// The next configured workspace, wrapping at the end.
    Next,
    /// The previous configured workspace, wrapping at the start.
    Previous,
    /// No workspace at all, leaving only the bindings that apply everywhere.
    None,
}

impl fmt::Display for WorkspaceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(workspace) => write!(f, "{workspace}"),
            Self::Next => f.write_str("next"),
            Self::Previous => f.write_str("previous"),
            Self::None => f.write_str("none"),
        }
    }
}

impl std::str::FromStr for WorkspaceTarget {
    type Err = MalformedWorkspaceTarget;

    /// Reads the form a binding writes:
    ///
    /// ```text
    /// sydclaw
    /// next
    /// previous
    /// none
    /// ```
    ///
    /// A bare name is a workspace, because that is what a binding almost always
    /// says and a prefix would only be noise.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim() {
            "" => Err(MalformedWorkspaceTarget(text.to_owned())),
            "next" => Ok(Self::Next),
            "previous" | "prev" => Ok(Self::Previous),
            "none" => Ok(Self::None),
            name => Ok(Self::Named(WorkspaceId::new(name))),
        }
    }
}

/// A workspace target that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` does not name a workspace; expected an id, `next`, `previous` or `none`")]
pub struct MalformedWorkspaceTarget(pub String);

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    fn parse(text: &str) -> Result<WorkspaceTarget, MalformedWorkspaceTarget> {
        WorkspaceTarget::from_str(text)
    }

    #[test]
    fn a_bare_name_is_a_workspace() {
        // The common case in a binding, so it needs no ceremony.
        assert_eq!(
            parse("sydclaw"),
            Ok(WorkspaceTarget::Named(WorkspaceId::new("sydclaw")))
        );
    }

    #[test]
    fn every_target_survives_being_written_out_and_read_back() {
        for text in ["sydclaw", "next", "previous", "none"] {
            let target = parse(text).expect("the form is valid");
            assert_eq!(target.to_string(), text);
        }
    }

    #[test]
    fn naming_nothing_is_refused_rather_than_treated_as_leaving() {
        // Leaving a workspace is `none`, said deliberately. An empty target is
        // a mistake, and guessing at it would silently drop the operator out of
        // the project they were in.
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn prev_is_accepted_as_a_shorthand() {
        assert_eq!(parse("prev"), Ok(WorkspaceTarget::Previous));
    }

    #[test]
    fn a_workspace_names_the_provider_it_wants_for_a_role() {
        // The same role is one agent in one project and another elsewhere,
        // which is the point of a binding naming a role.
        let mut workspace = Workspace::new("sydclaw", "Sydclaw", "/tmp/sydclaw");
        workspace
            .roles
            .insert(AgentId::new("builder"), ProviderName::new("claude"));

        assert_eq!(
            workspace.provider_for(&AgentId::new("builder")),
            Some(&ProviderName::new("claude"))
        );
        assert_eq!(workspace.provider_for(&AgentId::new("reviewer")), None);
    }

    #[test]
    fn a_new_workspace_isolates_nothing_until_it_says_so() {
        let workspace = Workspace::new("sydclaw", "Sydclaw", "/tmp/sydclaw");
        assert!(!workspace.isolate_agents);
        assert!(workspace.apps.is_empty());
    }
}
