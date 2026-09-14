//! macOS implementations of the PushOS system ports.
//!
//! Everything here is an adapter. The action providers depend on the ports in
//! `pushos-domain`, never on this crate, so PushOS can be built and tested
//! without a Mac and ported to another host by adding a sibling crate.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

pub mod app;
mod applications;
mod media;
/// Hearing the Mac go to sleep, so the Push 2 is not left lit all night.
///
/// The only module in this crate allowed `unsafe`, because there is no other
/// way to be told before macOS sleeps: the notification is a C callback from
/// IOKit, and it must be answered.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod power;
mod process;
mod script;
mod shortcuts;
mod terminals;

pub use applications::OpenLauncher;
pub use media::{AppleScriptMedia, DEFAULT_PLAYER};
#[cfg(target_os = "macos")]
pub use power::{SleepWatch, SleepWatchError};
pub use process::SystemProcessRunner;
pub use script::{ApplicationName, ScriptRunner, UnsafeApplicationName};
pub use shortcuts::ShortcutsCli;
pub use terminals::TerminalAppSessions;
