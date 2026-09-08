//! The pseudo-terminal adapter, against real processes.
//!
//! Everything else about terminals is tested against a fake. This file is the
//! part that cannot be: whether PushOS can actually start a program, read what
//! it says, type into it and know how it ended.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pushos_domain::error::ErrorClass;
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{
    TerminalEvent, TerminalHost, TerminalObserver, TerminalSpec, TerminalStatus,
};
use pushos_domain::terminal::TerminalTarget;
use pushos_terminal::{OpenTerminal, PtyTerminals, TerminalSupervisor};
use pushos_testkit::FixedRoot;
use tokio::sync::mpsc;

/// How long a test waits for a real process to say something.
///
/// Generous: a loaded machine starting a shell is not a failure.
const PATIENCE: Duration = Duration::from_secs(20);

/// Reports terminal activity onto a channel a test can await.
#[derive(Debug)]
struct Channel(mpsc::UnboundedSender<(SessionId, TerminalEvent)>);

impl TerminalObserver for Channel {
    fn observe(&self, terminal: &SessionId, event: TerminalEvent) {
        let _ = self.0.send((terminal.clone(), event));
    }
}

struct Harness {
    host: Arc<PtyTerminals>,
    events: mpsc::UnboundedReceiver<(SessionId, TerminalEvent)>,
}

impl Harness {
    fn new() -> Self {
        let (sender, events) = mpsc::unbounded_channel();
        Self {
            host: Arc::new(PtyTerminals::new(Arc::new(Channel(sender)))),
            events,
        }
    }

    /// Collects output until the terminal exits, returning both.
    async fn drain(&mut self) -> (String, Option<i32>) {
        let mut output = String::new();
        loop {
            let next = tokio::time::timeout(PATIENCE, self.events.recv()).await;
            match next.expect("the terminal produced nothing before the deadline") {
                Some((_, TerminalEvent::Output { text })) => output.push_str(&text),
                Some((_, TerminalEvent::Exited { code })) => return (output, code),
                None => panic!("the host stopped reporting before the terminal exited"),
            }
        }
    }

    /// Collects output until it contains `needle`.
    async fn wait_for(&mut self, needle: &str) -> String {
        let mut output = String::new();
        loop {
            if output.contains(needle) {
                return output;
            }
            let next = tokio::time::timeout(PATIENCE, self.events.recv()).await;
            match next.unwrap_or_else(|_| panic!("never saw `{needle}`; saw `{output}`")) {
                Some((_, TerminalEvent::Output { text })) => output.push_str(&text),
                Some((_, TerminalEvent::Exited { code })) => {
                    panic!("the terminal exited with {code:?} before saying `{needle}`")
                }
                None => panic!("the host stopped reporting"),
            }
        }
    }
}

fn spec(program: &str, args: &[&str]) -> TerminalSpec {
    TerminalSpec::new("test", program, std::env::temp_dir())
        .with_args(args.iter().map(|arg| (*arg).to_owned()))
}

#[tokio::test]
async fn a_program_runs_and_what_it_prints_comes_back() {
    let mut harness = Harness::new();
    harness
        .host
        .open(
            SessionId::new("echo"),
            spec("/bin/echo", &["hello from pushos"]),
        )
        .await
        .expect("echo exists on every Mac");

    let (output, code) = harness.drain().await;
    assert!(output.contains("hello from pushos"), "got `{output}`");
    assert_eq!(code, Some(0));
}

#[tokio::test]
async fn a_failing_program_reports_the_status_it_exited_with() {
    let mut harness = Harness::new();
    harness
        .host
        .open(SessionId::new("fail"), spec("/bin/sh", &["-c", "exit 3"]))
        .await
        .expect("sh exists on every Mac");

    let (_, code) = harness.drain().await;
    assert_eq!(code, Some(3));
}

#[tokio::test]
async fn everything_a_program_printed_arrives_before_its_exit_is_reported() {
    // The ordering guarantee the whole design rests on: the last thing a build
    // said must still be on the display after it finishes.
    let mut harness = Harness::new();
    harness
        .host
        .open(
            SessionId::new("ordered"),
            spec(
                "/bin/sh",
                &["-c", "for i in 1 2 3 4 5; do echo line $i; done"],
            ),
        )
        .await
        .expect("sh exists on every Mac");

    let (output, code) = harness.drain().await;
    assert_eq!(code, Some(0));
    for line in 1..=5 {
        assert!(output.contains(&format!("line {line}")), "got `{output}`");
    }
}

#[tokio::test]
async fn a_program_can_be_typed_into_and_answers() {
    let mut harness = Harness::new();
    let id = SessionId::new("shell");
    harness
        .host
        .open(id.clone(), spec("/bin/sh", &[]))
        .await
        .expect("sh exists on every Mac");

    harness
        .host
        .write(&id, "echo the-answer-is-42\n")
        .await
        .expect("the shell is running");
    let output = harness.wait_for("the-answer-is-42").await;
    assert!(output.contains("the-answer-is-42"), "got `{output}`");

    harness.host.close(&id).await.expect("it is still running");
}

#[tokio::test]
async fn a_program_is_told_how_big_its_terminal_is() {
    // Programs behave differently when they think they have no room, and a
    // terminal with no size at all makes some of them refuse to run.
    let mut harness = Harness::new();
    harness
        .host
        .open(
            SessionId::new("size"),
            spec("/bin/sh", &["-c", "stty size"]),
        )
        .await
        .expect("sh exists on every Mac");

    let (output, _) = harness.drain().await;
    assert!(output.contains("40 120"), "got `{output}`");
}

