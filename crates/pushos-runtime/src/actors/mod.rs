//! The components that own runtime state.

mod agents;
mod input;
pub(crate) mod leds;
mod render;
pub(crate) mod slots;
mod surface;

pub(crate) use agents::lines_for;
pub use agents::{AgentReporter, AgentTask};
pub use input::{InputTask, SurfaceView};
pub use render::RenderTask;
pub use surface::SurfaceState;
