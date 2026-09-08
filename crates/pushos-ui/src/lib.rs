//! Rendering for the Push 2 display and lights.
//!
//! Rendering is deterministic and never waits. It consumes an immutable
//! snapshot and produces a frame, so it cannot be blocked behind a model, a
//! network call, the database or a subprocess.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod canvas;
mod mascot;
mod renderer;
mod snapshot;
mod text;
mod theme;
mod widgets;

pub use canvas::{Area, Canvas};
pub use mascot::{MASCOT, Mascot};
pub use renderer::{PushRenderer, RendererUnavailable};
pub use snapshot::{
    LedPlan, Notice, Overlay, PageView, SLOT_COUNT, Slot, Splash, Tone, UiSnapshot,
};
pub use text::{Align, FontUnavailable, TextRenderer, TextStyle};
pub use theme::{Theme, TypeScale};
pub use widgets::Layout;
