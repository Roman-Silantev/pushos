//! The live configuration, and how it is replaced.
//!
//! Readers take a snapshot without blocking and without a lock. A reload builds
//! a whole new configuration first and swaps it in only if it is valid, so an
//! invalid edit leaves the running surface exactly as it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{info, warn};

use crate::build::RuntimeConfig;
use crate::error::ConfigError;
use crate::loader;

/// Holds the configuration currently in force.
#[derive(Debug)]
pub struct ConfigStore {
    current: ArcSwap<RuntimeConfig>,
    root: PathBuf,
}

impl ConfigStore {
    /// Builds a store holding an empty configuration.
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self {
            current: ArcSwap::from_pointee(RuntimeConfig::empty()),
            root: root.into(),
        }
    }

    /// Builds a store by loading `root`.
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let root = root.into();
        let config = RuntimeConfig::build(&loader::load(&root)?)?;
        Ok(Self {
            current: ArcSwap::from_pointee(config),
            root,
        })
    }

    /// The root the store reads from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The configuration currently in force.
    ///
    /// Cheap enough to call on the input path: no lock is taken and no work is
    /// done beyond one atomic load.
    pub fn current(&self) -> Arc<RuntimeConfig> {
        self.current.load_full()
    }

    /// Re-reads the root and replaces the configuration if it is valid.
    ///
    /// Returns the new configuration on success. On failure the running
    /// configuration is untouched, and the caller decides whether to tell the
    /// operator.
    pub fn reload(&self) -> Result<Arc<RuntimeConfig>, ConfigError> {
        let candidate = Arc::new(RuntimeConfig::build(&loader::load(&self.root)?)?);
        info!(
            bindings = candidate.bindings.len(),
            pages = candidate.pages.len(),
            "configuration replaced"
        );
        self.current.store(Arc::clone(&candidate));
        Ok(candidate)
    }

    /// Reloads, logging and swallowing a failure.
    ///
    /// Used by the file watcher, where a half-written file is an ordinary event
    /// rather than a fault: the operator is still typing.
    pub fn reload_or_keep(&self) -> bool {
        match self.reload() {
            Ok(_) => true,
            Err(error) => {
                warn!(%error, "keeping the running configuration");
                for problem in error.problems() {
                    warn!(%problem, "configuration problem");
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::ExecutionId;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pushos-store-{}", ExecutionId::generate()));
            std::fs::create_dir_all(&path).expect("the temporary directory is writable");
            Self(path)
        }

        fn write_main(&self, text: &str) {
            std::fs::write(self.0.join("pushos.toml"), text).expect("writable");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const ONE_PAGE: &str = r#"
        [[pages]]
        id = "home"
        name = "Home"

        [[bindings]]
        control = "button.play"
        gesture = "press"
        action = "media.play_pause"
    "#;

    #[test]
    fn an_empty_store_has_nothing_configured() {
        let store = ConfigStore::empty("/nowhere");
        assert!(store.current().pages.is_empty());
        assert!(store.current().bindings.is_empty());
    }

    #[test]
    fn a_valid_reload_replaces_the_configuration() {
        let scratch = Scratch::new();
        scratch.write_main(ONE_PAGE);

        let store = ConfigStore::load(&scratch.0).expect("the configuration is valid");
        assert_eq!(store.current().pages.len(), 1);

        scratch.write_main(&format!(
            "{ONE_PAGE}\n[[pages]]\nid = \"music\"\nname = \"Music\"\n"
        ));
        store.reload().expect("the new configuration is valid");
        assert_eq!(store.current().pages.len(), 2);
    }

    #[test]
    fn an_invalid_reload_leaves_the_running_configuration_untouched() {
        let scratch = Scratch::new();
        scratch.write_main(ONE_PAGE);

        let store = ConfigStore::load(&scratch.0).expect("valid");
        let before = store.current();

        scratch.write_main(
            "[[bindings]]\ncontrol = \"pad.400\"\ngesture = \"tap\"\naction = \"page.next\"\n",
        );
        assert!(store.reload().is_err());

        let after = store.current();
        assert_eq!(after.pages.len(), before.pages.len());
        assert_eq!(after.bindings.len(), before.bindings.len());
    }

    #[test]
    fn a_half_written_file_does_not_take_the_surface_down() {
        let scratch = Scratch::new();
        scratch.write_main(ONE_PAGE);
        let store = ConfigStore::load(&scratch.0).expect("valid");

        scratch.write_main("[[bindings]]\ncontrol = ");
        assert!(
            !store.reload_or_keep(),
            "the reload should be reported as unsuccessful"
        );
        assert_eq!(
            store.current().bindings.len(),
            1,
            "the running surface is unchanged"
        );
    }

    #[test]
    fn a_snapshot_taken_before_a_reload_stays_valid_afterwards() {
        let scratch = Scratch::new();
        scratch.write_main(ONE_PAGE);
        let store = ConfigStore::load(&scratch.0).expect("valid");

        let held = store.current();
        scratch.write_main("[[pages]]\nid = \"music\"\nname = \"Music\"\n");
        store.reload().expect("valid");

        // An action resolved against the old configuration must still be able
        // to read it while it finishes.
        assert_eq!(held.pages.len(), 1);
        assert_eq!(store.current().pages.len(), 1);
        assert_eq!(store.current().pages[0].id.as_str(), "music");
    }
}
