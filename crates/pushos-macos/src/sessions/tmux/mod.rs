//! Sessions kept by tmux.
//!
//! tmux keeps a terminal running with no window attached, and lets any number
//! of windows show it. That makes it the host for one session per pad: PushOS
//! starts or finds a session by name, types into it and reads it without
//! bringing anything to the front, and the operator opens it wherever they
//! like, in Terminal or in their editor's terminal, with `tmux attach`.
//!
//! Every command is an argument vector handed to tmux. Nothing here is a shell
//! line, and nothing typed into a session can change which command runs.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pushos_domain::attached::is_session_name;
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{
    AttachError, Key, OpenSession, Opened, ProcessOutcome, ProcessRunner, ProcessSpec,
};

pub(super) use self::listing::{Client, Layout, Pane};
use self::listing::{FIELD, RECORD, parse_listing, parse_screens};
use super::programs;

mod listing;

/// How many rows from the bottom of each pane to read while looking.
///
/// Enough for a question and the prompt beneath it, which is all the reading
/// of a screen looks at.
pub(super) const ROWS: u32 = 16;

/// How much history the focused view reads.
const HISTORY: u32 = 400;

/// How long any one tmux command may take.
///
/// tmux answers from memory in a millisecond or two. Anything slower is a
/// server in trouble, and a look that waited on it would hold up the next.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long to leave between pasting text and pressing Enter.
///
/// An agent that watches for pasted text treats a return arriving with the
/// paste as part of it, and would put a new line in the prompt instead of
/// sending it.
const SETTLE: Duration = Duration::from_millis(100);

/// The size of a session started with no window to take its size from.
///
/// Wide enough that an agent does not wrap everything it says. A window that
/// attaches later resizes it to itself.
const DETACHED_SIZE: (&str, &str) = ("200", "50");

/// Talks to a tmux server.
#[derive(Debug)]
pub(super) struct Tmux {
    processes: Arc<dyn ProcessRunner>,
    /// Where tmux is, when that is already known. Looked for on every use
    /// otherwise, so tmux installed while PushOS runs is found without a
    /// restart.
    program: Option<PathBuf>,
    /// A private server, for tests. The operator's own server otherwise, which
    /// is the one `tmux attach` in any terminal reaches.
    socket: Option<String>,
    /// Numbers the paste buffers, so two sends at once cannot swap text.
    buffers: AtomicU64,
}

