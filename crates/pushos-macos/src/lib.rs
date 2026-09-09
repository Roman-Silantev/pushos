//! macOS implementations of the PushOS system ports.
//!
//! Everything here is an adapter. The action providers depend on the ports in
//! `pushos-domain`, never on this crate, so PushOS can be built and tested
//! without a Mac and ported to another host by adding a sibling crate.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod applications;
mod media;
mod process;
mod script;
mod shortcuts;
mod terminals;

pub use applications::OpenLauncher;
pub use media::{AppleScriptMedia, DEFAULT_PLAYER};
pub use process::SystemProcessRunner;
pub use script::{ApplicationName, ScriptRunner, UnsafeApplicationName};
pub use shortcuts::ShortcutsCli;
pub use terminals::TerminalAppSessions;
