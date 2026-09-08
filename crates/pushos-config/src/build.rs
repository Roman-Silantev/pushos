//! Turning a parsed file into something the runtime can use.
//!
//! Validation and construction happen together and produce either a complete
//! configuration or a complete list of problems. There is no partial result, so
//! a bad edit can never be half-applied to a running system.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use pushos_bindings::{BindingTable, GestureTiming, find_conflicts};
use pushos_domain::action::{ActionDefinition, ActionSelector, Params};
use pushos_domain::agent::AgentDefinition;
use pushos_domain::binding::{Binding, BindingScope};
use pushos_domain::ids::PageId;
use pushos_domain::page::Page;
use pushos_domain::permissions::PermissionSet;

use crate::error::{ConfigError, Problem};
use crate::model::{BindingEntry, ConfigFile};

/// A validated configuration, ready to be swapped in.
#[derive(Debug)]
pub struct RuntimeConfig {
    /// The pages that exist, in declaration order.
    pub pages: Vec<Page>,
    /// The page PushOS starts on and returns to.
    pub home_page: Option<PageId>,
    /// The bindings in force, indexed for resolution.
    pub bindings: BindingTable,
    /// Gesture timing.
    pub timing: GestureTiming,
    /// The capabilities the operator has granted.
    pub permissions: PermissionSet,
    /// The media player to control.
    pub media_player: Option<String>,
    /// The agent roles that exist.
    pub agents: Vec<AgentDefinition>,
    /// The agent providers PushOS may start.
    pub providers: Vec<crate::model::ProviderEntry>,
    /// The projects that exist, in declaration order.
    pub workspaces: Vec<pushos_domain::workspace::Workspace>,
    /// Where agents work when a role does not name a workspace.
    pub workspace_root: Option<std::path::PathBuf>,
    /// The program a terminal runs when a binding does not name one.
    pub shell: Option<String>,
}

impl RuntimeConfig {
    /// Validates a parsed file and builds a runtime configuration from it.
    ///
    /// Collects every problem rather than stopping at the first, so one editing
    /// pass can fix a whole file.
    pub fn build(file: &ConfigFile) -> Result<Self, ConfigError> {
        let mut problems = Vec::new();

        let timing = build_timing(&file.gestures);
        if let Err(invalid) = timing.validate() {
            problems.push(Problem::Timing(invalid));
        }

        let (pages, page_ids) = build_pages(file, &mut problems);
        if let Some(home) = &file.runtime.home_page
            && !page_ids.contains(home.as_str())
        {
            problems.push(Problem::UnknownHomePage { page: home.clone() });
        }

        let providers = build_providers(file, &mut problems);
        let agents = build_agents(file, &providers, &mut problems);
        let workspaces = build_workspaces(file, &providers, &page_ids, &mut problems);
        let workspace_ids: HashSet<String> = workspaces
            .iter()
            .map(|workspace| workspace.id.to_string())
            .collect();

        // Built last, because a binding may be scoped to a page or a project
        // and both have to exist before it can be checked against them.
        let bindings = build_bindings(file, &page_ids, &workspace_ids, &mut problems);
        problems.extend(find_conflicts(&bindings).into_iter().map(Problem::Conflict));

        if !problems.is_empty() {
            return Err(ConfigError::Invalid { problems });
        }

        Ok(Self {
            pages,
            home_page: file.runtime.home_page.as_deref().map(PageId::new),
            bindings: BindingTable::new(bindings),
            timing,
            permissions: file.permissions.granted.iter().copied().collect(),
            media_player: file.runtime.media_player.clone(),
            agents,
            providers,
            workspaces,
            workspace_root: file
                .runtime
                .workspace_root
                .as_deref()
                .map(crate::paths::expand_home),
            shell: file.runtime.shell.clone(),
        })
    }

    /// An empty configuration, used before any file has been read.
    pub fn empty() -> Self {
        Self {
            pages: Vec::new(),
            home_page: None,
            bindings: BindingTable::new([]),
            timing: GestureTiming::DEFAULT,
            permissions: PermissionSet::empty(),
            media_player: None,
            agents: Vec::new(),
            providers: Vec::new(),
            workspaces: Vec::new(),
            workspace_root: None,
            shell: None,
        }
    }

    /// Where agents work, falling back to wherever PushOS was started.
    pub fn workspace_root(&self) -> std::path::PathBuf {
        self.workspace_root.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
    }

    /// The role with this identity.
    pub fn agent(&self, id: &pushos_domain::ids::AgentId) -> Option<&AgentDefinition> {
        self.agents.iter().find(|agent| agent.id == *id)
    }

    /// Where a page sits in the configured order, one-based.
    pub fn page_position(&self, page: &PageId) -> Option<(usize, usize)> {
        let index = self
            .pages
            .iter()
            .position(|candidate| candidate.id == *page)?;
        Some((index + 1, self.pages.len()))
    }

    /// The page after `page` in configured order, wrapping at the end.
    pub fn page_after(&self, page: Option<&PageId>) -> Option<&Page> {
        self.step_page(page, 1)
    }

