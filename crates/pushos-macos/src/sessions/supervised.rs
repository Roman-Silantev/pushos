//! Sessions a coding agent keeps for PushOS, without a terminal of its own.
//!
//! A session in a terminal is a program running: it holds its memory whether
//! it is working or waiting, and on a Mac with sixty-four pads that is more
//! memory than there is. Claude Code will instead keep a session itself:
//! started with `--bg` it runs under Claude Code's own supervisor, `stop`
//! ends its process while keeping the conversation, and asking for it again
//! starts it where it left off.
//!
//! That is what lets a pad stand for a session that costs nothing until it is
//! wanted. What a pad holds is the short identifier Claude Code prints, which
//! stays the same across being put away and asked for again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pushos_domain::attached::Activity;
use pushos_domain::ports::{AttachError, ProcessRunner, ProcessSpec};
use tracing::debug;

use super::programs;

/// How long a command about these sessions may take.
///
/// Starting one waits for Claude Code to write it down and print its
/// identifier, which is quick; it does not wait for the work to be done.
const PATIENCE: Duration = Duration::from_secs(20);

/// How much of a session's recent output is kept when it is read.
const LOG: usize = 64 * 1024;

/// One session Claude Code is keeping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Supervised {
    /// The short identifier `claude attach`, `logs` and `stop` take.
    pub(super) id: String,
    /// The conversation, for asking Claude Code to carry it on.
    pub(super) session: String,
    /// What Claude Code calls it, which is the work it was given.
    pub(super) name: String,
    /// Where it is working.
    pub(super) directory: PathBuf,
    /// What it appears to be doing.
    pub(super) activity: Activity,
    /// Whether a process is running for it now.
    ///
    /// One that is not still exists, still holds its conversation, and still
    /// belongs on a pad; it simply costs nothing.
    pub(super) running: bool,
}

/// Runs the commands that start, stop and reach these sessions.
#[derive(Debug)]
pub(super) struct Supervisor {
    processes: Arc<dyn ProcessRunner>,
    /// Where Claude Code is, when that is already known.
    program: Option<PathBuf>,
}

impl Supervisor {
    /// Speaks to the Claude Code installed for this user.
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            processes,
            program: None,
        }
    }

    /// Speaks to a particular Claude Code.
    #[cfg(test)]
    pub(super) fn at(processes: Arc<dyn ProcessRunner>, program: &str) -> Self {
        Self {
            processes,
            program: Some(PathBuf::from(program)),
        }
    }

    /// Starts a session on some work, and returns what to call it by.
    ///
    /// The work is what Claude Code names the session after, so a pad that
    /// says what it is for gets a session that says the same.
    pub(super) async fn start(&self, directory: &Path, work: &str) -> Result<String, AttachError> {
        let mut spec = self.command(["--bg".to_owned(), work.to_owned()])?;
        spec = spec.in_directory(directory);

        let outcome = self.run(spec).await?;
        let id = identifier(&outcome).ok_or_else(|| {
            AttachError::unavailable(
                "Claude Code started a session but did not say what it is called",
            )
        })?;
        debug!(session = %id, directory = %directory.display(), "started a session Claude Code keeps");
        Ok(id)
    }

    /// Gives a session its next instruction, starting it again if it was put
    /// away.
    pub(super) async fn instruct(
        &self,
        session: &str,
        directory: &Path,
        text: &str,
    ) -> Result<(), AttachError> {
        let spec = self
            .command([
                "--bg".to_owned(),
                "--resume".to_owned(),
                session.to_owned(),
                text.to_owned(),
            ])?
            .in_directory(directory);
        self.run(spec).await.map(|_| ())
    }

    /// Ends a session's process, keeping its conversation.
    pub(super) async fn put_away(&self, id: &str) -> Result<(), AttachError> {
        let spec = self.command(["stop".to_owned(), id.to_owned()])?;
        self.run(spec).await?;
        debug!(session = %id, "put a session away");
        Ok(())
    }

    /// The last of what a session has said.
    pub(super) async fn read(&self, id: &str, most: usize) -> Result<String, AttachError> {
        let spec = self
            .command(["logs".to_owned(), id.to_owned()])?
            .capturing(LOG);
        let outcome = self.run(spec).await?;
        let output = outcome.stdout_tail;
        let from = output.len().saturating_sub(most);
        Ok(output[from..].to_owned())
    }

    /// What to run in a window to take one over.
    ///
    /// The arguments rather than a line, so nothing an identifier contains is
    /// ever read by a shell.
    pub(super) fn attach_command(&self, id: &str) -> Option<Vec<String>> {
        let program = self.program()?;
        Some(vec![
            program.to_string_lossy().into_owned(),
            "attach".to_owned(),
            id.to_owned(),
        ])
    }

    /// Where Claude Code is installed, if it is.
    fn program(&self) -> Option<PathBuf> {
        self.program.clone().or_else(|| programs::find("claude"))
    }

    fn command(&self, args: impl IntoIterator<Item = String>) -> Result<ProcessSpec, AttachError> {
        let program = self.program().ok_or_else(|| {
            AttachError::unavailable("Claude Code is not installed, so it can keep no sessions")
        })?;
        Ok(ProcessSpec::new(program.to_string_lossy(), args).within(PATIENCE))
    }

    async fn run(
        &self,
        spec: ProcessSpec,
    ) -> Result<pushos_domain::ports::ProcessOutcome, AttachError> {
        let outcome = self
            .processes
            .run(&spec)
            .await
            .map_err(|error| AttachError::unavailable(error.to_string()))?;
        if !outcome.succeeded() {
            let said = outcome.stderr_tail.trim();
            return Err(AttachError::unavailable(if said.is_empty() {
                "Claude Code refused".to_owned()
            } else {
                said.to_owned()
            }));
        }
        Ok(outcome)
    }
}

