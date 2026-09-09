//! Where configuration lives.

use std::path::{Path, PathBuf};

/// The directory name PushOS uses under the operator's configuration home.
const APP_DIRECTORY: &str = "pushos";
/// The file loaded when a directory is given.
pub const MAIN_FILE: &str = "pushos.toml";
/// The subdirectory whose files are merged after the main file.
pub const INCLUDE_DIRECTORY: &str = "conf.d";

/// The directory installed packs live in, one directory each.
pub const PACK_DIRECTORY: &str = "packs";

/// The manifest at the top of a pack.
pub const PACK_MANIFEST: &str = "pack.toml";

/// The file that marks an installed pack as switched off.
///
/// A file rather than a database row, so an operator can disable a pack with
/// `touch` and see why it is off by looking.
pub const PACK_DISABLED: &str = "disabled";

/// The default configuration directory.
///
/// Follows the operator's `XDG_CONFIG_HOME` when set, so a machine that already
/// organises configuration that way keeps doing so; otherwise `~/.config`.
pub fn default_directory() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Some(configured.join(APP_DIRECTORY));
    }
    home_directory().map(|home| home.join(".config").join(APP_DIRECTORY))
}

/// The default directory for runtime state.
///
/// Deliberately not the configuration directory: the database is written
/// constantly, and a configuration watcher pointed at it would reload on every
/// write.
pub fn default_state_directory() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Some(configured.join(APP_DIRECTORY));
    }
    home_directory().map(|home| home.join(".local").join("share").join(APP_DIRECTORY))
}

/// The operator's home directory.
pub fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Expands a leading `~` in a configured path.
///
/// Configuration is written by people, and a path in a file is far more likely
/// to be written with a tilde than with an absolute home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix('~') else {
        return PathBuf::from(path);
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    match home_directory() {
        Some(home) if rest.is_empty() => home,
        Some(home) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// The files to load for a given root, in the order they are merged.
///
/// Included files are sorted by name so the result never depends on the order
/// the file system happens to list them in.
pub fn files_for(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }

    // Packs first, so that what the operator wrote themselves is read after and
    // wins where a setting can only have one value. A pack is a starting point
    // they were offered, not a thing that overrules them.
    let mut files = pack_files(root);

    let main = root.join(MAIN_FILE);
    if main.is_file() {
        files.push(main);
    }

    let mut included: Vec<_> = std::fs::read_dir(root.join(INCLUDE_DIRECTORY))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect();
    included.sort();
    files.extend(included);

    files
}

/// Every configuration file belonging to an enabled pack.
///
/// Packs are read in name order, and each pack's own files in name order
/// within it, so what a surface ends up being never depends on how the file
/// system happened to list a directory.
pub fn pack_files(root: &Path) -> Vec<PathBuf> {
    let mut packs: Vec<PathBuf> = std::fs::read_dir(root.join(PACK_DIRECTORY))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    packs.sort();

    let mut files = Vec::new();
    for pack in packs {
        if pack.join(PACK_DISABLED).exists() {
            continue;
        }
        files.extend(files_in_pack(&pack));
    }
    files
}

/// The configuration files inside one pack, in the order they are merged.
///
/// Everything ending in `.toml` at any depth, except the manifest, which
/// describes the pack rather than contributing to the surface.
pub fn files_in_pack(pack: &Path) -> Vec<PathBuf> {
    let manifest = pack.join(PACK_MANIFEST);
    let mut found = Vec::new();
    let mut pending = vec![pack.to_path_buf()];

    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|it| it == "toml") && path != manifest {
                found.push(path);
            }
        }
    }

    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_does_not_live_inside_the_configuration_directory() {
        let (Some(config), Some(state)) = (default_directory(), default_state_directory()) else {
            return;
        };
        assert_ne!(config, state);
        assert!(
            !state.starts_with(&config),
            "writing state must not look like a configuration change"
        );
    }

    #[test]
    fn a_path_without_a_tilde_is_left_alone() {
        assert_eq!(expand_home("/etc/pushos"), PathBuf::from("/etc/pushos"));
        assert_eq!(expand_home("relative/path"), PathBuf::from("relative/path"));
    }

    #[test]
    fn a_leading_tilde_becomes_the_home_directory() {
        let Some(home) = home_directory() else {
            return;
        };
        assert_eq!(expand_home("~/Projects"), home.join("Projects"));
        assert_eq!(expand_home("~"), home);
    }

    #[test]
    fn a_tilde_inside_a_path_is_not_expanded() {
        assert_eq!(expand_home("/tmp/~backup"), PathBuf::from("/tmp/~backup"));
    }

    #[test]
    fn a_file_root_loads_only_that_file() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-paths-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
        let file = directory.join("custom.toml");
        std::fs::write(&file, "").expect("the file is writable");

        assert_eq!(files_for(&file), [file.clone()][..]);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn included_files_are_merged_after_the_main_file_in_name_order() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-paths-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(directory.join(INCLUDE_DIRECTORY))
            .expect("the temporary directory is writable");
        std::fs::write(directory.join(MAIN_FILE), "").expect("writable");
        for name in ["30-late.toml", "10-early.toml", "notes.txt"] {
            std::fs::write(directory.join(INCLUDE_DIRECTORY).join(name), "").expect("writable");
        }

        let files = files_for(&directory);
        let names: Vec<_> = files
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .collect();

        assert_eq!(names, [MAIN_FILE, "10-early.toml", "30-late.toml"]);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_directory_with_nothing_in_it_yields_no_files() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-paths-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        assert!(files_for(&directory).is_empty());
        std::fs::remove_dir_all(&directory).ok();
    }
}
