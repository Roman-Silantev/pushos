//! Writing a starting configuration.

use std::path::Path;

use super::paths;

/// The preset a new installation starts from.
const STARTER: &str = include_str!("../../../../presets/starter.toml");

/// Writes the starter configuration, refusing to overwrite by default.
pub(crate) fn execute(requested: Option<&Path>, force: bool) -> Result<(), String> {
    let root = paths::config_root(requested)?;
    let target = if root.extension().is_some() {
        root.clone()
    } else {
        root.join(pushos_config::paths::MAIN_FILE)
    };

    if target.exists() && !force {
        return Err(format!(
            "`{}` already exists; pass --force to replace it",
            target.display()
        ));
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create `{}`: {error}", parent.display()))?;
    }

    std::fs::write(&target, STARTER)
        .map_err(|error| format!("could not write `{}`: {error}", target.display()))?;

    println!("wrote {}", target.display());
    println!("run `pushos check` to validate it, then `pushos run`");
    Ok(())
}

#[cfg(test)]
mod tests {
    use pushos_config::{ConfigFile, RuntimeConfig};

    use super::*;

    #[test]
    fn the_starter_configuration_that_ships_is_valid() {
        let parsed: ConfigFile =
            toml::from_str(STARTER).expect("the starter preset should be well-formed TOML");
        RuntimeConfig::build(&parsed).expect("the starter preset should be valid");
    }

    /// Every commented-out setting and binding in the preset, uncommented.
    ///
    /// A line counts as an example when what it says, with the comment marker
    /// removed, is a table header or an assignment. Prose never is, and a rule
    /// rather than a list of key names means a new setting cannot be
    /// documented without also being checked.
    fn with_every_example_enabled() -> String {
        STARTER
            .lines()
            .map(|line| {
                let Some(uncommented) = line.trim_start().strip_prefix("# ") else {
                    return line;
                };
                if looks_like_toml(uncommented) {
                    uncommented
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether a line is a table header or an assignment.
    fn looks_like_toml(line: &str) -> bool {
        if line.starts_with('[') {
            return true;
        }

        let Some((key, _)) = line.split_once(" = ") else {
            return false;
        };
        !key.is_empty()
            && key
                .chars()
                .all(|character| character.is_alphanumeric() || "_-.".contains(character))
    }

    #[test]
    fn every_example_the_starter_documents_actually_works() {
        // A setting shown under the wrong section, or a binding whose action no
        // longer exists, is a trap: the operator uncomments it and PushOS
        // refuses to start. This is what caught `shell` being documented under
        // gestures rather than runtime.
        let enabled = with_every_example_enabled();
        let parsed: ConfigFile = toml::from_str(&enabled)
            .unwrap_or_else(|error| panic!("a documented example is not valid TOML: {error}"));

        let config = RuntimeConfig::build(&parsed).unwrap_or_else(|error| {
            let problems: Vec<String> = error.problems().iter().map(ToString::to_string).collect();
            panic!("a documented example is not a valid configuration: {problems:?}")
        });

        assert!(
            config.shell.is_some(),
            "the shell example should land in the runtime section"
        );
        assert!(
            config.workspace_root.is_some(),
            "the workspace example should land in the runtime section"
        );
        assert!(
            !config.agents.is_empty(),
            "the agent examples should produce roles"
        );
        assert!(
            config.bindings.len() > parsed.pages.len(),
            "the binding examples should produce bindings"
        );
    }

    #[test]
    fn every_action_the_starter_documents_exists_and_declares_that_verb() {
        // Checked against the providers PushOS actually ships rather than a
        // list written here, so documentation cannot name something that was
        // renamed or removed. A verb that does not exist would fail only when
        // the operator finally pressed the control.
        let enabled = with_every_example_enabled();
        let parsed: ConfigFile = toml::from_str(&enabled).expect("valid TOML");
        let config = RuntimeConfig::build(&parsed).expect("valid");

        let shipped = crate::host::shipped_namespaces(&config);

        for binding in &parsed.bindings {
            let (provider, verb) = binding
                .action
                .split_once('.')
                .unwrap_or_else(|| panic!("`{}` is not written as provider.verb", binding.action));

            let verbs = shipped.get(provider).unwrap_or_else(|| {
                panic!("`{}` names a provider PushOS does not ship", binding.action)
            });
            assert!(
                verbs.iter().any(|shipped| shipped == verb),
                "`{}` names a verb `{provider}` does not have",
                binding.action
            );
        }
    }

    #[test]
    fn an_existing_configuration_is_not_replaced_without_being_asked() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-init-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");

        execute(Some(&directory), false).expect("the first write succeeds");
        let error = execute(Some(&directory), false).expect_err("the second must not overwrite");
        assert!(error.contains("--force"));

        execute(Some(&directory), true).expect("forcing replaces it");
        std::fs::remove_dir_all(&directory).ok();
    }
}
