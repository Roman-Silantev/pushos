//! The components that own runtime state.

mod agents;
mod attached;
mod input;
pub(crate) mod leds;
mod render;
mod sessions;
pub(crate) mod slots;
mod surface;
mod terminals;
mod workflows;

pub(crate) use agents::lines_for as agent_lines;
pub use agents::{AgentReporter, AgentTask};
pub(crate) use attached::AttachedTask;
pub(crate) use attached::lines_for as attached_lines;
pub use input::{InputTask, SurfaceView};
pub use render::RenderTask;
pub(crate) use sessions::SessionPublisher;
pub use surface::SurfaceState;
pub(crate) use terminals::describe as describe_terminal;
pub(crate) use terminals::lines_for as terminal_lines;
pub use terminals::{TerminalReporter, TerminalTask};
pub(crate) use workflows::describe as describe_run;
pub(crate) use workflows::lines_for as run_lines;
pub use workflows::{RunReporter, WorkflowTask};
