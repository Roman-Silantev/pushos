//! Terminals PushOS runs itself.
//!
//! A surface with a dozen jobs on it cannot be a desktop with a dozen windows.
//! PushOS therefore runs its terminals as pseudo-terminals it owns: they start
//! without anything appearing on screen, they keep running when nothing is
//! looking, and a pad that means "the test run" means it whether or not a
//! window exists.
//!
//! The pieces:
//!
//! - [`PtyTerminals`] is the adapter, and the only part that knows what a
//!   pseudo-terminal is.
//! - [`TerminalSupervisor`] is the single owner of every terminal, and the only
//!   thing that changes the registry.
//! - [`TerminalRegistry`] holds what PushOS knows: names, status and a bounded
//!   tail of what each one said.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]
#![doc(html_no_source)]

mod pty;
mod registry;
mod session;
mod supervisor;
mod tail;
mod text;

pub use pty::PtyTerminals;
pub use registry::TerminalRegistry;
pub use session::{SUMMARY_LINES, TerminalSession, TerminalSummary};
pub use supervisor::{OpenTerminal, TerminalSupervisor};
pub use tail::OutputTail;
