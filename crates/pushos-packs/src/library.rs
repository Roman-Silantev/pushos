//! Reading packs, and what installing one does.
//!
//! Installing is a directory copy and a permission grant, in that order, with
//! nothing written until the whole thing has been checked. Removing is a
//! directory delete. There is no package database: what is installed is what is
//! in the packs directory.

use std::path::{Path, PathBuf};

use pushos_config::model::ConfigFile;
use pushos_config::paths;
use pushos_domain::ids::PackId;
use pushos_domain::pack::{Contents, Installed, Pack, PackState};
use pushos_domain::permissions::Permission;
use tracing::{info, warn};

use crate::error::PackError;
use crate::manifest::Manifest;

/// The file the installer writes when the operator grants what a pack asked for.
///
/// Written by PushOS rather than shipped by the pack, and named so that anyone
/// reading the directory can see which grants came from a decision and which
/// came from a file. Deleting it withdraws them.
pub const GRANT_FILE: &str = "00-granted.toml";

/// Reads a pack from a directory.
///
/// Reads its manifest, counts what it contributes, and refuses a pack whose own
/// files try to grant permissions. That last check is what makes the review
/// mean something.
pub fn read(directory: &Path) -> Result<Pack, PackError> {
    let manifest_path = directory.join(paths::PACK_MANIFEST);
    if !manifest_path.is_file() {
        return Err(PackError::NotAPack {
            path: directory.to_path_buf(),
        });
    }

    let text = std::fs::read_to_string(&manifest_path).map_err(|source| PackError::Unreadable {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|source| PackError::Malformed {
        path: manifest_path,
        source,
    })?;

    let contributed = contributions(&manifest.id, directory)?;
    manifest.into_pack(contributed)
}

/// What a pack's files add, and a refusal if they grant anything.
///
/// The grant file is skipped, because PushOS wrote it. Nothing else may say
/// what a pack is allowed to do, and a pack shipping a file by that name is
/// refused at install rather than quietly trusted.
fn contributions(id: &str, directory: &Path) -> Result<Contents, PackError> {
    let mut contents = Contents::default();

    for path in paths::files_in_pack(directory) {
        if path.file_name().is_some_and(|name| name == GRANT_FILE) {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| PackError::Unreadable {
            path: path.clone(),
            source,
        })?;
        let parsed: ConfigFile = toml::from_str(&text).map_err(|source| PackError::Malformed {
            path: path.clone(),
            source,
        })?;

        // The one rule that makes consent mean something. What a pack may do is
        // what the operator agreed to, and a pack able to write its own grant
        // would have made the review theatre.
        if !parsed.permissions.granted.is_empty() {
            return Err(PackError::GrantsItself {
                pack: id.to_owned(),
                file: path,
            });
        }

        contents.agents += parsed.agents.len();
        contents.workflows += parsed.workflows.len();
        contents.pages += parsed.pages.len();
        contents.bindings += parsed.bindings.len();
        contents.phrases += parsed.voice.commands.len();
        contents.note_sources += parsed.memory.sources.len();
    }

    Ok(contents)
}

/// The packs installed under a configuration root.
///
/// A directory that is not a pack is reported and skipped rather than failing
/// the listing: one broken pack should not hide the others.
pub fn installed(config_root: &Path) -> Vec<Installed> {
    let mut directories: Vec<PathBuf> = std::fs::read_dir(config_root.join(paths::PACK_DIRECTORY))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();

    directories
        .into_iter()
        .filter_map(|root| match read(&root) {
            Ok(pack) => Some(Installed {
                pack,
                state: state_of(&root),
                root,
            }),
            Err(error) => {
                warn!(%error, path = %root.display(), "an installed pack could not be read");
                None
            }
        })
        .collect()
}

/// Whether an installed pack is in force.
fn state_of(root: &Path) -> PackState {
    if root.join(paths::PACK_DISABLED).exists() {
        PackState::Disabled
    } else {
        PackState::Enabled
    }
}

/// Installs a pack, granting what the operator agreed to.
///
/// Everything is checked before anything is written: the pack alone, the pack
/// against what is already there, and the result as a whole. A half-installed
/// pack would leave a surface nobody chose.
pub fn install(
    source: &Path,
    config_root: &Path,
    granting: &[Permission],
    replace: bool,
) -> Result<Pack, PackError> {
    // Checked before the manifest is even trusted: PushOS writes the grant
    // file, and a pack arriving with one would be writing its own permissions
    // under a name the reader is told to skip.
    if source.join(GRANT_FILE).exists() {
        return Err(PackError::GrantsItself {
            pack: source.display().to_string(),
            file: source.join(GRANT_FILE),
        });
    }

    let pack = read(source)?;
    let target = config_root
        .join(paths::PACK_DIRECTORY)
        .join(pack.id.as_str());

    if target.exists() {
        if !replace {
            return Err(PackError::AlreadyInstalled {
                pack: pack.id.clone(),
            });
        }
        remove_directory(&target)?;
    }

    // Checked in place, before anything is copied. A pack that would make the
    // surface ambiguous is refused with the bindings named rather than
    // installed and then reported at every startup.
    if let Err(problems) = would_work(source, config_root, &pack, granting) {
        return Err(PackError::WouldNotWork {
            pack: pack.id.to_string(),
            problems,
        });
    }

    copy_directory(source, &target)?;
    write_grant(&target, &pack, granting)?;

    info!(pack = %pack.id, version = %pack.version, adds = %pack.adds, "pack installed");
    Ok(pack)
}

/// Whether a pack would install cleanly here, without installing it.
///
/// What `pushos pack show` reports, so an operator is told before they agree
/// rather than after. Returns every problem rather than the first, the way the
/// configuration builder does, so one editing pass can fix a whole install.
pub fn check(
    source: &Path,
    config_root: &Path,
    granting: &[Permission],
) -> Result<(), Vec<String>> {
    let pack = read(source).map_err(|error| vec![error.to_string()])?;
    would_work(source, config_root, &pack, granting)
}

/// Whether the configuration would still hold together with this pack in it.
fn would_work(
    source: &Path,
    config_root: &Path,
    pack: &Pack,
    granting: &[Permission],
) -> Result<(), Vec<String>> {
    let mut merged = pushos_config::load(config_root).map_err(|error| {
        vec![format!(
            "the configuration already there could not be read: {error}"
        )]
    })?;

    for path in paths::files_in_pack(source) {
        if path.file_name().is_some_and(|name| name == GRANT_FILE) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Err(vec![format!("`{}` could not be read", path.display())]);
        };
        match toml::from_str::<ConfigFile>(&text) {
            Ok(parsed) => merged.merge(parsed),
            Err(error) => return Err(vec![format!("`{}`: {error}", path.display())]),
        }
    }

    for permission in granting {
        merged.permissions.granted.push(*permission);
    }

    pushos_config::RuntimeConfig::build(&merged)
        .map(|_| ())
        .map_err(|error| {
            let problems: Vec<String> = error.problems().iter().map(ToString::to_string).collect();
            if problems.is_empty() {
                vec![error.to_string()]
            } else {
                problems
            }
        })?;

    // Reported rather than refused: a pack whose files are fine but which
    // contributes nothing is odd, and the operator should hear about it.
    if pack.adds.is_empty() {
        warn!(pack = %pack.id, "the pack contributes nothing to the surface");
    }
    Ok(())
}

