//! Working out where things live.

use std::path::PathBuf;

/// The database's name inside the state directory.
const STATE_FILE: &str = "state.sqlite";

/// The configuration root to use, given what was asked for on the command line.
pub(crate) fn config_root(requested: Option<&std::path::Path>) -> Result<PathBuf, String> {
    if let Some(path) = requested {
        return Ok(path.to_path_buf());
    }
    pushos_config::paths::default_directory().ok_or_else(|| {
        "could not work out where configuration lives; pass --config with a path".to_owned()
    })
}

/// Where runtime state is kept.
///
/// Deliberately outside the configuration directory. The database is written
/// constantly, and the configuration watcher is pointed at that directory, so
/// keeping them together would mean reloading on every write.
pub(crate) fn state_file() -> Result<PathBuf, String> {
    let directory = pushos_config::paths::default_state_directory().ok_or_else(|| {
        "could not work out where to keep runtime state; set XDG_DATA_HOME or HOME".to_owned()
    })?;
    Ok(directory.join(STATE_FILE))
}

/// Where isolated working trees are kept.
///
/// Outside the operator's repository on purpose: PushOS does not leave
/// directories inside a project that the operator did not put there.
pub(crate) fn worktree_root() -> Result<PathBuf, String> {
    let directory = pushos_config::paths::default_state_directory().ok_or_else(|| {
        "could not work out where to keep runtime state; set XDG_DATA_HOME or HOME".to_owned()
    })?;
    Ok(directory.join("worktrees"))
}

/// Where the control socket lives.
pub(crate) fn control_socket() -> Result<PathBuf, String> {
    let directory = pushos_config::paths::default_state_directory().ok_or_else(|| {
        "could not work out where to keep runtime state; set XDG_DATA_HOME or HOME".to_owned()
    })?;
    Ok(pushos_api::socket_in(&directory))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_path_is_used_as_given() {
        let requested = std::path::Path::new("/tmp/somewhere");
        assert_eq!(config_root(Some(requested)).expect("explicit"), requested);
    }

    #[test]
    fn the_socket_sits_beside_the_database() {
        let (Ok(socket), Ok(state)) = (control_socket(), state_file()) else {
            return;
        };
        assert_eq!(socket.parent(), state.parent());
    }

    #[test]
    fn state_never_lands_inside_the_watched_configuration_directory() {
        let (Ok(state), Ok(config)) = (state_file(), config_root(None)) else {
            return;
        };
        assert!(
            !state.starts_with(&config),
            "the database must not sit where the configuration watcher is looking"
        );
    }
}
