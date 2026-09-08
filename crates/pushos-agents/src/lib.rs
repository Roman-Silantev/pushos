//! Agent roles, live sessions, and the routing between them.
//!
//! A binding names a role, not a process. This crate is what turns "the builder
//! in this workspace" into a session that exists, starting one if it has to, so
//! a pad configured today still means something tomorrow when the provider's
//! own session identifier has long expired.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod registry;
mod router;
mod session;
mod supervisor;

pub use registry::SessionRegistry;
pub use router::{AgentRoster, AgentRouter, Resolution, RoutingError, StartRequest};
pub use session::{PendingApproval, Session};
pub use supervisor::{AgentSupervisor, SilentObserver, SupervisorObserver};
