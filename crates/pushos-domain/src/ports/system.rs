//! Ports for the host operating system.
//!
//! These exist so that media, applications, Shortcuts and subprocesses can be
//! faked in tests, and so that no macOS API leaks into the domain.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::ActionError;

/// Controls whatever is currently playing.
#[async_trait]
pub trait MediaController: Send + Sync + std::fmt::Debug {
    /// Toggles playback.
    async fn play_pause(&self) -> Result<(), ActionError>;
    /// Skips forward one track.
    async fn next_track(&self) -> Result<(), ActionError>;
    /// Skips back one track.
    async fn previous_track(&self) -> Result<(), ActionError>;
    /// Sets output volume as a percentage, `0..=100`.
    async fn set_volume(&self, percent: u8) -> Result<(), ActionError>;
    /// Reads the current volume as a percentage, when the backend exposes one.
    async fn volume(&self) -> Result<Option<u8>, ActionError>;
    /// Reads what is playing, for the display overlay.
    async fn now_playing(&self) -> Result<Option<MediaSnapshot>, ActionError>;
}

/// What is currently playing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaSnapshot {
    /// Application providing the audio.
    pub source: String,
    /// Track title.
    pub title: String,
    /// Performing artist.
    pub artist: Option<String>,
    /// How far into the track playback has reached.
    pub position: Option<Duration>,
    /// Total track length.
    pub duration: Option<Duration>,
    /// Whether audio is advancing.
    pub playing: bool,
}

/// What an application action should open or focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationTarget {
    /// Bring an application to the front, launching it if needed.
    Application {
        /// The application's user-visible name.
        name: String,
    },
    /// Open a path, using the system's default handler.
    Path {
        /// The path to open.
        path: PathBuf,
        /// Open it with a named application instead of the default handler.
        with: Option<String>,
    },
    /// Open a URL in the default browser.
    Url {
        /// The URL to open.
        url: String,
    },
}

/// Launches and focuses applications.
#[async_trait]
pub trait ApplicationLauncher: Send + Sync + std::fmt::Debug {
    /// Opens or focuses the target.
    async fn open(&self, target: &ApplicationTarget) -> Result<(), ActionError>;
}

/// Runs Apple Shortcuts.
#[async_trait]
pub trait ShortcutRunner: Send + Sync + std::fmt::Debug {
    /// Lists the Shortcuts available to the current user.
    async fn list(&self) -> Result<Vec<String>, ActionError>;
    /// Runs a Shortcut by name, optionally passing it text input.
    async fn run(&self, name: &str, input: Option<&str>) -> Result<(), ActionError>;
}

/// A subprocess to run.
///
/// The program and its arguments are kept separate all the way to the syscall.
/// There is deliberately no field for a shell command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    /// The executable to run.
    pub program: String,
    /// Its arguments, passed without any shell interpretation.
    pub args: Vec<String>,
    /// Working directory, when it matters.
    pub cwd: Option<PathBuf>,
    /// How long to wait before giving up.
    pub timeout: Duration,
}

impl ProcessSpec {
    /// The default budget for a foreground action.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

    /// Builds a specification with the default budget.
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            cwd: None,
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Sets the working directory.
    #[must_use]
    pub fn in_directory(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets the time budget.
    #[must_use]
    pub const fn within(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// How a subprocess finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// Exit status, absent when the process was signalled.
    pub exit_code: Option<i32>,
    /// Trailing standard output, truncated for display.
    pub stdout_tail: String,
    /// Trailing standard error, truncated for display.
    pub stderr_tail: String,
}

impl ProcessOutcome {
    /// Whether the process reported success.
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs subprocesses.
#[async_trait]
pub trait ProcessRunner: Send + Sync + std::fmt::Debug {
    /// Runs the process to completion, or until its budget expires.
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutcome, ActionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_spec_keeps_arguments_separate_from_the_program() {
        let spec = ProcessSpec::new("cargo", ["test".to_owned(), "--all".to_owned()]);
        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.args, ["test", "--all"]);
        assert_eq!(spec.timeout, ProcessSpec::DEFAULT_TIMEOUT);
    }

    #[test]
    fn only_a_zero_exit_counts_as_success() {
        let ok = ProcessOutcome {
            exit_code: Some(0),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        };
        let signalled = ProcessOutcome {
            exit_code: None,
            ..ok.clone()
        };
        let failed = ProcessOutcome {
            exit_code: Some(1),
            ..ok.clone()
        };

        assert!(ok.succeeded());
        assert!(!signalled.succeeded());
        assert!(!failed.succeeded());
    }
}
