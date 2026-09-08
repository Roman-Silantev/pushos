//! What PushOS knows about one terminal.

use std::path::PathBuf;
use std::time::Instant;

use pushos_domain::ids::{SessionId, WorkspaceId};
use pushos_domain::ports::{TerminalEvent, TerminalHandle, TerminalSpec, TerminalStatus};

use crate::tail::OutputTail;

/// How many lines a summary carries.
///
/// The Push 2 display can show a couple of lines of output next to everything
/// else it has to say; Studio shows more, but not a scrollback.
pub const SUMMARY_LINES: usize = 6;

/// One terminal, and everything the surface needs to say about it.
#[derive(Debug)]
pub struct TerminalSession {
    /// What PushOS calls it.
    pub id: SessionId,
    /// What the operator calls it, and what a binding names.
    pub name: String,
    /// The program it is running.
    pub program: String,
    /// Where it is running.
    pub cwd: PathBuf,
    /// Which workspace it belongs to.
    pub workspace: Option<WorkspaceId>,
    /// Its process identifier, for reporting.
    pub pid: Option<u32>,
    /// What it is doing.
    pub status: TerminalStatus,
    /// When it opened.
    pub started_at: Instant,
    /// When it last did anything.
    pub last_activity: Instant,
    /// The end of its output. Bounded; the rest is gone.
    output: OutputTail,
}

impl TerminalSession {
    /// Records a terminal that has just opened.
    pub fn opened(
        handle: &TerminalHandle,
        spec: &TerminalSpec,
        workspace: Option<WorkspaceId>,
        now: Instant,
    ) -> Self {
        Self {
            id: handle.id.clone(),
            name: spec.name.clone(),
            program: spec.program.clone(),
            cwd: spec.cwd.clone(),
            workspace,
            pid: handle.pid,
            status: TerminalStatus::Starting,
            started_at: now,
            last_activity: now,
            output: OutputTail::new(),
        }
    }

    /// Applies something the terminal did.
    pub fn apply(&mut self, event: &TerminalEvent, now: Instant) {
        self.last_activity = now;
        match event {
            TerminalEvent::Output { text } => {
                // The first thing a program says is the proof it started.
                if self.status == TerminalStatus::Starting {
                    self.status = TerminalStatus::Running;
                }
                self.output.push(text);
            }
            TerminalEvent::Exited { code } => {
                self.status = TerminalStatus::Exited { code: *code };
            }
        }
    }

    /// Marks a terminal that ended because PushOS was asked to end it.
    pub fn stopped(&mut self, now: Instant) {
        self.status = TerminalStatus::Stopped;
        self.last_activity = now;
    }

    /// Marks a terminal that never got going.
    pub fn failed(&mut self, now: Instant) {
        self.status = TerminalStatus::Failed;
        self.last_activity = now;
    }

    /// The last line worth showing.
    pub fn last_line(&self) -> Option<&str> {
        self.output.last_meaningful_line()
    }

    /// The end of the output, oldest first.
    pub fn recent_output(&self, lines: usize) -> Vec<&str> {
        self.output.recent(lines)
    }

    /// Whether it can still be typed into.
    pub fn is_live(&self) -> bool {
        self.status.is_live()
    }

    /// A cheap, owned view for the display and for Studio.
    pub fn summary(&self, selected: bool) -> TerminalSummary {
        TerminalSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            program: self.program.clone(),
            status: self.status,
            pid: self.pid,
            selected,
            last_line: self.last_line().map(ToOwned::to_owned),
            recent: self
                .recent_output(SUMMARY_LINES)
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
        }
    }
}

/// What a terminal looks like from outside.
///
/// Copied rather than borrowed: the registry keeps changing while the surface
/// is being drawn, and nothing outside it should hold a reference to a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSummary {
    /// What PushOS calls it.
    pub id: SessionId,
    /// What the operator calls it.
    pub name: String,
    /// The program it is running.
    pub program: String,
    /// What it is doing.
    pub status: TerminalStatus,
    /// Its process identifier.
    pub pid: Option<u32>,
    /// Whether unqualified actions act on it.
    pub selected: bool,
    /// The last line worth showing.
    pub last_line: Option<String>,
    /// The end of its output, oldest first.
    pub recent: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TerminalSpec {
        TerminalSpec::new("tests", "cargo", "/tmp")
    }

    fn session() -> TerminalSession {
        let handle = TerminalHandle {
            id: SessionId::new("t1"),
            pid: Some(42),
        };
        TerminalSession::opened(&handle, &spec(), None, Instant::now())
    }

    #[test]
    fn a_new_terminal_is_starting_and_has_said_nothing() {
        let session = session();
        assert_eq!(session.status, TerminalStatus::Starting);
        assert_eq!(session.last_line(), None);
        assert!(session.is_live());
    }

    #[test]
    fn the_first_output_is_what_proves_it_started() {
        let mut session = session();
        session.apply(
            &TerminalEvent::Output {
                text: "compiling\n".to_owned(),
            },
            Instant::now(),
        );

        assert_eq!(session.status, TerminalStatus::Running);
        assert_eq!(session.last_line(), Some("compiling"));
    }

    #[test]
    fn exiting_ends_the_session_and_keeps_what_it_said() {
        let mut session = session();
        let now = Instant::now();
        session.apply(
            &TerminalEvent::Output {
                text: "77 passed\n".to_owned(),
            },
            now,
        );
        session.apply(&TerminalEvent::Exited { code: Some(0) }, now);

        assert_eq!(session.status, TerminalStatus::Exited { code: Some(0) });
        assert!(!session.is_live());
        assert_eq!(
            session.last_line(),
            Some("77 passed"),
            "a finished terminal still has to say how it went"
        );
    }

    #[test]
    fn activity_moves_forward_with_every_event() {
        let mut session = session();
        let opened = session.last_activity;
        let later = opened + std::time::Duration::from_secs(5);

        session.apply(
            &TerminalEvent::Output {
                text: "x".to_owned(),
            },
            later,
        );
        assert_eq!(session.last_activity, later);
        assert_eq!(session.started_at, opened, "the start does not move");
    }

    #[test]
    fn a_summary_carries_what_the_surface_shows_and_nothing_it_holds() {
        let mut session = session();
        let now = Instant::now();
        for line in 0..20 {
            session.apply(
                &TerminalEvent::Output {
                    text: format!("line {line}\n"),
                },
                now,
            );
        }

        let summary = session.summary(true);
        assert!(summary.selected);
        assert_eq!(summary.name, "tests");
        assert_eq!(summary.pid, Some(42));
        assert_eq!(summary.recent.len(), SUMMARY_LINES);
        assert_eq!(summary.last_line.as_deref(), Some("line 19"));
    }

    #[test]
    fn a_terminal_that_never_started_is_failed_rather_than_finished() {
        let mut session = session();
        session.failed(Instant::now());

        assert_eq!(session.status, TerminalStatus::Failed);
        assert!(!session.status.succeeded());
        assert!(!session.is_live());
    }
}
