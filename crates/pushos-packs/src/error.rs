//! Why a pack could not be read, reviewed or installed.

use std::path::PathBuf;

use pushos_domain::ids::PackId;

/// What went wrong with a pack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackError {
    /// There is nothing at that path, or nothing that looks like a pack.
    #[error("`{path}` is not a pack; a pack is a directory with a `pack.toml` in it")]
    NotAPack {
        /// Where PushOS looked.
        path: PathBuf,
    },

    /// The manifest could not be read.
    #[error("`{path}` could not be read: {source}")]
    Unreadable {
        /// Which file.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },

    /// The manifest is not valid.
    #[error("`{path}`: {source}")]
    Malformed {
        /// Which file.
        path: PathBuf,
        /// Why not.
        #[source]
        source: toml::de::Error,
    },

    /// The manifest is well-formed but says something PushOS cannot use.
    #[error("`{pack}`: {reason}")]
    Invalid {
        /// Which pack.
        pack: String,
        /// What is wrong with it.
        reason: String,
    },

    /// A pack's own files try to grant permissions.
    ///
    /// Its own kind because it is the one rule that makes consent mean
    /// something: what a pack may do is what the operator agreed to, and a
    /// pack that could write its own grant would have made the review theatre.
    #[error(
        "`{pack}` grants permissions in `{}`; a pack declares what it needs in its manifest \
         and the operator grants it at install",
        file.display()
    )]
    GrantsItself {
        /// Which pack.
        pack: String,
        /// The file that tried.
        file: PathBuf,
    },

    /// The pack's configuration does not hold together, alone or with what is
    /// already installed.
    #[error("`{pack}` cannot be installed: {}", problems.join("; "))]
    WouldNotWork {
        /// Which pack.
        pack: String,
        /// What is wrong, all of it.
        problems: Vec<String>,
    },

    /// A pack with that identity is already installed.
    #[error("`{pack}` is already installed; remove it first, or pass --force to replace it")]
    AlreadyInstalled {
        /// Which pack.
        pack: PackId,
    },

    /// No pack with that identity is installed.
    #[error("no pack called `{pack}` is installed")]
    NotInstalled {
        /// Which pack was asked for.
        pack: PackId,
    },

    /// The pack could not be copied in or deleted.
    #[error("{context}: {source}")]
    Unwritable {
        /// What PushOS was attempting.
        context: String,
        /// Why it failed.
        #[source]
        source: std::io::Error,
    },
}

impl PackError {
    /// Reports a manifest that says something PushOS cannot use.
    pub(crate) fn invalid(pack: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            pack: pack.into(),
            reason: reason.into(),
        }
    }

    /// Reports a failure to copy or delete.
    pub(crate) fn unwritable(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Unwritable {
            context: context.into(),
            source,
        }
    }
}
