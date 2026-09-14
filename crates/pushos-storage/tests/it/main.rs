//! The database, against real files.
//!
//! One test binary rather than one per file. Every integration test target
//! links the crate and everything beneath it, so each extra file was another
//! full copy of the dependency graph on disk and another link to wait for.
//! As modules of one binary they share both.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod notes;
mod persistence;
