//! The interfaces the domain owns and infrastructure implements.
//!
//! Every dependency arrow crossing this boundary points inward: adapters depend
//! on these traits, never the reverse.

mod action_provider;
mod agent;
mod clock;
mod push;
mod system;
mod terminal;

pub use action_provider::{ActionProvider, ProviderCapabilities};
pub use agent::{
    AgentBackend, AgentCapabilities, AgentError, AgentEvent, AgentObserver, ApprovalId,
    ApprovalOption, SessionHandle, SessionRequest, StopReason,
};
pub use clock::{Clock, SystemClock};
pub use push::{
    DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayFrame, PushInput, PushOutput, PushSurfaceError,
    SurfaceKind,
};
pub use system::{
    ApplicationLauncher, ApplicationTarget, MediaController, MediaSnapshot, ProcessOutcome,
    ProcessRunner, ProcessSpec, ShortcutRunner,
};
pub use terminal::{
    TerminalError, TerminalEvent, TerminalHandle, TerminalHost, TerminalObserver, TerminalSize,
    TerminalSpec, TerminalStatus,
};
