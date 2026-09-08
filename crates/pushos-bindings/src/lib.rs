//! Gesture recognition and binding resolution.
//!
//! This crate turns normalised hardware input into the single action a gesture
//! means, in three steps:
//!
//! 1. [`GestureRecognizer`] owns all gesture timing for the whole system.
//! 2. [`BindingTable`] indexes the bindings currently in force.
//! 3. [`BindingResolver`] picks the one binding that applies to the current
//!    page and workspace.
//!
//! Precedence is fixed at `workspace+page`, `workspace`, `page`, `global`.
//! Configurations that precedence cannot separate are rejected by
//! [`validation::find_conflicts`] rather than resolved arbitrarily.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

pub mod gesture;
mod resolver;
mod table;
pub mod validation;

pub use gesture::{
    GestureDetail, GestureEvent, GestureInterest, GestureRecognizer, GestureTiming, InterestKind,
    InvalidTiming,
};
pub use resolver::{BindingResolver, ControlLookup};
pub use table::BindingTable;
pub use validation::{BindingConflict, find_conflicts};
