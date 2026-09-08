//! Hardware-free fakes for every PushOS port.
//!
//! The core of PushOS must be testable, and developable, with no Push 2
//! attached, no agent provider installed and no macOS permissions granted.
//! These fakes satisfy the same interfaces as the real adapters so that a test
//! exercises the production code path rather than a parallel one.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod clock;
mod fake_agent;
mod fake_push;
mod fake_runs;
mod fake_system;
mod fake_terminal;
mod fake_workspace;
mod recording_provider;

pub use clock::ManualClock;
pub use fake_agent::{AgentCall, FakeAgent};
pub use fake_push::{FakePush, FakePushInput, SurfaceState};
pub use fake_runs::FakeRunStore;
pub use fake_system::{FakeApplications, FakeMedia, FakeProcesses, FakeShortcuts, MediaCall};
pub use fake_terminal::{FakeTerminal, RecordingTerminalObserver};
pub use fake_workspace::{FakeRepository, FakeWorkspaceMemory, FixedRoot};
pub use recording_provider::{RecordingProvider, ScriptedOutcome};