/// Writes what the operator granted, as a file PushOS owns.
fn write_grant(target: &Path, pack: &Pack, granting: &[Permission]) -> Result<(), PackError> {
    if granting.is_empty() {
        return Ok(());
    }

    let names: Vec<String> = granting
        .iter()
        .map(|permission| format!("    \"{permission}\","))
        .collect();
    let text = format!(
        "# Granted when `{}` was installed, not by the pack itself.\n\
         # Delete this file to withdraw these; the pack stays installed.\n\
         [permissions]\n\
         granted = [\n{}\n]\n",
        pack.id,
        names.join("\n")
    );

    std::fs::write(target.join(GRANT_FILE), text)
        .map_err(|source| PackError::unwritable("writing what was granted", source))
}

/// Removes an installed pack.
///
/// Deletes what was copied in and nothing else. Anything the operator wrote
/// themselves lives outside the pack directory and is untouched.
pub fn remove(pack: &PackId, config_root: &Path) -> Result<(), PackError> {
    let target = config_root.join(paths::PACK_DIRECTORY).join(pack.as_str());
    if !target.is_dir() {
        return Err(PackError::NotInstalled { pack: pack.clone() });
    }

    remove_directory(&target)?;
    info!(%pack, "pack removed");
    Ok(())
}

/// Turns an installed pack on or off.
///
/// Disabling keeps the files, so turning a pack off does not cost the operator
/// whatever they changed inside it.
pub fn set_state(pack: &PackId, config_root: &Path, state: PackState) -> Result<(), PackError> {
    let target = config_root.join(paths::PACK_DIRECTORY).join(pack.as_str());
    if !target.is_dir() {
        return Err(PackError::NotInstalled { pack: pack.clone() });
    }

    let marker = target.join(paths::PACK_DISABLED);
    match state {
        PackState::Enabled => {
            if marker.exists() {
                std::fs::remove_file(&marker)
                    .map_err(|source| PackError::unwritable("enabling the pack", source))?;
            }
        }
        PackState::Disabled => {
            std::fs::write(
                &marker,
                "This pack is installed and switched off. Delete this file to switch it on.\n",
            )
            .map_err(|source| PackError::unwritable("disabling the pack", source))?;
        }
    }

    info!(%pack, state = state.describe(), "pack state changed");
    Ok(())
}

