//! Where the control socket lives, and who may reach it.

use std::path::{Path, PathBuf};

/// The socket's name inside the state directory.
pub const SOCKET_FILE: &str = "control.sock";

/// The permissions the socket is created with: readable and writable by its
/// owner and nobody else.
///
/// The socket can start actions and rewrite configuration, so it must not be
/// reachable by other accounts on a shared machine.
pub const SOCKET_MODE: u32 = 0o600;

/// The default socket path.
pub fn default_socket() -> Option<PathBuf> {
    pushos_config::paths::default_state_directory().map(|state| state.join(SOCKET_FILE))
}

/// The socket path inside a given state directory.
pub fn socket_in(state_directory: &Path) -> PathBuf {
    state_directory.join(SOCKET_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_owner_only() {
        assert_eq!(SOCKET_MODE & 0o077, 0, "no group or other access");
        assert_eq!(SOCKET_MODE, 0o600);
    }

    #[test]
    fn the_socket_sits_in_the_state_directory_not_the_configuration_one() {
        let (Some(socket), Some(config)) =
            (default_socket(), pushos_config::paths::default_directory())
        else {
            return;
        };
        assert!(
            !socket.starts_with(&config),
            "creating the socket must not look like a configuration change"
        );
    }

    #[test]
    fn a_state_directory_yields_a_socket_beside_the_database() {
        assert_eq!(
            socket_in(Path::new("/tmp/state")),
            PathBuf::from("/tmp/state/control.sock")
        );
    }
}
