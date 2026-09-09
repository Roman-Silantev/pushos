//! Reading configuration off disk.

use std::path::Path;

use tracing::debug;

use crate::error::ConfigError;
use crate::model::ConfigFile;
use crate::paths;

/// Reads and merges every configuration file under `root`.
///
/// `root` may be a single file or a directory holding `pushos.toml` and an
/// optional `conf.d`. A missing root is not an error: PushOS starts with an
/// empty surface and waits for one to be written.
pub fn load(root: &Path) -> Result<ConfigFile, ConfigError> {
    let mut merged = ConfigFile::default();

    for path in paths::files_for(root) {
        debug!(?path, "reading configuration");
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Unreadable {
            path: path.clone(),
            source,
        })?;
        let parsed: ConfigFile =
            toml::from_str(&text).map_err(|source| ConfigError::Malformed {
                path: path.clone(),
                source,
            })?;
        merged.merge(parsed);
    }

    Ok(merged)
}

/// Reads and merges the configuration inside one pack directory.
///
/// The same reading as [`load`], over the files a pack contributes rather than
/// the ones an operator wrote. Used to say what a pack would add before it is
/// installed, and what it did add afterwards.
pub fn load_pack(pack: &Path) -> Result<ConfigFile, ConfigError> {
    let mut merged = ConfigFile::default();

    for path in paths::files_in_pack(pack) {
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Unreadable {
            path: path.clone(),
            source,
        })?;
        let parsed: ConfigFile =
            toml::from_str(&text).map_err(|source| ConfigError::Malformed {
                path: path.clone(),
                source,
            })?;
        merged.merge(parsed);
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use pushos_domain::ids::ExecutionId;

    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pushos-loader-{}", ExecutionId::generate()));
            std::fs::create_dir_all(&path).expect("the temporary directory is writable");
            Self(path)
        }

        fn write(&self, name: &str, text: &str) {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("writable");
            }
            std::fs::write(path, text).expect("writable");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_missing_root_loads_an_empty_configuration() {
        let config = load(Path::new("/nowhere/at/all")).expect("a missing root is not an error");
        assert!(config.pages.is_empty());
    }

    #[test]
    fn a_directory_merges_its_main_file_and_includes() {
        let scratch = Scratch::new();
        scratch.write(
            "pushos.toml",
            "[runtime]\nhome_page = \"home\"\n\n[[pages]]\nid = \"home\"\nname = \"Home\"\n",
        );
        scratch.write(
            "conf.d/music.toml",
            "[[pages]]\nid = \"music\"\nname = \"Music\"\n",
        );

        let config = load(&scratch.0).expect("both files are well-formed");
        assert_eq!(config.pages.len(), 2);
        assert_eq!(config.runtime.home_page.as_deref(), Some("home"));
    }

    #[test]
    fn a_malformed_file_names_itself() {
        let scratch = Scratch::new();
        scratch.write("pushos.toml", "this is not = = toml");

        let error = load(&scratch.0).expect_err("the file is malformed");
        assert!(error.to_string().contains("pushos.toml"));
    }

    #[test]
    fn one_bad_include_fails_the_whole_load_rather_than_being_skipped() {
        let scratch = Scratch::new();
        scratch.write("pushos.toml", "[[pages]]\nid = \"home\"\nname = \"Home\"\n");
        scratch.write("conf.d/broken.toml", "control = = ");

        assert!(
            load(&scratch.0).is_err(),
            "a partial configuration must never be built"
        );
    }

    #[test]
    fn a_single_file_root_loads_that_file() {
        let scratch = Scratch::new();
        scratch.write("custom.toml", "[[pages]]\nid = \"home\"\nname = \"Home\"\n");

        let config = load(&scratch.0.join("custom.toml")).expect("well-formed");
        assert_eq!(config.pages.len(), 1);
    }
}