/// Copies a pack in, keeping its shape.
fn copy_directory(source: &Path, target: &Path) -> Result<(), PackError> {
    std::fs::create_dir_all(target)
        .map_err(|error| PackError::unwritable(format!("making `{}`", target.display()), error))?;

    let entries = std::fs::read_dir(source)
        .map_err(|error| PackError::unwritable(format!("reading `{}`", source.display()), error))?;

    for entry in entries.flatten() {
        let from = entry.path();
        let name = entry.file_name();
        // Whatever a pack was developed under travels with the author, not with
        // the pack: `.git` is theirs and is nothing to do with the surface.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let to = target.join(&name);

        if from.is_dir() {
            copy_directory(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| {
                PackError::unwritable(format!("copying `{}`", from.display()), error)
            })?;
        }
    }

    Ok(())
}

fn remove_directory(target: &Path) -> Result<(), PackError> {
    std::fs::remove_dir_all(target)
        .map_err(|error| PackError::unwritable(format!("removing `{}`", target.display()), error))
}

/// Somewhere packs are installed.
///
/// A thin handle so a caller does not repeat the configuration root at every
/// call, and so the CLI and Studio ask the same thing the same way.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    /// Builds a library over a configuration root.
    pub fn at(config_root: impl Into<PathBuf>) -> Self {
        Self {
            root: config_root.into(),
        }
    }

    /// What is installed.
    pub fn installed(&self) -> Vec<Installed> {
        installed(&self.root)
    }

    /// One installed pack.
    pub fn find(&self, pack: &PackId) -> Option<Installed> {
        self.installed()
            .into_iter()
            .find(|held| held.pack.id == *pack)
    }

    /// Installs a pack, granting what the operator agreed to.
    pub fn install(
        &self,
        source: &Path,
        granting: &[Permission],
        replace: bool,
    ) -> Result<Pack, PackError> {
        install(source, &self.root, granting, replace)
    }

    /// Removes an installed pack.
    pub fn remove(&self, pack: &PackId) -> Result<(), PackError> {
        remove(pack, &self.root)
    }

    /// Turns an installed pack on or off.
    pub fn set_state(&self, pack: &PackId, state: PackState) -> Result<(), PackError> {
        set_state(pack, &self.root, state)
    }

    /// Where packs are installed.
    pub fn root(&self) -> &Path {
        &self.root
    }
}
