//! Work that runs itself.
//!
//! A workflow is a graph the operator wrote: plan, then build, then test, then
//! review, with somewhere to go when the tests fail. One pad starts it, and it
//! carries on without anyone watching.
//!
//! What makes it a runtime rather than a script is that it survives PushOS
//! stopping. A step is written down before the step after it runs, so a crash
//! loses at most the step that was in flight, and what a restart reads is never
//! ahead of what actually happened.
//!
//! The pieces:
//!
//! - [`WorkflowCatalogue`] is what the operator configured, checked once so a
//!   run can never reach a step that is not there.
//! - [`WorkflowEngine`] runs them, and is the only thing that decides where a
//!   run goes next.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]
#![doc(html_no_source)]

mod catalogue;
mod engine;

pub use catalogue::{WorkflowCatalogue, WorkflowProblem, problems_with};
pub use engine::{RunObserver, SilentObserver, WorkflowEngine, WorkflowError};