    /// The page before `page` in configured order, wrapping at the start.
    pub fn page_before(&self, page: Option<&PageId>) -> Option<&Page> {
        self.step_page(page, -1)
    }

    fn step_page(&self, page: Option<&PageId>, step: isize) -> Option<&Page> {
        if self.pages.is_empty() {
            return None;
        }
        let total = isize::try_from(self.pages.len()).ok()?;
        let current = page
            .and_then(|id| self.pages.iter().position(|candidate| candidate.id == *id))
            .and_then(|index| isize::try_from(index).ok())
            .unwrap_or(0);

        let next = (current + step).rem_euclid(total);
        self.pages.get(usize::try_from(next).ok()?)
    }
}

/// Builds the provider list, refusing two providers with one name.
fn build_providers(
    file: &ConfigFile,
    problems: &mut Vec<Problem>,
) -> Vec<crate::model::ProviderEntry> {
    let mut seen = HashSet::with_capacity(file.providers.len());
    let mut providers = Vec::with_capacity(file.providers.len());

    for entry in &file.providers {
        if !seen.insert(entry.id.clone()) {
            problems.push(Problem::DuplicateProvider {
                id: entry.id.clone(),
            });
            continue;
        }
        // An explicitly empty program is a mistake; an absent one means "use
        // the command PushOS already knows for this name", which the host
        // resolves and reports on when it cannot.
        if entry
            .program
            .as_ref()
            .is_some_and(|program| program.trim().is_empty())
        {
            problems.push(Problem::ProviderWithoutProgram {
                id: entry.id.clone(),
            });
            continue;
        }
        providers.push(entry.clone());
    }

    providers
}

