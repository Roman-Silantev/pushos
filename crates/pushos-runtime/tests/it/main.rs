//! The runtime, driven end to end with no hardware attached.
//!
//! One test binary rather than one per file. Every integration test target
//! links the crate and everything beneath it, so each extra file was another
//! full copy of the dependency graph on disk and another link to wait for.
//! As modules of one binary they share both.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod control_socket;
mod end_to_end;
mod memory;
mod reconnect;
mod voice;
mod workspaces;
