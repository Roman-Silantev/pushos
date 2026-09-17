//! Every coding session on this Mac that PushOS did not start.
//!
//! Three sources, each good at something the others cannot do:
//!
//! - **tmux** keeps sessions running with no window, by name. PushOS can start
//!   one there for a pad, type into it and read it without bringing anything
//!   to the front, and the operator opens it in any terminal.
//! - **Terminal** is where sessions already open are. PushOS can read and type
//!   into its tabs through AppleScript.
//! - **Claude Code** says what each of its sessions is doing, wherever it
//!   runs, including in an editor, where PushOS can see it and not reach it.
//!
//! One look asks all three at once, and [`merge`] turns what they saw into one
//! list, each session once. Which source reaches which session is remembered
//! from that look, so typing into a session goes the right way without asking
//! again.

mod claude;
mod codex;
mod hosts;
mod merge;
mod programs;
mod seats;
mod supervised;
mod terminal_app;
mod tmux;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use pushos_domain::attached::Attached;
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{
    AttachError, AttachedSessions, Keeper, Key, OpenSession, Opened, ProcessRunner, ProcessSpec,
};
use tokio::sync::Mutex;
use tracing::warn;

use self::claude::ClaudeCode;
use self::codex::CodexThreads;
use self::hosts::{Hosts, Whereabouts};
use self::merge::{Route, Seen, merge};
use self::seats::Seats;
use self::supervised::Supervisor;
use self::terminal_app::TerminalApp;
use self::tmux::Tmux;

pub use self::hosts::terminal_above;

/// What the identifier of a session a coding agent keeps begins with.
///
/// Told apart from a terminal device, which is what every other session is
/// named by, so one kind is never taken for the other.
const KEPT_PREFIX: &str = "kept:";

/// What a session Claude Code keeps runs in, as an operator would look for it.
const CLAUDE_CODE: &str = "Claude Code";

/// What the identifier of a Codex thread begins with.
const THREAD_PREFIX: &str = "thread:";

/// What a Codex thread runs in, as an operator would look for it.
const CODEX: &str = "Codex";

/// Every coding session on this Mac that PushOS did not start.
#[derive(Debug)]
pub struct MacSessions {
    processes: Arc<dyn ProcessRunner>,
    terminal: TerminalApp,
    tmux: Tmux,
    claude: ClaudeCode,
    /// Starts, instructs and puts away the sessions Claude Code keeps.
    supervisor: Supervisor,
    /// The threads Codex keeps, all of them in one server.
    codex: CodexThreads,
    /// Which of those sessions each named seat is holding.
    seats: Seats,
    hosts: Hosts,
    /// How to reach each session, as of the last look.
    routes: Mutex<HashMap<AttachedId, Route>>,
    /// Where each Claude Code process is. A process does not move, so it is
    /// looked up once rather than on every look.
    whereabouts: Mutex<HashMap<u32, Whereabouts>>,
    /// Whether Terminal refusing has been reported, so it is said once.
    refused: AtomicBool,
}

/// How one source is doing, for `pushos doctor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceReport {
    /// Which source.
    pub source: &'static str,
    /// Whether it is usable.
    pub usable: bool,
    /// What it found, or what is wrong and how to fix it.
    pub detail: String,
}

