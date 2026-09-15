//! Temporary directories and files that go when a test ends, however it ends.
//!
//! A test that fails does not reach the line that tidies up after it, and a
//! test run that fails a few times a day leaves a directory behind each time.
//! Dropping is the one thing that happens whether a test passed, failed or
//! panicked, so that is where removal belongs.

use std::path::{Path, PathBuf};

use pushos_domain::ids::ExecutionId;

/// A directory for one test, removed when the test ends.
#[derive(Debug)]
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    /// Makes an empty directory named after `label`.
    pub(crate) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pushos-{label}-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    /// Makes one holding a configuration file.
    pub(crate) fn holding_config(label: &str, text: &str) -> Self {
        let scratch = Self::new(label);
        scratch.write_config(text);
        scratch
    }

    /// Writes the configuration file, replacing what was there.
    pub(crate) fn write_config(&self, text: &str) {
        std::fs::write(self.0.join("pushos.toml"), text).expect("writable");
    }

    /// Where it is.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// A file that is not made here but goes the same way: a Unix socket.
///
/// Its path is short on purpose. A socket path may be at most 100 characters,
/// and the system temporary directory on macOS uses most of that.
#[derive(Debug)]
pub(crate) struct SocketPath(PathBuf);

impl SocketPath {
    /// Names a socket that does not exist yet.
    pub(crate) fn new() -> Self {
        let id = ExecutionId::generate().to_string().replace('-', "");
        Self(PathBuf::from(format!(
            "/tmp/pos-{}.sock",
            &id[id.len() - 12..]
        )))
    }
}

impl AsRef<Path> for SocketPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for SocketPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}