/// Reads the identifier out of what starting a session printed.
///
/// Claude Code prints the ways to reach the new session rather than the bare
/// identifier, so the identifier is taken from the first of them.
fn identifier(outcome: &pushos_domain::ports::ProcessOutcome) -> Option<String> {
    let printed = format!("{}\n{}", outcome.stdout_tail, outcome.stderr_tail);
    printed
        .split_whitespace()
        .skip_while(|word| *word != "attach")
        .nth(1)
        .filter(|id| id.chars().all(|c| c.is_ascii_hexdigit()) && !id.is_empty())
        .map(ToOwned::to_owned)
}

/// What a state Claude Code reports means for a pad.
///
/// A session that failed asks for a person as surely as one that is blocked:
/// both are stuck, and the pad's job is to say so.
pub(super) fn activity_of(state: Option<&str>, running: bool) -> Activity {
    // Nothing is happening in a session that was put away, and nothing can be
    // answered in it either, so it says so rather than keeping the colour it
    // had when it stopped.
    if !running {
        return Activity::Quiet;
    }
    match state {
        Some("working") => Activity::Working,
        Some("blocked" | "failed") => Activity::NeedsDecision,
        // Done, or a state a later Claude Code invented: it is there, and it
        // is not asking for anything.
        _ => Activity::Ready,
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::ProcessOutcome;
    use pushos_testkit::FakeProcesses;

    use super::*;

    fn outcome(stdout: &str) -> ProcessOutcome {
        ProcessOutcome {
            exit_code: Some(0),
            stdout_tail: stdout.to_owned(),
            stderr_tail: String::new(),
        }
    }

    #[test]
    fn the_identifier_is_read_out_of_what_starting_one_printed() {
        let printed = outcome(
            "  claude attach 55c88714    open in this terminal\n  \
             claude logs 55c88714      show recent output\n",
        );
        assert_eq!(identifier(&printed).as_deref(), Some("55c88714"));
    }

    #[test]
    fn printing_something_else_entirely_is_not_taken_for_an_identifier() {
        assert_eq!(identifier(&outcome("started, somewhere")), None);
        assert_eq!(identifier(&outcome("claude attach")), None);
        assert_eq!(identifier(&outcome("claude attach not-hex")), None);
    }

    #[tokio::test]
    async fn starting_one_runs_claude_in_the_folder_it_is_for() {
        let processes = FakeProcesses::new();
        processes.reply_with(|_| outcome("claude attach abc123de   open in this terminal"));
        let supervisor = Supervisor::at(Arc::new(processes.clone()), "/usr/bin/claude");

        let id = supervisor
            .start(Path::new("/tmp/work"), "tidy the tests")
            .await
            .expect("started");

        assert_eq!(id, "abc123de");
        let spec = &processes.spawned()[0];
        assert_eq!(spec.program, "/usr/bin/claude");
        assert_eq!(spec.args, ["--bg", "tidy the tests"]);
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("/tmp/work")));
    }

    #[tokio::test]
    async fn an_instruction_carries_the_conversation_on_rather_than_starting_another() {
        let processes = FakeProcesses::new();
        let supervisor = Supervisor::at(Arc::new(processes.clone()), "/usr/bin/claude");

        supervisor
            .instruct(
                "55c88714-1080-4f39-b0dc-ad15435085da",
                Path::new("/tmp/work"),
                "carry on",
            )
            .await
            .expect("instructed");

        let spec = &processes.spawned()[0];
        assert_eq!(
            spec.args,
            [
                "--bg",
                "--resume",
                "55c88714-1080-4f39-b0dc-ad15435085da",
                "carry on"
            ]
        );
    }

    #[tokio::test]
    async fn putting_one_away_stops_it_by_its_short_name() {
        let processes = FakeProcesses::new();
        let supervisor = Supervisor::at(Arc::new(processes.clone()), "/usr/bin/claude");

        supervisor.put_away("55c88714").await.expect("put away");

        assert_eq!(processes.spawned()[0].args, ["stop", "55c88714"]);
    }

    #[tokio::test]
    async fn a_refusal_says_what_claude_code_said() {
        let processes = FakeProcesses::new();
        processes.reply_with(|_| ProcessOutcome {
            exit_code: Some(1),
            stdout_tail: String::new(),
            stderr_tail: "no session 55c88714".to_owned(),
        });
        let supervisor = Supervisor::at(Arc::new(processes.clone()), "/usr/bin/claude");

        let refused = supervisor.put_away("55c88714").await.expect_err("refused");
        assert!(
            refused.to_string().contains("no session 55c88714"),
            "{refused}"
        );
    }

    #[test]
    fn what_a_session_is_doing_is_read_from_what_claude_code_calls_it() {
        assert_eq!(activity_of(Some("working"), true), Activity::Working);
        assert_eq!(activity_of(Some("blocked"), true), Activity::NeedsDecision);
        assert_eq!(activity_of(Some("done"), true), Activity::Ready);
        assert_eq!(
            activity_of(Some("done"), false),
            Activity::Quiet,
            "put away, and costing nothing"
        );
        assert_eq!(activity_of(Some("invented-later"), true), Activity::Ready);
    }
}