impl Tmux {
    /// Talks to the operator's own tmux server.
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            processes,
            program: None,
            socket: None,
            buffers: AtomicU64::new(0),
        }
    }

    /// Talks to a private server of this name instead.
    #[cfg(test)]
    pub(super) fn on_socket(processes: Arc<dyn ProcessRunner>, socket: &str) -> Self {
        Self {
            socket: Some(socket.to_owned()),
            ..Self::new(processes)
        }
    }

    /// Talks to tmux at this path, whether or not anything is there.
    #[cfg(test)]
    pub(super) fn at(processes: Arc<dyn ProcessRunner>, program: &str) -> Self {
        Self {
            program: Some(PathBuf::from(program)),
            ..Self::new(processes)
        }
    }

    /// Where tmux is installed, if it is.
    pub(super) fn program(&self) -> Option<PathBuf> {
        self.program.clone().or_else(|| programs::find("tmux"))
    }

    /// Everything the server is keeping, with the bottom of every pane.
    ///
    /// Two commands however many panes there are: one lists them, the other
    /// reads them all. No server running is no sessions, not a failure.
    pub(super) async fn look(&self) -> Result<Layout, AttachError> {
        let listing = format!("C{FIELD}#{{client_tty}}{FIELD}#{{client_session}}");
        let panes = format!(
            "P{FIELD}#{{pane_id}}{FIELD}#{{pane_tty}}{FIELD}#{{session_name}}{FIELD}\
             #{{pane_height}}{FIELD}#{{pane_dead}}{FIELD}#{{host}}{FIELD}#{{pane_title}}"
        );
        let outcome = self
            .run(&[
                "list-clients",
                "-F",
                &listing,
                ";",
                "list-panes",
                "-a",
                "-F",
                &panes,
            ])
            .await?;
        if !outcome.succeeded() {
            return if no_server(&outcome) {
                Ok(Layout::default())
            } else {
                Err(failed("listing tmux sessions", &outcome))
            };
        }

        let (clients, mut panes) = parse_listing(&outcome.stdout_tail);
        if panes.is_empty() {
            return Ok(Layout {
                panes: Vec::new(),
                clients,
            });
        }

        let screens = self.screens(&panes).await?;
        for (pane, _) in &mut panes {
            if let Some(screen) = screens.iter().find(|(id, _)| *id == pane.id) {
                pane.screen.clone_from(&screen.1);
            }
        }
        Ok(Layout {
            panes: panes.into_iter().map(|(pane, _)| pane).collect(),
            clients,
        })
    }

    /// Reads the bottom of every pane in one command.
    async fn screens(&self, panes: &[(Pane, u32)]) -> Result<Vec<(String, String)>, AttachError> {
        let mut arguments: Vec<String> = Vec::new();
        for (pane, height) in panes {
            if !arguments.is_empty() {
                arguments.push(";".to_owned());
            }
            let start = height.saturating_sub(ROWS).to_string();
            arguments.extend(
                [
                    "display-message",
                    "-p",
                    "-t",
                    &pane.id,
                    &format!("{RECORD}#{{pane_id}}"),
                    ";",
                    "capture-pane",
                    "-p",
                    "-t",
                    &pane.id,
                    "-S",
                    &start,
                ]
                .map(str::to_owned),
            );
        }
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let outcome = self.run(&arguments).await?;
        if !outcome.succeeded() {
            // A pane that closed between the listing and the reading fails the
            // whole batch. Its neighbours are still worth showing, unread.
            return Ok(Vec::new());
        }
        Ok(parse_screens(&outcome.stdout_tail))
    }

    /// The last of what a pane has said, history included.
    pub(super) async fn read(&self, pane: &str, most: usize) -> Result<String, AttachError> {
        let start = format!("-{HISTORY}");
        let outcome = self
            .run(&["capture-pane", "-p", "-J", "-t", pane, "-S", &start])
            .await?;
        ensure(&outcome, pane)?;
        let text = outcome.stdout_tail;
        let from = text
            .char_indices()
            .rev()
            .nth(most.saturating_sub(1))
            .map_or(0, |(index, _)| index);
        Ok(text[from..].to_owned())
    }

    /// Types a line into a pane and sends it.
    ///
    /// Pasted rather than typed key by key, and pasted the way a terminal
    /// pastes, so an agent takes a line break in what was dictated as part of
    /// the message rather than as the end of it. A control character on its
    /// own, such as an interrupt, is a key and is pressed as one.
    pub(super) async fn send(&self, pane: &str, text: &str) -> Result<(), AttachError> {
        if !text.is_empty() && text.chars().all(char::is_control) {
            let outcome = self.run(&["send-keys", "-t", pane, "-l", text]).await?;
            return ensure(&outcome, pane);
        }

        let line = text.strip_suffix('\n').unwrap_or(text);
        let buffer = format!("pushos-{}", self.buffers.fetch_add(1, Ordering::Relaxed));
        let outcome = self
            .run(&[
                "set-buffer",
                "-b",
                &buffer,
                "--",
                line,
                ";",
                "paste-buffer",
                "-p",
                "-d",
                "-b",
                &buffer,
                "-t",
                pane,
            ])
            .await?;
        ensure(&outcome, pane)?;

        tokio::time::sleep(SETTLE).await;
        let outcome = self.run(&["send-keys", "-t", pane, "Enter"]).await?;
        ensure(&outcome, pane)
    }

    /// Presses a key in a pane. Nothing is brought to the front.
    pub(super) async fn press(&self, pane: &str, key: Key) -> Result<(), AttachError> {
        let outcome = self.run(&["send-keys", "-t", pane, key_name(key)]).await?;
        ensure(&outcome, pane)
    }

    /// Makes a pane the one its session shows.
    pub(super) async fn select(&self, pane: &str) -> Result<(), AttachError> {
        let outcome = self
            .run(&["select-window", "-t", pane, ";", "select-pane", "-t", pane])
            .await?;
        ensure(&outcome, pane)
    }

    /// Starts a session under a name, or finds the one already kept under it.
    pub(super) async fn open(&self, request: &OpenSession) -> Result<Opened, AttachError> {
        if !is_session_name(&request.name) {
            return Err(AttachError::unavailable(format!(
                "`{}` cannot name a session; use letters, digits, hyphens and underscores",
                request.name
            )));
        }
        let exactly = format!("={}:", request.name);

        let found = self
            .run(&["display-message", "-p", "-t", &exactly, "#{pane_tty}"])
            .await?;
        if found.succeeded() {
            return Ok(Opened {
                id: AttachedId::new(found.stdout_tail.trim()),
                started: false,
            });
        }

        let directory = request
            .directory
            .as_ref()
            .map(|directory| expand_home(directory))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let directory = directory.to_string_lossy();
        let (width, height) = DETACHED_SIZE;

        let started = self
            .run(&[
                "new-session",
                "-d",
                "-s",
                &request.name,
                "-c",
                &directory,
                "-x",
                width,
                "-y",
                height,
                "-P",
                "-F",
                "#{pane_tty}",
            ])
            .await?;
        if !started.succeeded() {
            return Err(failed(
                &format!("starting the tmux session `{}`", request.name),
                &started,
            ));
        }

        if let Some(run) = request
            .command
            .as_deref()
            .filter(|run| !run.trim().is_empty())
        {
            // Typed into the shell rather than run in its place, so the session
            // outlives the program. A shell reads what is typed once it is
            // ready, so there is nothing to wait for.
            let typed = self
                .run(&[
                    "send-keys",
                    "-t",
                    &exactly,
                    "-l",
                    run,
                    ";",
                    "send-keys",
                    "-t",
                    &exactly,
                    "Enter",
                ])
                .await?;
            if !typed.succeeded() {
                return Err(failed(
                    &format!("starting `{run}` in `{}`", request.name),
                    &typed,
                ));
            }
        }

        Ok(Opened {
            id: AttachedId::new(started.stdout_tail.trim()),
            started: true,
        })
    }

    /// The command a window runs to show a session.
    pub(super) fn attach_command(&self, session: &str) -> Option<Vec<String>> {
        let program = self.program()?;
        let mut command = vec![program.to_string_lossy().into_owned()];
        if let Some(socket) = &self.socket {
            command.extend(["-L".to_owned(), socket.clone()]);
        }
        command.extend([
            "attach-session".to_owned(),
            "-t".to_owned(),
            format!("={session}"),
        ]);
        Some(command)
    }

    /// Runs one tmux command line, made of several commands if need be.
    async fn run(&self, arguments: &[&str]) -> Result<ProcessOutcome, AttachError> {
        let program = self.program().ok_or_else(|| {
            AttachError::unavailable("tmux is not installed; install it with `brew install tmux`")
        })?;

        let mut args: Vec<String> = Vec::with_capacity(arguments.len() + 2);
        if let Some(socket) = &self.socket {
            args.extend(["-L".to_owned(), socket.clone()]);
        }
        args.extend(arguments.iter().map(|&argument| argument.to_owned()));

        let spec = ProcessSpec::new(program.to_string_lossy(), args)
            .within(PATIENCE)
            .capturing(1024 * 1024);
        self.processes.run(&spec).await.map_err(|error| {
            let class = error.class();
            AttachError::backend("asking tmux", class, error)
        })
    }
}