impl MacSessions {
    /// Watches every source on this Mac.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            terminal: TerminalApp::new(Arc::clone(&processes)),
            tmux: Tmux::new(Arc::clone(&processes)),
            claude: ClaudeCode::new(Arc::clone(&processes)),
            supervisor: Supervisor::new(Arc::clone(&processes)),
            codex: CodexThreads::new(true),
            seats: Seats::kept_in(None),
            hosts: Hosts::new(Arc::clone(&processes)),
            processes,
            routes: Mutex::new(HashMap::new()),
            whereabouts: Mutex::new(HashMap::new()),
            refused: AtomicBool::new(false),
        }
    }

    /// The same, with everything Codex can do rather than only what a pad
    /// uses.
    ///
    /// Costs memory in the Codex server, and a program of its own per thread
    /// for some of it; for an operator who wants browsing or apps in the
    /// threads on their pads.
    #[must_use]
    pub fn with_every_codex_feature(mut self) -> Self {
        self.codex = CodexThreads::new(false);
        self
    }

    /// The same, speaking to a particular Claude Code rather than the one
    /// installed, so a test does not depend on the machine it runs on.
    #[cfg(test)]
    fn with_claude_at(mut self, program: &str, registry: &std::path::Path) -> Self {
        self.claude = ClaudeCode::at(Arc::clone(&self.processes), program, registry);
        self.supervisor = Supervisor::at(Arc::clone(&self.processes), program);
        self
    }

    /// The same, writing down what each seat holds in the given file.
    ///
    /// Without one, a pad that started a session finds it again only for as
    /// long as PushOS keeps running.
    #[must_use]
    pub fn keeping_seats_in(mut self, book: std::path::PathBuf) -> Self {
        self.seats = Seats::kept_in(Some(book));
        self
    }

    /// Asks each source what it can see, and says how each is doing.
    pub async fn report(&self) -> Vec<SourceReport> {
        let terminal = match self.terminal.tabs().await {
            Ok(tabs) => SourceReport {
                source: terminal_app::APPLICATION,
                usable: true,
                detail: format!("{} tab(s) open", tabs.len()),
            },
            Err(error) => SourceReport {
                source: terminal_app::APPLICATION,
                usable: false,
                detail: error.to_string(),
            },
        };

        let tmux = match (self.tmux.program(), self.tmux.look().await) {
            (None, _) => SourceReport {
                source: merge::TMUX,
                usable: false,
                detail: "not installed; `brew install tmux` to keep a session for each pad"
                    .to_owned(),
            },
            (Some(program), Ok(layout)) => SourceReport {
                source: merge::TMUX,
                usable: true,
                detail: format!(
                    "{} session(s), using {}",
                    layout.panes.len(),
                    program.display()
                ),
            },
            (Some(_), Err(error)) => SourceReport {
                source: merge::TMUX,
                usable: false,
                detail: error.to_string(),
            },
        };

        let claude = match self.claude.program() {
            None => SourceReport {
                source: "Claude Code",
                usable: false,
                detail: "not installed; its sessions are read from their screens instead"
                    .to_owned(),
            },
            Some(program) => SourceReport {
                source: "Claude Code",
                usable: true,
                detail: format!(
                    "{} session(s) running, using {}",
                    self.claude.sessions().await.len(),
                    program.display()
                ),
            },
        };

        let codex = match self.codex.program() {
            None => SourceReport {
                source: CODEX,
                usable: false,
                detail: "not installed; its threads cannot be put on pads".to_owned(),
            },
            Some(program) => {
                let listing = self.codex.threads().await;
                SourceReport {
                    source: CODEX,
                    usable: listing.answered,
                    detail: if listing.answered {
                        format!(
                            "{} thread(s), {} of them loaded, using {}",
                            listing.threads.len(),
                            listing
                                .threads
                                .iter()
                                .filter(|thread| thread.loaded)
                                .count(),
                            program.display()
                        )
                    } else {
                        format!("{} would not answer", program.display())
                    },
                }
            }
        };

        vec![terminal, tmux, claude, codex]
    }

    /// How to reach a session, looking again if it was not in the last look.
    async fn route(&self, session: &AttachedId) -> Result<Route, AttachError> {
        if let Some(route) = self.routes.lock().await.get(session) {
            return Ok(route.clone());
        }
        self.discover().await?;
        self.routes
            .lock()
            .await
            .get(session)
            .cloned()
            .ok_or_else(|| AttachError::Gone {
                session: session.clone(),
            })
    }

    /// Where each of these Claude Code processes is, looking up only new ones.
    async fn locate(&self, pids: &[u32]) -> HashMap<u32, Whereabouts> {
        let mut known = self.whereabouts.lock().await;
        known.retain(|pid, _| pids.contains(pid));
        let unknown: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| !known.contains_key(pid))
            .collect();
        if !unknown.is_empty() {
            known.extend(self.hosts.locate(&unknown).await);
        }
        known.clone()
    }

    /// Brings an application to the front, for a session PushOS cannot reach.
    async fn activate(&self, application: &str) -> Result<(), AttachError> {
        let spec = ProcessSpec::new("/usr/bin/open", ["-a".to_owned(), application.to_owned()]);
        self.processes.run(&spec).await.map(drop).map_err(|error| {
            let class = error.class();
            AttachError::backend(format!("bringing {application} to the front"), class, error)
        })
    }

    /// The sessions Claude Code keeps, as sessions on the surface.
    ///
    /// One that is running is shown whoever started it. One that was put away
    /// is shown only while a seat holds it: it is a pad's session, waiting to
    /// be asked for again, rather than a finished piece of work nobody meant
    /// to keep on the surface.
    async fn on_seats(&self, kept: Vec<supervised::Supervised>) -> Vec<(Attached, Route)> {
        let mut on_surface = Vec::new();
        for session in kept {
            let seat = self.seats.seat_of(&session.id).await;
            if !session.running && seat.is_none() {
                continue;
            }
            let mut attached = Attached::new(
                format!("{KEPT_PREFIX}{}", session.id),
                session.name.clone(),
                session.activity,
                CLAUDE_CODE,
            )
            .dispatched(Keeper::ClaudeCode);
            if let Some(seat) = seat {
                attached = attached.named(seat);
            }
            on_surface.push((
                attached,
                Route::Kept {
                    id: session.id,
                    conversation: session.session,
                    directory: session.directory,
                },
            ));
        }
        on_surface
    }

    /// The Codex threads seats hold, as sessions on the surface.
    ///
    /// Only the ones a seat holds. Codex remembers every thread the operator
    /// has ever had, and a surface showing all of them would be a list of
    /// their history rather than of their pads.
    async fn threads_on_seats(&self, threads: Vec<codex::Thread>) -> Vec<(Attached, Route)> {
        let mut on_surface = Vec::new();
        for thread in threads {
            let Some(seat) = self.seats.seat_of(&thread.id).await else {
                continue;
            };
            on_surface.push((
                Attached::new(
                    format!("{THREAD_PREFIX}{}", thread.id),
                    thread.name,
                    thread.activity,
                    CODEX,
                )
                .dispatched(Keeper::CodexThreads)
                .named(seat),
                Route::Thread { id: thread.id },
            ));
        }
        on_surface
    }

    /// Finds the session a seat holds, or starts one for it.
    ///
    /// Finding rather than starting another, so one pad both starts a worker
    /// and comes back to it however many times it is pressed. One that was put
    /// away is found too: it is started again by being spoken to or opened.
    async fn take_a_seat(&self, request: &OpenSession) -> Result<Opened, AttachError> {
        let codex = request.keeper == Keeper::CodexThreads;
        let prefix = if codex { THREAD_PREFIX } else { KEPT_PREFIX };

        if let Some(held) = self.seats.holding(&request.name).await
            && self.still_there(&held, codex).await
        {
            return Ok(Opened {
                id: AttachedId::new(format!("{prefix}{held}")),
                started: false,
            });
        }

        let work = request.command.as_deref().unwrap_or_default().trim();
        if work.is_empty() {
            return Err(AttachError::unavailable(format!(
                "`{}` has nothing to work on; give the pad `work` to start it with",
                request.name
            )));
        }
        let directory = match &request.directory {
            Some(directory) => directory.clone(),
            None => std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default(),
        };

        let id = if codex {
            self.codex.start(&directory, &request.name, work).await?
        } else {
            let id = self.supervisor.start(&directory, work).await?;
            // The answer Claude Code gave a moment ago is about a world
            // without this session in it, and the next look decides what
            // still exists.
            self.claude.look_again().await;
            id
        };
        self.seats.took(&request.name, &id).await;
        Ok(Opened {
            id: AttachedId::new(format!("{prefix}{id}")),
            started: true,
        })
    }

    /// Whether what a seat holds is still something its keeper knows about.
    ///
    /// A keeper that would not answer leaves the seat alone: starting another
    /// session because the first could not be confirmed would leave two agents
    /// working in one folder, and the older one unreachable.
    async fn still_there(&self, held: &str, codex: bool) -> bool {
        if codex {
            let listing = self.codex.threads().await;
            !listing.answered || listing.threads.iter().any(|thread| thread.id == held)
        } else {
            let look = self.claude.kept().await;
            !look.answered || look.kept.iter().any(|session| session.id == held)
        }
    }

    fn unreachable(session: &AttachedId, application: Option<&String>) -> AttachError {
        AttachError::Unreachable {
            session: session.clone(),
            host: application.map_or_else(|| "Claude Code".to_owned(), Clone::clone),
        }
    }
}

