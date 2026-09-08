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
