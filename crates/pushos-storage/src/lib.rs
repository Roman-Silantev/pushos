//! Local persistence.
//!
//! SQLite is the runtime's local truth. One task owns the connection and every
//! write goes through it, so there is a single, obvious answer to who may write
//! and in what order.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod connection;
mod error;
mod memory;
mod records;
mod schema;
mod writer;

pub use error::StorageError;
pub use records::{EventRecord, WorkspaceMemoryRow};
pub use writer::{Storage, StorageWriter};
