//! Action dispatch and the built-in providers.
//!
//! Every executable binding becomes an action, and every action is addressed as
//! `provider.verb`. The dispatcher knows nothing about what any verb does, so
//! adding a provider never touches the hardware adapter, the binding resolver
//! or the renderer.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod dispatcher;
pub mod providers;
mod registry;

pub use dispatcher::{ActionDispatcher, DEFAULT_ACTION_BUDGET};
pub use registry::{DuplicateProvider, ProviderRegistry};
