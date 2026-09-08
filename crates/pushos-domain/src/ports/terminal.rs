//! The terminal port.
//!
//! PushOS runs terminals it manages itself rather than opening windows. A
//! surface with a dozen jobs on it cannot be a desktop with a dozen windows,
//! and a pad that means "the test run" has to mean it whether or not anything
//! is on screen.
//!
//! Opening a real terminal window is a separate, deliberate action: an operator
//! asking to look at something, not the normal way work runs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::ids::SessionId;

/// What to run, and where.
///
/// Program and arguments stay separate, as everywhere else in PushOS: nothing
/// configured or transcribed is ever reinterpreted as shell syntax. What the
/// operator subsequently types into the terminal is a different matter, and is
/// exactly what a terminal is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSpec {
    /// What the operator calls it.
    pub name: String,
    /// The program to run.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The directory it starts in.
    pub cwd: PathBuf,
    /// Environment to set for it.
    pub env: BTreeMap<String, String>,
    /// How wide the terminal believes it is.
    pub size: TerminalSize,
}

impl TerminalSpec {
    /// Builds a specification at the default size.
    pub fn new(
        name: impl Into<String>,
        program: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            size: TerminalSize::DEFAULT,
        }
    }

    /// Sets the arguments.
    #[must_use]
    pub fn with_args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args = args.into_iter().collect();
        self
    }
}

/// How large a terminal believes its window is.
///
/// Nothing displays these terminals, but programs behave differently when they
/// think they have no room, so PushOS gives them a sensible one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    /// Columns.
    pub columns: u16,
    /// Rows.
    pub rows: u16,
}

impl TerminalSize {
    /// A comfortable size for a program that will not be looked at directly.
    pub const DEFAULT: Self = Self {
        columns: 120,
        rows: 40,
    };
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A terminal the host has opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalHandle {
    /// What PushOS calls it.
    pub id: SessionId,
    /// The process identifier, when the host knows one.
    ///
    /// For reporting only. PushOS ends terminals through the host, never by
    /// signalling a number it is holding.
    pub pid: Option<u32>,
}

/// What a terminal is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalStatus {
    /// Coming up.
    Starting,
    /// Running.
    Running,
    /// Finished.
    Exited {
        /// Its exit status, absent when it was signalled.
        code: Option<i32>,
    },
    /// Ended because PushOS was asked to end it.
    ///
    /// Distinct from [`Self::Exited`] with no status, which looks identical
    /// from the outside. A red light for a job the operator deliberately
    /// stopped is a lie the surface would keep telling.
    Stopped,
    /// It could not be started, or it stopped abnormally.
    Failed,
}

impl TerminalStatus {
    /// Whether the terminal is still there to talk to.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    /// Whether it finished the way it was supposed to.
    pub const fn succeeded(self) -> bool {
        matches!(self, Self::Exited { code: Some(0) })
    }

    /// Whether anything went wrong.
    ///
    /// Being stopped is not going wrong: it is the operator getting what they
    /// asked for.
    pub const fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Failed
                | Self::Exited {
                    code: Some(1..) | None
                }
        )
    }

    /// The light that communicates this status.
    pub const fn status_color(self) -> crate::color::StatusColor {
        use crate::color::StatusColor;
        match self {
            Self::Starting | Self::Running => StatusColor::Working,
            Self::Exited { code: Some(0) } => StatusColor::Complete,
            Self::Stopped => StatusColor::Idle,
            Self::Exited { .. } | Self::Failed => StatusColor::Failed,
        }
    }
}

/// Something a terminal did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// It produced output.
    ///
    /// Arrives continuously and may be coalesced for display, but the tail a
    /// consumer keeps must always be the most recent.
    Output {
        /// What it wrote.
        text: String,
    },
    /// It finished.
    Exited {
        /// Its exit status, absent when it was signalled.
        code: Option<i32>,
    },
}

/// Receives everything a terminal does.
pub trait TerminalObserver: Send + Sync + std::fmt::Debug {
    /// Reports one thing a terminal did.
    ///
    /// Must not block: this is called from the thread reading the terminal, and
    /// holding it up stalls the program on the other end.
    fn observe(&self, terminal: &SessionId, event: TerminalEvent);
}

/// Runs terminals on the host.
#[async_trait]
pub trait TerminalHost: Send + Sync + std::fmt::Debug {
    /// Opens a terminal and starts its program.
    async fn open(
        &self,
        id: SessionId,
        spec: TerminalSpec,
    ) -> Result<TerminalHandle, TerminalError>;

