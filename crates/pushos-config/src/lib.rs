//! Declarative configuration.
//!
//! Human-readable TOML is the source of truth. PushOS Studio edits these files;
//! it does not own a database of its own, and closing it changes nothing.
//!
//! A reload is atomic: parse, validate, build a whole new configuration, then
//! swap it in. Invalid configuration never replaces what is running.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod bindings;
mod build;
mod editor;
mod error;
mod loader;
mod memory;
pub mod model;
pub mod paths;
mod sequences;
mod spec;
mod store;
mod surface;
mod voice;
mod watcher;
mod workflows;

pub use build::RuntimeConfig;
pub use editor::{BindingEdit, ConfigDocuments, PageRemoval};
pub use error::{ConfigError, Problem};
pub use loader::{load, load_pack};
pub use memory::MemorySettings;
pub use model::{
    BindingEntry, ConfigFile, MemorySection, MemorySourceEntry, PageEntry, SessionSection,
    VoiceCommandEntry, VoiceSection,
};
pub use spec::{BindingAddress, BindingSpec, PageSpec};
pub use store::ConfigStore;
pub use voice::VoiceSettings;
pub use watcher::{ConfigWatcher, DEBOUNCE, WatchError};
