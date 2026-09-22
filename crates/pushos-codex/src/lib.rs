//! A whole fleet of Codex threads in one process.
//!
//! PushOS's other agent adapter speaks the Agent Client Protocol, which starts
//! one process per session. That is the right shape for Claude Code, whose
//! sessions are processes whatever anyone does, and the wrong shape for a
//! fleet: measured on an M4, a Claude Code session costs around 440 MB, so a
//! dozen is what a laptop holds.
//!
//! Codex's app-server hosts many threads in one process instead. Sixty-four
//! idle threads measured at about 30 MB between them, and a thread opens in
//! roughly a tenth of a second. That is the difference between a surface that
//! shows a dozen agents and one that shows sixty-four.
//!
//! Three pieces: [`AppServer`] owns the process and the protocol, [`mapping`]
//! turns Codex's vocabulary into the domain's, and [`CodexBackend`] implements
//! the agent port in terms of both.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod backend;
mod error;
pub mod mapping;
mod server;

pub use backend::CodexBackend;
pub use error::CodexError;
pub use server::{AppServer, Incoming};