#[async_trait]
impl AttachedSessions for MacSessions {
    async fn discover(&self) -> Result<Vec<Attached>, AttachError> {
        let (tabs, layout, claude, kept, threads) = tokio::join!(
            self.terminal.tabs(),
            self.tmux.look(),
            self.claude.sessions(),
            self.claude.kept(),
            self.codex.threads()
        );
        let answered = kept.answered && threads.answered;

        // Each source on its own terms: Terminal refusing permission, or tmux
        // not being installed, hides nothing the others can see.
        let (tabs, refused) = match tabs {
            Ok(tabs) => {
                self.refused.store(false, Ordering::Relaxed);
                (tabs, None)
            }
            Err(error) => {
                if !self.refused.swap(true, Ordering::Relaxed) {
                    warn!(%error, "cannot see the tabs open in Terminal");
                }
                (Vec::new(), Some(error))
            }
        };
        let layout = layout.unwrap_or_default();

        let pids: Vec<u32> = claude.iter().map(|session| session.pid).collect();
        let whereabouts = self.locate(&pids).await;

        let mut merged = merge(&Seen {
            tabs,
            panes: layout.panes,
            clients: layout.clients,
            claude,
            whereabouts,
        });
        // One book holds both kinds, so what still exists is worked out over
        // both before anything is dropped from it — and only when both
        // actually answered. A keeper that said nothing knows nothing, and
        // pruning on that would lose which session a pad holds for good.
        if answered {
            let existing: Vec<String> = kept
                .kept
                .iter()
                .map(|session| session.id.clone())
                .chain(threads.threads.iter().map(|thread| thread.id.clone()))
                .collect();
            self.seats.keep_only(&existing).await;
        }
        merged.extend(self.on_seats(kept.kept).await);
        merged.extend(self.threads_on_seats(threads.threads).await);

        // Terminal refusing is still worth an error when it leaves nothing to
        // show, because that is the one an operator can fix.
        if merged.is_empty()
            && let Some(error) = refused
        {
            return Err(error);
        }

        let mut routes = self.routes.lock().await;
        routes.clear();
        let mut sessions = Vec::with_capacity(merged.len());
        for (session, route) in merged {
            routes.insert(session.id.clone(), route);
            sessions.push(session);
        }
        Ok(sessions)
    }

