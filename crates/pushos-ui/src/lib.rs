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

/// How many animation frames a second the surface is drawn at.
///
/// Everything that moves counts frames rather than reading a clock, so this is
/// what turns those counts into seconds. The render task derives its frame
/// interval from it, and the durations below are written against it: change
/// this one number and the motion keeps its timing.
pub const FRAMES_A_SECOND: u32 = 15;

pub use canvas::{Area, Canvas};
pub use mascot::{MASCOT, Mascot};
pub use renderer::{PushRenderer, RendererUnavailable};
pub use snapshot::{
    Focus, LedPlan, Notice, Overlay, PageView, SLOT_COUNT, SessionLine, Slot, Splash,
    SurfacePresence, Tone, UiSnapshot,
};
pub use text::{Align, FontUnavailable, TextRenderer, TextStyle};
pub use theme::{Theme, TypeScale};
pub use widgets::Layout;
