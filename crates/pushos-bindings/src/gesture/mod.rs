//! Gesture recognition.
//!
//! One component owns every hold timer, double-tap window and Shift decision in
//! PushOS. Features consume [`GestureEvent`]s and never observe raw timing.

mod event;
mod interest;
mod recognizer;
mod state;
mod timing;

pub use event::{GestureDetail, GestureEvent};
pub use interest::{GestureInterest, InterestKind};
pub use recognizer::GestureRecognizer;
pub use timing::{GestureTiming, InvalidTiming};
