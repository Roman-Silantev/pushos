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

/// Every coding session on this Mac that PushOS did not start.
#[derive(Debug)]
pub struct MacSessions {
    processes: Arc<dyn ProcessRunner>,
    terminal: TerminalApp,
    tmux: Tmux,
    claude: ClaudeCode,
    /// Starts, instructs and puts away the sessions Claude Code keeps.
    supervisor: Supervisor,
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
            seats: Seats::kept_in(None),
            hosts: Hosts::new(Arc::clone(&processes)),
            processes,
            routes: Mutex::new(HashMap::new()),
            whereabouts: Mutex::new(HashMap::new()),
            refused: AtomicBool::new(false),
        }
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

        vec![terminal, tmux, claude]
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
        let existing: Vec<String> = kept.iter().map(|session| session.id.clone()).collect();
        self.seats.keep_only(&existing).await;

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
            .dispatched();
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

    /// Finds the session a seat holds, or starts one for it.
    ///
    /// Finding rather than starting another, so one pad both starts a worker
    /// and comes back to it however many times it is pressed. One that was put
    /// away is found too: it is started again by being spoken to or opened.
    async fn take_a_seat(&self, request: &OpenSession) -> Result<Opened, AttachError> {
        if let Some(held) = self.seats.holding(&request.name).await
            && self
                .claude
                .kept()
                .await
                .iter()
                .any(|session| session.id == held)
        {
            return Ok(Opened {
                id: AttachedId::new(format!("{KEPT_PREFIX}{held}")),
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

        let id = self.supervisor.start(&directory, work).await?;
        self.seats.took(&request.name, &id).await;
        Ok(Opened {
            id: AttachedId::new(format!("{KEPT_PREFIX}{id}")),
            started: true,
        })
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
        let (tabs, layout, claude, kept) = tokio::join!(
            self.terminal.tabs(),
            self.tmux.look(),
            self.claude.sessions(),
            self.claude.kept()
        );

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
        merged.extend(self.on_seats(kept).await);

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
            Route::Nowhere { application } => Err(Self::unreachable(session, application.as_ref())),
        }
    }

    async fn press(&self, session: &AttachedId, key: Key) -> Result<(), AttachError> {
        match self.route(session).await? {
            Route::Tab(device) => self.terminal.press(&device, key).await,
            Route::Pane { pane, .. } => self.tmux.press(&pane, key).await,
            // There is no screen to press a key at: it takes instructions.
            Route::Kept { .. } => Err(AttachError::unavailable(
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
            Keeper::Agent => self.take_a_seat(request).await?,
        };
        // Looked at again at once, so the session just started can be reached
        // by the action that asked for it without waiting for the next look.
        self.discover().await?;
        Ok(opened)
    }

    async fn put_away(&self, session: &AttachedId) -> Result<bool, AttachError> {
        let Route::Kept { id, .. } = self.route(session).await? else {
            // A session in a terminal is a program somebody is looking at, and
            // stopping it would be closing their window.
            return Ok(false);
        };
        self.supervisor.put_away(&id).await?;
        Ok(true)
    }

    fn describe(&self) -> &'static str {
        "tmux, Terminal and Claude Code"
    }
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

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