    async fn read(&self, session: &AttachedId, most: usize) -> Result<String, AttachError> {
        match self.route(session).await? {
            Route::Tab(device) => self.terminal.read(&device, most).await,
            Route::Pane { pane, .. } => self.tmux.read(&pane, most).await,
            Route::Kept { id, .. } => self.supervisor.read(&id, most).await,
            Route::Thread { id } => self.codex.read(&id, most).await,
            Route::Nowhere { application } => Err(Self::unreachable(session, application.as_ref())),
        }
    }

    async fn send(&self, session: &AttachedId, text: &str) -> Result<(), AttachError> {
        match self.route(session).await? {
            Route::Tab(device) => self.terminal.send(&device, text).await,
            Route::Pane { pane, .. } => self.tmux.send(&pane, text).await,
            // An instruction is what starts one that was put away, so this is
            // both how it is spoken to and how it is woken.
            Route::Kept {
                conversation,
                directory,
                ..
            } => {
                self.supervisor
                    .instruct(&conversation, &directory, text)
                    .await
            }
            Route::Thread { id } => self.codex.instruct(&id, text).await,
            Route::Nowhere { application } => Err(Self::unreachable(session, application.as_ref())),
        }
    }

    async fn press(&self, session: &AttachedId, key: Key) -> Result<(), AttachError> {
        match self.route(session).await? {
            Route::Tab(device) => self.terminal.press(&device, key).await,
            Route::Pane { pane, .. } => self.tmux.press(&pane, key).await,
            // There is no screen to press a key at: it takes instructions.
            Route::Kept { .. } | Route::Thread { .. } => Err(AttachError::unavailable(
                "this session has no screen to press a key in; send it an instruction instead",
            )),
            Route::Nowhere { application } => Err(Self::unreachable(session, application.as_ref())),
        }
    }

    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError> {
        match self.route(session).await? {
            Route::Tab(device) => self.terminal.focus(&device).await,
            Route::Pane {
                pane,
                session: name,
            } => {
                self.tmux.select(&pane).await?;

                // A window already showing it is brought forward. One in an
                // application PushOS cannot script is left where it is; the
                // pane is selected in it, which is as far as PushOS can go.
                let layout = self.tmux.look().await.unwrap_or_default();
                let mut showing = layout
                    .clients
                    .iter()
                    .filter(|client| client.session == name);
                if let Some(client) = showing.next() {
                    return match self.terminal.focus(&client.device).await {
                        Err(AttachError::Gone { .. }) => Ok(()),
                        other => other,
                    };
                }

                let command = self.tmux.attach_command(&name).ok_or_else(|| {
                    AttachError::unavailable(
                        "tmux is not installed; install it with `brew install tmux`",
                    )
                })?;
                self.terminal.open_window(&command).await
            }
            // Opening a window onto one that was put away starts it again,
            // which is what the operator asked for by opening it.
            Route::Kept { id, .. } => {
                let command = self.supervisor.attach_command(&id).ok_or_else(|| {
                    AttachError::unavailable(
                        "Claude Code is not installed, so it keeps no sessions",
                    )
                })?;
                self.terminal.open_window(&command).await
            }
            Route::Thread { id } => {
                let command = self.codex.attach_command(&id).ok_or_else(|| {
                    AttachError::unavailable("Codex is not installed, so it keeps no threads")
                })?;
                self.terminal.open_window(&command).await
            }
            // Nothing to type into, but the operator can still be taken to it.
            Route::Nowhere {
                application: Some(application),
            } => self.activate(&application).await,
            Route::Nowhere { application: None } => Err(Self::unreachable(session, None)),
        }
    }