#[tokio::test]
async fn closing_a_terminal_stops_the_program_it_was_running() {
    let mut harness = Harness::new();
    let id = SessionId::new("sleeper");
    harness
        .host
        .open(id.clone(), spec("/bin/sh", &["-c", "sleep 300"]))
        .await
        .expect("sh exists on every Mac");

    harness.host.close(&id).await.expect("it is running");

    // Nothing here waits five minutes, so arriving at all is the assertion.
    let (_, code) = harness.drain().await;
    assert_ne!(code, Some(0), "a terminal that was stopped did not finish");
    assert_eq!(harness.host.open_count(), 0);
}

#[tokio::test]
async fn a_terminal_that_finishes_stops_accepting_input() {
    let mut harness = Harness::new();
    let id = SessionId::new("brief");
    harness
        .host
        .open(id.clone(), spec("/bin/echo", &["done"]))
        .await
        .expect("echo exists on every Mac");

    harness.drain().await;
    let error = harness
        .host
        .write(&id, "too late\n")
        .await
        .expect_err("the program has gone");
    assert_eq!(error.class(), ErrorClass::Validation);
}

#[tokio::test]
async fn starting_in_a_directory_that_is_not_there_says_so_plainly() {
    let harness = Harness::new();
    let missing = PathBuf::from("/pushos/no/such/place");
    let error = harness
        .host
        .open(
            SessionId::new("nowhere"),
            TerminalSpec::new("test", "/bin/sh", &missing),
        )
        .await
        .expect_err("the directory does not exist");

    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(
        error.to_string().contains("/pushos/no/such/place"),
        "the message should name the directory: `{error}`"
    );
}

#[tokio::test]
async fn a_program_that_does_not_exist_fails_at_the_start() {
    let harness = Harness::new();
    let error = harness
        .host
        .open(SessionId::new("absent"), spec("/pushos/not/a/program", &[]))
        .await
        .expect_err("there is no such program");

    assert!(
        error.to_string().contains("/pushos/not/a/program"),
        "the message should name the program: `{error}`"
    );
    assert_eq!(harness.host.open_count(), 0);
}

#[tokio::test]
async fn colour_and_cursor_control_never_reach_the_display() {
    let mut harness = Harness::new();
    harness
        .host
        .open(
            SessionId::new("coloured"),
            spec("/bin/sh", &["-c", "printf '\\033[32mgreen\\033[0m\\n'"]),
        )
        .await
        .expect("sh exists on every Mac");

    let (output, _) = harness.drain().await;
    assert!(output.contains("green"), "got `{output:?}`");
    assert!(
        !output.contains('\u{1b}'),
        "an escape reached the display: `{output:?}`"
    );
}

#[tokio::test]
async fn the_supervisor_runs_a_command_in_a_real_shell_and_keeps_the_answer() {
    // The whole path, end to end: open a shell, type a command, read the tail
    // the surface would show.
    let (sender, mut events) = mpsc::unbounded_channel();
    let host = Arc::new(PtyTerminals::new(Arc::new(Channel(sender))));
    let supervisor = TerminalSupervisor::new(host, Arc::new(FixedRoot::new(std::env::temp_dir())))
        .with_shell("/bin/sh");

    let opened = supervisor
        .open(OpenTerminal::named("scratch"))
        .await
        .expect("sh exists on every Mac");
    supervisor
        .run(
            &TerminalTarget::Named("scratch".to_owned()),
            "echo 77-passed",
        )
        .await
        .expect("the shell is running");

    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let next = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("the shell said nothing before the deadline");
        let Some((id, event)) = next else {
            panic!("the host stopped reporting")
        };
        supervisor.apply(&id, &event).await;

        let listed = supervisor.summaries().await;
        if listed.iter().any(|summary| {
            summary
                .recent
                .iter()
                .any(|line| line.contains("77-passed") && !line.contains("echo"))
        }) {
            break;
        }
    }

    let listed = supervisor.summaries().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, opened.id);
    assert_eq!(listed[0].status, TerminalStatus::Running);
    assert!(listed[0].selected);

    supervisor.shutdown().await;
}

#[tokio::test]
async fn closing_leaves_no_echo_behind_as_the_last_thing_it_said() {
    // Dropping the write end sends an end-of-file the terminal echoes as
    // `^D`, which would then be what the display showed for a job the
    // operator deliberately stopped.
    let mut harness = Harness::new();
    let id = SessionId::new("quiet");
    harness
        .host
        .open(
            id.clone(),
            spec("/bin/sh", &["-c", "echo working; sleep 300"]),
        )
        .await
        .expect("sh exists on every Mac");

    let before = harness.wait_for("working").await;
    harness.host.close(&id).await.expect("it is running");
    let (after, _) = harness.drain().await;

    let said = format!("{before}{after}");
    assert!(
        !said.contains("^D"),
        "an end-of-file echo reached the display: `{said}`"
    );
}

#[tokio::test]
async fn closing_a_terminal_twice_is_refused_rather_than_signalling_again() {
    let mut harness = Harness::new();
    let id = SessionId::new("twice");
    harness
        .host
        .open(id.clone(), spec("/bin/sh", &["-c", "sleep 300"]))
        .await
        .expect("sh exists on every Mac");

    harness.host.close(&id).await.expect("it is running");
    let error = harness
        .host
        .close(&id)
        .await
        .expect_err("it has already been asked to stop");
    assert_eq!(error.class(), ErrorClass::Validation);

    harness.drain().await;
}
