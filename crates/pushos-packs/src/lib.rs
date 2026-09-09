//! Installable packs.
//!
//! A pack is a directory of ordinary PushOS configuration with a manifest on
//! the front. Installing one copies it in; removing one deletes what was copied
//! and nothing else. There is no package database and no lock file: what is
//! installed is what is in the packs directory, which an operator can list with
//! `ls` and read with any editor.
//!
//! What the manifest adds beyond the files is consent. A pack declares what it
//! needs, PushOS shows that before anything is written, and the operator agrees
//! to a list they read. A pack cannot grant itself anything, which is enforced
//! rather than promised: a pack whose files contain a permissions section is
//! refused.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod error;
mod library;
mod manifest;
mod review;

pub use error::PackError;
pub use library::{GRANT_FILE, Library, check, install, installed, read, remove, set_state};
pub use review::Review;