    async fn open(&self, request: &OpenSession) -> Result<Opened, AttachError> {
        let opened = match request.keeper {
            Keeper::Terminal => self.tmux.open(request).await?,
            Keeper::ClaudeCode | Keeper::CodexThreads => self.take_a_seat(request).await?,
        };
        // Looked at again at once, so the session just started can be reached
        // by the action that asked for it without waiting for the next look.
        self.discover().await?;
        Ok(opened)
    }

    async fn put_away(&self, session: &AttachedId) -> Result<bool, AttachError> {
        match self.route(session).await? {
            Route::Kept { id, .. } => {
                self.supervisor.put_away(&id).await?;
                Ok(true)
            }
            Route::Thread { id } => {
                self.codex.put_away(&id).await?;
                Ok(true)
            }
            // A session in a terminal is a program somebody is looking at, and
            // stopping it would be closing their window.
            _ => Ok(false),
        }
    }

    fn describe(&self) -> &'static str {
        "tmux, Terminal and Claude Code"
    }
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    /// A scratch directory for a seat book, removed when the test ends.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pushos-seatbook-{label}-{}",
                pushos_domain::ids::ExecutionId::generate()
            ));
            std::fs::create_dir_all(&path).expect("writable");
            Self(path)
        }

        fn book(&self) -> std::path::PathBuf {
            self.0.join("seats.json")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A Claude Code whose listing learns about a session once it is started.
    ///
    /// Exactly what the real one does, and the reason the answer from a
    /// moment ago cannot be trusted after PushOS starts something.
    fn claude_that_learns(processes: &FakeProcesses) -> Arc<std::sync::atomic::AtomicBool> {
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let knows = Arc::clone(&started);
        processes.reply_with(move |spec| {
            let starting = spec.args.iter().any(|argument| argument == "--bg");
            let listing = spec.args.iter().any(|argument| argument == "agents");
            if starting {
                knows.store(true, std::sync::atomic::Ordering::Relaxed);
                return FakeProcesses::printed("claude attach 55c88714   open in this terminal");
            }
            if listing {
                let listed = knows.load(std::sync::atomic::Ordering::Relaxed);
                return FakeProcesses::printed(if listed {
                    r#"[{"id":"55c88714","sessionId":"55c88714-aa","cwd":"/tmp","kind":"background","state":"done","pid":4242}]"#
                } else {
                    "[]"
                });
            }
            FakeProcesses::printed("")
        });
        started
    }

    fn a_builder(name: &str) -> OpenSession {
        OpenSession {
            name: name.to_owned(),
            directory: Some(std::path::PathBuf::from("/tmp")),
            keeper: Keeper::ClaudeCode,
            command: Some("write the tests".to_owned()),
        }
    }

    #[tokio::test]
    async fn a_seat_still_holds_the_session_it_just_started() {
        // Starting one looks again at once, and the answer from a moment
        // before it existed would say it is not there. Taking that for "it is
        // gone" drops the seat, leaving a session running that no pad reaches.
        let scratch = Scratch::new("kept");
        let processes = FakeProcesses::new();
        claude_that_learns(&processes);
        let sessions = MacSessions::new(Arc::new(processes))
            .with_claude_at("/bin/claude", &scratch.0)
            .keeping_seats_in(scratch.book());
        // As the poll loop does, a moment before the pad is pressed: this is
        // the answer that will be stale by the time the session exists.
        sessions.discover().await.ok();

        let opened = sessions.open(&a_builder("builder")).await.expect("started");

        assert_eq!(opened.id.as_str(), "kept:55c88714");
        assert_eq!(
            sessions.seats.holding("builder").await.as_deref(),
            Some("55c88714"),
            "the seat still holds what it just started"
        );
    }

    #[tokio::test]
    async fn a_keeper_that_will_not_answer_never_empties_the_seat_book() {
        // PushOS starting while Claude Code is mid-update, or before the
        // keychain is ready: it has never answered, so it knows nothing.
        // Reading that as "every session is gone" would lose which session
        // each pad holds, for good.
        let scratch = Scratch::new("silent");
        std::fs::write(
            scratch.book(),
            r#"{"builder": "55c88714", "reviewer": "9db46d48"}"#,
        )
        .expect("writable");

        let processes = FakeProcesses::new();
        processes.reply_with(|spec| {
            if spec.args.iter().any(|argument| argument == "agents") {
                return pushos_domain::ports::ProcessOutcome {
                    exit_code: Some(1),
                    stdout_tail: String::new(),
                    stderr_tail: "could not read the session list".to_owned(),
                };
            }
            FakeProcesses::printed("")
        });
        let sessions = MacSessions::new(Arc::new(processes))
            .with_claude_at("/bin/claude", &scratch.0)
            .keeping_seats_in(scratch.book());

        sessions.discover().await.ok();

        assert_eq!(
            sessions.seats.holding("builder").await.as_deref(),
            Some("55c88714"),
            "silence is not proof that a session went"
        );
        assert_eq!(
            sessions.seats.holding("reviewer").await.as_deref(),
            Some("9db46d48")
        );
    }

    #[tokio::test]
    async fn a_session_that_was_never_seen_is_gone() {
        let processes = FakeProcesses::new();
        let sessions = MacSessions::new(Arc::new(processes));
        let error = sessions
            .send(&AttachedId::new("/dev/ttys999"), "hello")
            .await
            .expect_err("nothing is open");
        assert!(matches!(error, AttachError::Gone { .. }));
    }

    #[tokio::test]
    async fn a_session_pushos_can_only_watch_refuses_typing_and_says_where_it_is() {
        let processes = FakeProcesses::new();
        let sessions = MacSessions::new(Arc::new(processes));
        let id = AttachedId::new("claude:0d2c");
        sessions.routes.lock().await.insert(
            id.clone(),
            Route::Nowhere {
                application: Some("Visual Studio Code".to_owned()),
            },
        );

        let error = sessions.send(&id, "hello").await.expect_err("unreachable");
        assert!(error.to_string().contains("Visual Studio Code"), "{error}");
        assert!(matches!(error, AttachError::Unreachable { .. }));
    }

    #[tokio::test]
    async fn focusing_a_session_pushos_can_only_watch_brings_its_application_forward() {
        let processes = FakeProcesses::new();
        let sessions = MacSessions::new(Arc::new(processes.clone()));
        let id = AttachedId::new("claude:0d2c");
        sessions.routes.lock().await.insert(
            id.clone(),
            Route::Nowhere {
                application: Some("Visual Studio Code".to_owned()),
            },
        );

        sessions.focus(&id).await.expect("activated");
        let spawned = processes.spawned();
        assert_eq!(spawned[0].program, "/usr/bin/open");
        assert_eq!(spawned[0].args, ["-a", "Visual Studio Code"]);
    }
}