    /// Types into a terminal.
    ///
    /// The text is written exactly as given, including any newline. A caller
    /// wanting a command to run must say so.
    async fn write(&self, terminal: &SessionId, input: &str) -> Result<(), TerminalError>;

    /// Tells a terminal it has been resized.
    async fn resize(&self, terminal: &SessionId, size: TerminalSize) -> Result<(), TerminalError>;

    /// Ends a terminal and releases what it was holding.
    async fn close(&self, terminal: &SessionId) -> Result<(), TerminalError>;
}

/// Why a terminal operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TerminalError {
    /// PushOS does not know that terminal.
    #[error("terminal `{terminal}` is not open")]
    NoSuchTerminal {
        /// The terminal that was named.
        terminal: SessionId,
    },

    /// The terminal has finished and cannot be talked to.
    #[error("terminal `{terminal}` has finished")]
    Finished {
        /// The terminal that was named.
        terminal: SessionId,
    },

    /// The host reported a fault.
    #[error("{context}")]
    Host {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl TerminalError {
    /// Classifies the failure.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            Self::NoSuchTerminal { .. } | Self::Finished { .. } => ErrorClass::Validation,
            Self::Host { class, .. } => *class,
        }
    }

    /// Wraps a host fault with the context that makes it actionable.
    pub fn host(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Host {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_terminal_is_live_and_a_finished_one_is_not() {
        assert!(TerminalStatus::Starting.is_live());
        assert!(TerminalStatus::Running.is_live());
        assert!(!TerminalStatus::Exited { code: Some(0) }.is_live());
        assert!(!TerminalStatus::Stopped.is_live());
        assert!(!TerminalStatus::Failed.is_live());
    }

    #[test]
    fn a_terminal_the_operator_stopped_is_not_a_failure() {
        // The surface would otherwise show red for the operator getting
        // exactly what they asked for.
        assert!(!TerminalStatus::Stopped.is_failure());
        assert_ne!(
            TerminalStatus::Stopped.status_color(),
            crate::color::StatusColor::Failed
        );

        assert!(TerminalStatus::Failed.is_failure());
        assert!(TerminalStatus::Exited { code: Some(1) }.is_failure());
        assert!(TerminalStatus::Exited { code: None }.is_failure());
        assert!(!TerminalStatus::Exited { code: Some(0) }.is_failure());
    }

    #[test]
    fn only_a_zero_exit_counts_as_success() {
        assert!(TerminalStatus::Exited { code: Some(0) }.succeeded());
        assert!(!TerminalStatus::Exited { code: Some(1) }.succeeded());
        assert!(!TerminalStatus::Exited { code: None }.succeeded());
        assert!(!TerminalStatus::Failed.succeeded());
    }

    #[test]
    fn a_terminal_that_failed_never_shows_as_finished_cleanly() {
        use crate::color::StatusColor;
        assert_eq!(
            TerminalStatus::Exited { code: Some(0) }.status_color(),
            StatusColor::Complete
        );
        assert_eq!(
            TerminalStatus::Exited { code: Some(1) }.status_color(),
            StatusColor::Failed
        );
        assert_eq!(TerminalStatus::Failed.status_color(), StatusColor::Failed);
    }

    #[test]
    fn a_specification_keeps_arguments_separate_from_the_program() {
        let spec = TerminalSpec::new("tests", "cargo", "/tmp/project")
            .with_args(["test".to_owned(), "--all".to_owned()]);

        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.args, ["test", "--all"]);
        assert_eq!(spec.size, TerminalSize::DEFAULT);
    }

    #[test]
    fn a_terminal_is_given_a_size_a_program_can_work_with() {
        // Programs behave differently when they think they have no room, and
        // some refuse to run at all in a terminal smaller than a classic one.
        let size = TerminalSpec::new("t", "sh", "/tmp").size;
        assert!(size.columns >= 80, "{} columns is too narrow", size.columns);
        assert!(size.rows >= 24, "{} rows is too short", size.rows);
    }

    #[test]
    fn naming_a_terminal_that_is_not_open_is_a_validation_error() {
        let error = TerminalError::NoSuchTerminal {
            terminal: SessionId::new("gone"),
        };
        assert_eq!(error.class(), crate::error::ErrorClass::Validation);
    }
}