/// Turns a failed command about one pane into the pane having gone.
fn ensure(outcome: &ProcessOutcome, pane: &str) -> Result<(), AttachError> {
    if outcome.succeeded() {
        return Ok(());
    }
    if no_server(outcome) || outcome.stderr_tail.contains("can't find") {
        return Err(AttachError::Gone {
            session: AttachedId::new(pane),
        });
    }
    Err(failed("asking tmux", outcome))
}

/// Whether tmux failed only because no server is running.
fn no_server(outcome: &ProcessOutcome) -> bool {
    let said = &outcome.stderr_tail;
    said.contains("no server running") || said.contains("error connecting to")
}

fn failed(attempt: &str, outcome: &ProcessOutcome) -> AttachError {
    AttachError::backend(
        attempt.to_owned(),
        ErrorClass::ComponentFailure,
        std::io::Error::other(outcome.stderr_tail.trim().to_owned()),
    )
}

/// How tmux names a key.
const fn key_name(key: Key) -> &'static str {
    match key {
        Key::Tab => "Tab",
        Key::Enter => "Enter",
        Key::Escape => "Escape",
        Key::Up => "Up",
        Key::Down => "Down",
    }
}

/// Expands a leading `~` in a directory someone wrote.
fn expand_home(directory: &std::path::Path) -> PathBuf {
    let Ok(rest) = directory.strip_prefix("~") else {
        return directory.to_owned();
    };
    std::env::var_os("HOME").map_or_else(
        || directory.to_owned(),
        |home| PathBuf::from(home).join(rest),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use pushos_testkit::FakeProcesses;

    use super::*;

    /// A fake tmux that answers with whatever the test says and records every
    /// command line.
    fn fake(reply: impl Fn(&[String]) -> ProcessOutcome + Send + Sync + 'static) -> FakeProcesses {
        let processes = FakeProcesses::new();
        processes.reply_with(move |spec| reply(&spec.args));
        processes
    }

    fn tmux(processes: &FakeProcesses) -> Tmux {
        // The fake stands in for running it, so nothing need be installed.
        Tmux::at(Arc::new(processes.clone()), "/opt/homebrew/bin/tmux")
    }

    fn failing(stderr: &str) -> ProcessOutcome {
        ProcessOutcome {
            exit_code: Some(1),
            stdout_tail: String::new(),
            stderr_tail: stderr.to_owned(),
        }
    }

    #[tokio::test]
    async fn no_server_running_is_no_sessions() {
        let processes = fake(|_| failing("no server running on /private/tmp/tmux-501/default"));
        let tmux = tmux(&processes);
        assert_eq!(tmux.look().await.expect("not a failure"), Layout::default());
    }

    #[tokio::test]
    async fn text_is_pasted_as_one_argument_then_sent() {
        let processes = fake(|_| FakeProcesses::printed(""));
        let tmux = tmux(&processes);
        tmux.send("%3", "fix the tests; rm -rf ~\nthen commit\n")
            .await
            .expect("sent");

        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 2, "paste, then Enter");
        let paste = &spawned[0].args;
        let text = paste
            .iter()
            .position(|argument| argument == "--")
            .map(|at| paste[at + 1].clone());
        assert_eq!(
            text.as_deref(),
            Some("fix the tests; rm -rf ~\nthen commit"),
            "the whole text is one argument, with its final line break left to Enter"
        );
        assert!(
            paste.contains(&"-p".to_owned()),
            "pasted as a terminal pastes"
        );
        assert_eq!(spawned[1].args, ["send-keys", "-t", "%3", "Enter"]);
    }

    #[tokio::test]
    async fn an_interrupt_is_a_key_not_a_line() {
        let processes = fake(|_| FakeProcesses::printed(""));
        let tmux = tmux(&processes);
        tmux.send("%3", "\u{3}").await.expect("sent");

        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 1, "and no Enter after it");
        assert_eq!(spawned[0].args, ["send-keys", "-t", "%3", "-l", "\u{3}"]);
    }

    #[tokio::test]
    async fn a_session_already_running_is_found_rather_than_started_again() {
        let processes = fake(|args| {
            if args[0] == "display-message" {
                FakeProcesses::printed("/dev/ttys009\n")
            } else {
                FakeProcesses::printed("")
            }
        });
        let tmux = tmux(&processes);
        let opened = tmux
            .open(&OpenSession {
                name: "client-1".to_owned(),
                keeper: pushos_domain::ports::Keeper::Terminal,
                directory: None,
                command: Some("claude".to_owned()),
            })
            .await
            .expect("found");

        assert!(!opened.started);
        assert_eq!(opened.id.as_str(), "/dev/ttys009");
        assert_eq!(
            processes.spawned().len(),
            1,
            "nothing started, nothing typed"
        );
    }

    #[tokio::test]
    async fn a_new_session_starts_in_its_folder_and_types_its_command() {
        let asked = Arc::new(Mutex::new(0_u8));
        let counting = Arc::clone(&asked);
        let processes = fake(move |args| {
            let mut count = counting.lock().expect("unpoisoned");
            *count += 1;
            match args[0].as_str() {
                "display-message" => failing("can't find session: client-1"),
                "new-session" => FakeProcesses::printed("/dev/ttys010\n"),
                _ => FakeProcesses::printed(""),
            }
        });
        let tmux = tmux(&processes);
        let opened = tmux
            .open(&OpenSession {
                name: "client-1".to_owned(),
                keeper: pushos_domain::ports::Keeper::Terminal,
                directory: Some(PathBuf::from("/work/client")),
                command: Some("claude".to_owned()),
            })
            .await
            .expect("started");

        assert!(opened.started);
        assert_eq!(opened.id.as_str(), "/dev/ttys010");
        let spawned = processes.spawned();
        let new = &spawned[1].args;
        assert!(new.windows(2).any(|pair| pair == ["-s", "client-1"]));
        assert!(new.windows(2).any(|pair| pair == ["-c", "/work/client"]));
        assert_eq!(
            spawned[2].args,
            [
                "send-keys",
                "-t",
                "=client-1:",
                "-l",
                "claude",
                ";",
                "send-keys",
                "-t",
                "=client-1:",
                "Enter"
            ]
        );
    }

    #[tokio::test]
    async fn a_name_tmux_would_read_as_an_address_is_refused_before_asking() {
        let processes = FakeProcesses::new();
        let tmux = Tmux::new(Arc::new(processes.clone()));
        for bad in ["client:1", "a.b", "", "two words"] {
            let refused = tmux
                .open(&OpenSession {
                    name: bad.to_owned(),
                    keeper: pushos_domain::ports::Keeper::Terminal,
                    directory: None,
                    command: None,
                })
                .await;
            assert!(refused.is_err(), "`{bad}`");
        }
        assert!(processes.spawned().is_empty());
    }

    #[test]
    fn a_home_directory_written_with_a_tilde_is_expanded() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        assert_eq!(
            expand_home(std::path::Path::new("~/work")),
            PathBuf::from(home).join("work")
        );
        assert_eq!(
            expand_home(std::path::Path::new("/abs")),
            PathBuf::from("/abs")
        );
    }

    /// Against a real tmux, on a private server so nothing of the operator's
    /// is touched. Skipped where tmux is not installed.
    #[tokio::test]
    async fn a_real_session_can_be_opened_found_typed_into_and_read() {
        let socket = format!("pushos-test-{}", std::process::id());
        let tmux = Tmux::on_socket(Arc::new(crate::SystemProcessRunner::new()), &socket);
        if tmux.program().is_none() {
            eprintln!("tmux is not installed; skipping");
            return;
        }
        let request = OpenSession {
            name: "worker".to_owned(),
            keeper: pushos_domain::ports::Keeper::Terminal,
            directory: Some(std::env::temp_dir()),
            command: Some("printf 'ready-%s\\n' pushos".to_owned()),
        };

        let opened = tmux.open(&request).await.expect("tmux starts a session");
        assert!(opened.started && opened.id.as_str().starts_with("/dev/"));
        let again = tmux.open(&request).await.expect("and finds it");
        assert!(!again.started);
        assert_eq!(again.id, opened.id);

        let layout = tmux.look().await.expect("tmux lists it");
        let pane = layout
            .panes
            .iter()
            .find(|pane| pane.session == "worker")
            .expect("the session is listed")
            .clone();
        assert_eq!(pane.device, opened.id.as_str());

        tmux.send(&pane.id, "echo typed-by-pushos")
            .await
            .expect("typed");
        let mut said = String::new();
        for _ in 0..40 {
            said = tmux.read(&pane.id, 2_000).await.expect("readable");
            if said.contains("ready-pushos") && said.matches("typed-by-pushos").count() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(said.contains("ready-pushos"), "the command ran: {said}");
        assert!(
            said.matches("typed-by-pushos").count() >= 2,
            "what was typed ran and printed: {said}"
        );

        // tmux leaves its socket file behind when its server is killed.
        let socket_path = tmux
            .run(&["display-message", "-p", "#{socket_path}"])
            .await
            .map(|outcome| outcome.stdout_tail.trim().to_owned())
            .unwrap_or_default();
        tmux.run(&["kill-server"]).await.ok();
        if !socket_path.is_empty() {
            std::fs::remove_file(socket_path).ok();
        }
    }
}
