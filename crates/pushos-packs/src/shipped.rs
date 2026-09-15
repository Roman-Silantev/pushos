//! The packs PushOS ships with.
//!
//! Compiled in, the way the presets are, so every PushOS can offer them without
//! the repository beside it. A pack is read and installed from a directory, so
//! a shipped one is written out to a cache first and is then an ordinary pack:
//! the same review, the same checks, the same install.

use std::path::{Path, PathBuf};

/// One file of a shipped pack.
#[derive(Clone, Copy, Debug)]
pub struct ShippedFile {
    /// Where it goes, relative to the pack's directory.
    pub path: &'static str,
    /// What is in it.
    pub contents: &'static str,
}

/// A pack PushOS ships with.
#[derive(Clone, Copy, Debug)]
pub struct ShippedPack {
    /// Its directory name, which is also its identity.
    pub name: &'static str,
    /// Every file in it.
    pub files: &'static [ShippedFile],
}

macro_rules! shipped {
    ($pack:literal: $($path:literal),+ $(,)?) => {
        ShippedPack {
            name: $pack,
            files: &[$(ShippedFile {
                path: $path,
                contents: include_str!(concat!("../../../packs/", $pack, "/", $path)),
            }),+],
        }
    };
}

/// Every pack PushOS ships with.
///
/// Listed file by file, and held to the directory by a test, so a file added
/// to a pack and not here fails the build rather than going missing from it.
pub const SHIPPED: &[ShippedPack] = &[
    shipped!("operator":
        "pack.toml",
        "README.md",
        "agents/1-build.toml",
        "agents/2-clients.toml",
        "agents/3-growth.toml",
        "agents/4-money.toml",
        "agents/5-markets.toml",
        "agents/6-comms.toml",
        "agents/7-mine.toml",
        "bindings/default.toml",
        "pages/operator.toml",
    ),
    shipped!("review":
        "pack.toml",
        "README.md",
        "agents/reviewer.toml",
        "bindings/default.toml",
        "pages/review.toml",
        "workflows/review.toml",
    ),
];

impl ShippedPack {
    /// Writes the pack out under `cache`, and says where it is.
    ///
    /// Written only when the copy there differs, so reading the list of packs
    /// on offer again and again touches nothing.
    pub fn written_to(&self, cache: &Path) -> std::io::Result<PathBuf> {
        let root = cache.join(self.name);
        for file in self.files {
            let target = root.join(file.path);
            if std::fs::read_to_string(&target).is_ok_and(|held| held == file.contents) {
                continue;
            }
            if let Some(directory) = target.parent() {
                std::fs::create_dir_all(directory)?;
            }
            std::fs::write(&target, file.contents)?;
        }
        Ok(root)
    }
}

/// Where shipped packs are written out, for this version of PushOS.
///
/// Per version, so an upgrade never offers the previous version's files.
pub fn cache_directory() -> PathBuf {
    std::env::temp_dir()
        .join("pushos-shipped-packs")
        .join(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_on_disk(directory: &Path, prefix: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                files_on_disk(&path, prefix, found);
            } else {
                found.push(
                    path.strip_prefix(prefix)
                        .expect("inside")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }

    #[test]
    fn every_file_of_every_shipped_pack_is_compiled_in() {
        let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        for pack in SHIPPED {
            let root = packs.join(pack.name);
            let mut on_disk = Vec::new();
            files_on_disk(&root, &root, &mut on_disk);
            on_disk.sort();

            let mut listed: Vec<String> =
                pack.files.iter().map(|file| file.path.to_owned()).collect();
            listed.sort();
            assert_eq!(
                listed, on_disk,
                "`{}` ships different files than it has",
                pack.name
            );
        }
    }

    #[test]
    fn every_pack_in_the_repository_is_shipped() {
        let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        for entry in std::fs::read_dir(packs).expect("readable") {
            let path = entry.expect("an entry").path();
            if path.join("pack.toml").is_file() {
                let name = path
                    .file_name()
                    .expect("named")
                    .to_string_lossy()
                    .into_owned();
                assert!(
                    SHIPPED.iter().any(|pack| pack.name == name),
                    "`{name}` is in packs/ and not shipped"
                );
            }
        }
    }

    #[test]
    fn a_shipped_pack_written_out_reads_as_a_pack() {
        let cache =
            std::env::temp_dir().join(format!("pushos-shipped-test-{}", std::process::id()));
        for pack in SHIPPED {
            let root = pack.written_to(&cache).expect("writable");
            let read = crate::read(&root).expect("a shipped pack reads");
            assert_eq!(read.id.as_str(), pack.name);
            // And writing it out again changes nothing.
            pack.written_to(&cache).expect("still writable");
        }
        std::fs::remove_dir_all(cache).ok();
    }
}