/// Builds the agent roles, checking that any provider they name exists.
fn build_agents(
    file: &ConfigFile,
    providers: &[crate::model::ProviderEntry],
    problems: &mut Vec<Problem>,
) -> Vec<AgentDefinition> {
    let known: HashSet<_> = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect();
    let mut seen = HashSet::with_capacity(file.agents.len());
    let mut agents = Vec::with_capacity(file.agents.len());

    for entry in &file.agents {
        if !seen.insert(entry.id.clone()) {
            problems.push(Problem::DuplicateAgent {
                id: entry.id.clone(),
            });
            continue;
        }

        // Naming a provider that is not configured is a typo, not a preference:
        // the role would silently fall back to something else.
        let unknown: Vec<_> = entry
            .preferred
            .iter()
            .filter(|provider| !known.contains(provider.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            problems.push(Problem::UnknownAgentProvider {
                agent: entry.id.clone(),
                providers: unknown,
            });
            continue;
        }

        let mut definition = AgentDefinition::new(entry.id.as_str(), entry.name.as_str());
        definition.objective.clone_from(&entry.objective);
        definition.preferred = entry
            .preferred
            .iter()
            .map(pushos_domain::ids::ProviderName::new)
            .collect();
        definition.permissions = entry.permissions.iter().copied().collect();
        agents.push(definition);
    }

    agents
}

fn build_timing(section: &crate::model::GestureSection) -> GestureTiming {
    let defaults = GestureTiming::DEFAULT;
    GestureTiming {
        hold_threshold: section
            .hold_threshold_ms
            .map_or(defaults.hold_threshold, Duration::from_millis),
        double_tap_window: section
            .double_tap_window_ms
            .map_or(defaults.double_tap_window, Duration::from_millis),
    }
}

fn build_pages(file: &ConfigFile, problems: &mut Vec<Problem>) -> (Vec<Page>, HashSet<String>) {
    let mut pages = Vec::with_capacity(file.pages.len());
    let mut ids = HashSet::with_capacity(file.pages.len());

    for entry in &file.pages {
        if !ids.insert(entry.id.clone()) {
            problems.push(Problem::DuplicatePage {
                id: entry.id.clone(),
            });
            continue;
        }
        let mut page = Page::new(entry.id.as_str(), entry.name.as_str());
        page.description.clone_from(&entry.description);
        pages.push(page);
    }

    (pages, ids)
}

/// Builds the projects, refusing two with one identity and anything that
/// references something undeclared.
fn build_workspaces(
    file: &ConfigFile,
    providers: &[crate::model::ProviderEntry],
    pages: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Vec<pushos_domain::workspace::Workspace> {
    use pushos_domain::ids::{AgentId, PageId, ProviderName, WorkspaceId};
    use pushos_domain::workspace::Workspace;

    let known: HashSet<_> = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect();
    let mut seen = HashSet::with_capacity(file.workspaces.len());
    let mut workspaces = Vec::with_capacity(file.workspaces.len());

    for entry in &file.workspaces {
        if !seen.insert(entry.id.clone()) {
            problems.push(Problem::DuplicateWorkspace {
                id: entry.id.clone(),
            });
            continue;
        }

        if let Some(page) = &entry.home_page
            && !pages.contains(page.as_str())
        {
            problems.push(Problem::UnknownWorkspacePage {
                workspace: entry.id.clone(),
                page: page.clone(),
            });
        }

        let mut roles = std::collections::BTreeMap::new();
        for (agent, provider) in &entry.roles {
            if known.contains(provider.as_str()) {
                roles.insert(AgentId::new(agent), ProviderName::new(provider));
            } else {
                // Left out rather than kept: a role pointing at a provider that
                // does not exist would silently fall back to the global
                // preference, which is not what the file says.
                problems.push(Problem::UnknownWorkspaceProvider {
                    workspace: entry.id.clone(),
                    agent: agent.clone(),
                    provider: provider.clone(),
                });
            }
        }

        workspaces.push(Workspace {
            id: WorkspaceId::new(&entry.id),
            name: entry.name.clone(),
            root: crate::paths::expand_home(&entry.root),
            description: entry.description.clone(),
            home_page: entry.home_page.as_deref().map(PageId::new),
            apps: entry.apps.clone(),
            roles,
            env: entry.env.clone(),
            isolate_agents: entry.isolate_agents,
        });
    }

    workspaces
}

fn build_bindings(
    file: &ConfigFile,
    pages: &HashSet<String>,
    workspaces: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Vec<Binding> {
    let mut bindings = Vec::with_capacity(file.bindings.len());
    let mut seen_ids: HashMap<String, usize> = HashMap::new();

    for (position, entry) in file.bindings.iter().enumerate() {
        let index = position + 1;
        let before = problems.len();

        let control = entry.control.parse().map_err(|source| {
            problems.push(Problem::Control { index, source });
        });
        let gesture = entry.gesture.parse().map_err(|source| {
            problems.push(Problem::Gesture { index, source });
        });
        let selector = entry.action.parse::<ActionSelector>().map_err(|source| {
            problems.push(Problem::Action { index, source });
        });
        let scope = resolve_scope(entry, index, pages, workspaces, problems);

        let identity = entry.id.clone().unwrap_or_else(|| derived_id(entry, index));
        if let Some(first) = seen_ids.insert(identity.clone(), index) {
            problems.push(Problem::DuplicateBindingId {
                id: format!("{identity} (bindings {first} and {index})"),
            });
        }

        // Anything that failed above has already been reported; skip the entry
        // rather than substituting a default that would hide the error.
        let (Ok(control), Ok(gesture), Ok(selector), Some(scope)) =
            (control, gesture, selector, scope)
        else {
            debug_assert!(problems.len() > before, "a skipped binding must report why");
            continue;
        };

        bindings.push(Binding {
            id: identity.into(),
            control,
            gesture,
            scope,
            action: ActionDefinition::new(selector, build_params(entry)),
            priority: entry.priority,
            label: entry.label.clone(),
        });
    }

    bindings
}

/// Merges the `target` shorthand into the parameter map.
fn build_params(entry: &BindingEntry) -> Params {
    let mut params: Params = entry
        .params
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(target) = &entry.target {
        params.set(
            "target",
            pushos_domain::action::ParamValue::from(target.as_str()),
        );
    }
    params
}

/// Works out where a binding applies, checking any declared scope against the
/// fields that actually determine it.
fn resolve_scope(
    entry: &BindingEntry,
    index: usize,
    pages: &HashSet<String>,
    workspaces: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Option<BindingScope> {
    if let Some(page) = &entry.page
        && !pages.contains(page.as_str())
    {
        problems.push(Problem::UnknownPage {
            index,
            page: page.clone(),
        });
        return None;
    }

    // A binding scoped to a project nothing declares can never fire, and
    // finding that out by pressing the pad is the expensive way.
    if let Some(workspace) = &entry.workspace
        && !workspaces.contains(workspace.as_str())
    {
        problems.push(Problem::UnknownBindingWorkspace {
            index,
            workspace: workspace.clone(),
        });
        return None;
    }

    let scope = match (&entry.workspace, &entry.page) {
        (Some(workspace), Some(page)) => BindingScope::WorkspacePage {
            workspace: workspace.as_str().into(),
            page: page.as_str().into(),
        },
        (Some(workspace), None) => BindingScope::Workspace(workspace.as_str().into()),
        (None, Some(page)) => BindingScope::Page(page.as_str().into()),
        (None, None) => BindingScope::Global,
    };

    let expected = match &scope {
        BindingScope::Global => "global",
        BindingScope::Page(_) => "page",
        BindingScope::Workspace(_) => "workspace",
        BindingScope::WorkspacePage { .. } => "workspace+page",
    };

    if let Some(declared) = &entry.scope
        && declared != expected
    {
        problems.push(Problem::ScopeMismatch {
            index,
            declared: declared.clone(),
        });
        return None;
    }

    Some(scope)
}

/// Builds a stable identity for a binding that did not declare one.
fn derived_id(entry: &BindingEntry, index: usize) -> String {
    let scope = match (&entry.workspace, &entry.page) {
        (Some(workspace), Some(page)) => format!("{workspace}/{page}"),
        (Some(workspace), None) => workspace.clone(),
        (None, Some(page)) => page.clone(),
        (None, None) => "global".to_owned(),
    };
    format!("{scope}:{}:{}#{index}", entry.control, entry.gesture)
}
