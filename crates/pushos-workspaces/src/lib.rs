//! Projects.
//!
//! A workspace is the answer to "which project am I in". One pad selects it,
//! and everything downstream follows: bindings scoped to it come into force,
//! agents start in its repository with the providers it prefers, terminals open
//! there, and the page it was last on comes back.
//!
//! The pieces:
//!
//! - [`WorkspaceRegistry`] is what the operator configured, and is never
//!   edited by PushOS.
//! - [`WorkspaceManager`] owns which project is in effect, remembers what each
//!   was doing, and hands out working trees.
//! - [`GitRepository`] is the adapter, and the only part that knows about git.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]
#![doc(html_no_source)]

mod forgetful;
mod git;
mod leases;
mod manager;
mod registry;

pub use forgetful::ForgetfulMemory;
pub use git::GitRepository;
pub use leases::WorktreeLeases;
pub use manager::{Arrival, WorkspaceManager, describe_missing_app, warn_if_missing};
pub use registry::{UnknownWorkspace, WorkspaceRegistry};
