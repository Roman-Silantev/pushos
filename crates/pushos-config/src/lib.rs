//! Declarative configuration.
//!
//! Human-readable TOML is the source of truth. PushOS Studio edits these files;
//! it does not own a database of its own, and closing it changes nothing.
//!
//! A reload is atomic: parse, validate, build a whole new configuration, then
//! swap it in. Invalid configuration never replaces what is running.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod build;
mod editor;
mod error;
mod loader;
pub mod model;
pub mod paths;
mod spec;
mod store;
mod watcher;

pub use build::RuntimeConfig;
pub use editor::{BindingEdit, ConfigDocuments};
pub use error::{ConfigError, Problem};
pub use loader::load;
pub use model::{BindingEntry, ConfigFile, PageEntry};
pub use spec::{BindingAddress, BindingSpec};
pub use store::ConfigStore;
pub use watcher::{ConfigWatcher, DEBOUNCE, WatchError};
