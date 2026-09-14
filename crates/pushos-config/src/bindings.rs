//! Bindings: what each control does, where, and what it is called.

use std::collections::{HashMap, HashSet};

use pushos_domain::action::{ActionDefinition, ActionSelector, Params};
use pushos_domain::binding::{Binding, BindingScope};

use crate::error::Problem;
use crate::model::{BindingEntry, ConfigFile};

pub(crate) fn build(
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
