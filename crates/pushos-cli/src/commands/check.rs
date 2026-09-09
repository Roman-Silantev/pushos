//! Checking a configuration without running anything.

use std::path::Path;

use pushos_config::{ConfigError, RuntimeConfig};

use super::paths;

/// Validates the configuration and reports what it describes.
pub(crate) fn execute(requested: Option<&Path>) -> Result<(), String> {
    let root = paths::config_root(requested)?;
    let file = pushos_config::load(&root).map_err(|error| describe(&error))?;
    let config = RuntimeConfig::build(&file).map_err(|error| describe(&error))?;

    println!("{}", root.display());
    println!("  {} pages", config.pages.len());
    println!("  {} bindings", config.bindings.len());
    // Only when there are any. A line saying nothing is configured is noise on
    // every surface that never wanted one.
    match config.sequences.len() {
        0 => {}
        1 => println!("  1 sequence"),
        many => println!("  {many} sequences"),
    }

    let granted: Vec<_> = config
        .permissions
        .iter()
        .map(pushos_domain::permissions::Permission::slug)
        .collect();
    if granted.is_empty() {
        println!("  no permissions granted");
    } else {
        println!("  permissions: {}", granted.join(", "));
    }

    match &config.home_page {
        Some(home) => println!("  home page: {home}"),
        None => println!("  no home page set"),
    }

    Ok(())
}

/// Renders a configuration failure so every problem is visible at once.
pub(crate) fn describe(error: &ConfigError) -> String {
    use std::fmt::Write as _;

    let mut report = error.to_string();
    for problem in error.problems() {
        // Writing into a `String` cannot fail.
        let _ = write!(report, "\n  {problem}");
    }
    report
}
