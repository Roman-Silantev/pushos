//! Runtime wiring and supervision.
//!
//! State is owned by the component responsible for it and mutated through
//! messages, rather than shared behind one global lock. Every long-running task
//! has an owner, supports cancellation, and reports its failures.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod actors;
mod app;
mod bus;
mod shutdown;
mod supervisor;

pub use actors::{InputTask, RenderTask, SurfaceState, SurfaceView};
pub use app::Runtime;
pub use bus::{EventBus, EventSubscription};
pub use shutdown::{SHUTDOWN_GRACE, Shutdown};
pub use supervisor::{Backoff, INITIAL_BACKOFF, MAXIMUM_BACKOFF};
