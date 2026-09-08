//! The Agent Client Protocol adapter.
//!
//! PushOS's connection to Claude, Codex, Cursor and anything else that speaks
//! the protocol. This crate is the only place that knows what those tools are
//! called, and even here they are commands in configuration rather than names
//! in code.
//!
//! Each session gets its own agent process. That costs a subprocess and buys
//! isolation: a session cannot see another's work, and an agent that crashes
//! takes down only its own.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod backend;
mod command;
mod launcher;
pub mod mapping;
mod session;

pub use backend::AcpBackend;
pub use launcher::{AgentCommand, Workspace};
