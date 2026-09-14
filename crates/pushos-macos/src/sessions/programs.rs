//! Finding the programs that know about sessions.
//!
//! PushOS usually runs as a login agent, whose `PATH` is whatever it was
//! installed with, and a program installed since then is not on it. So each is
//! looked for on `PATH` and then where its installer puts it.

use std::path::{Path, PathBuf};

/// Where installers put programs that a login agent's `PATH` may not include:
/// Homebrew on Apple silicon, Homebrew on Intel, and a per-user install such as
/// Claude Code's.
const FALLBACKS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "~/.local/bin"];

/// Finds an executable by name, the way a shell would.
pub(super) fn find(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    find_in(name, &path, home.as_deref())
}

/// Finds an executable by name in a search path and the fallbacks.
fn find_in(name: &str, path: &std::ffi::OsStr, home: Option<&Path>) -> Option<PathBuf> {
    let fallbacks = FALLBACKS
        .iter()
        .filter_map(|directory| match directory.strip_prefix("~/") {
            Some(rest) => home.map(|home| home.join(rest)),
            None => Some(PathBuf::from(directory)),
        });

    std::env::split_paths(path)
        .chain(fallbacks)
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(candidate: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(candidate)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn directory(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pushos-programs-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("writable");
        root
    }

    fn program(directory: &Path, name: &str, mode: u32) {
        let file = directory.join(name);
        std::fs::write(&file, "#!/bin/sh\n").expect("writable");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).expect("settable");
    }

    #[test]
    fn a_program_on_the_path_is_found_there() {
        let bin = directory("path");
        program(&bin, "tool", 0o755);
        let found = find_in("tool", bin.as_os_str(), None);
        assert_eq!(found, Some(bin.join("tool")));
        std::fs::remove_dir_all(bin).ok();
    }

    #[test]
    fn a_program_in_the_users_own_bin_is_found_when_the_path_forgot_it() {
        // A login agent installed before Claude Code was has a PATH without
        // ~/.local/bin in it.
        let home = directory("home");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).expect("writable");
        program(&bin, "pushos-test-tool", 0o755);

        let found = find_in(
            "pushos-test-tool",
            std::ffi::OsStr::new("/usr/bin"),
            Some(&home),
        );
        assert_eq!(found, Some(bin.join("pushos-test-tool")));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn a_file_that_cannot_be_run_is_not_a_program() {
        let bin = directory("plain");
        program(&bin, "notes", 0o644);
        assert_eq!(find_in("notes", bin.as_os_str(), None), None);
        std::fs::remove_dir_all(bin).ok();
    }
}
