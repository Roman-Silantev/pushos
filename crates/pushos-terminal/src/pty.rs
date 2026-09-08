//! Terminals backed by a real pseudo-terminal.
//!
//! This is the only module in PushOS that knows what a pty is. Everything above
//! it works through [`TerminalHost`].
//!
//! A pty is blocking by nature: reads wait for the program to say something,
//! which may be hours. Each terminal therefore gets one dedicated thread, not a
//! task on the async runtime, and that thread is also what waits for the
//! program to finish. Reading and waiting in the same place is what guarantees
//! everything the program wrote is reported before its exit is.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use async_trait::async_trait;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use pushos_domain::error::ErrorClass;
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{
    TerminalError, TerminalEvent, TerminalHandle, TerminalHost, TerminalObserver, TerminalSize,
    TerminalSpec,
};
use tracing::{debug, warn};

use crate::text::OutputFilter;

/// How much is read at once.
///
/// A build prints in bursts; a shell prints a character at a time. This is
/// large enough that a burst is a few reads and small enough to sit on a
/// thread's stack.
const READ_CHUNK: usize = 8 * 1024;

/// Runs terminals as pseudo-terminals on this machine.
pub struct PtyTerminals {
    inner: Arc<Inner>,
}

struct Inner {
    observer: Arc<dyn TerminalObserver>,
    open: Mutex<HashMap<SessionId, Open>>,
}

/// One terminal, from the side that talks to it.
struct Open {
    /// Typing goes here. Shared with the thread that performs the write, since
    /// a program that has stopped reading can make a write wait.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Ends the program. Held separately from the child so that ending it does
    /// not need the thread that is blocked waiting for it, and taken once so a
    /// second close cannot signal a process identifier that may have been
    /// reused.
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    master: Box<dyn MasterPty + Send>,
}

impl std::fmt::Debug for PtyTerminals {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyTerminals")
            .field("open", &lock(&self.inner.open).len())
            .finish()
    }
}

impl PtyTerminals {
    /// Builds a host that reports everything to `observer`.
    pub fn new(observer: Arc<dyn TerminalObserver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                observer,
                open: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// How many terminals are running.
    pub fn open_count(&self) -> usize {
        lock(&self.inner.open).len()
    }
}

#[async_trait]
impl TerminalHost for PtyTerminals {
    async fn open(
        &self,
        id: SessionId,
        spec: TerminalSpec,
    ) -> Result<TerminalHandle, TerminalError> {
        validate(&spec)?;

        if lock(&self.inner.open).contains_key(&id) {
            return Err(TerminalError::host(
                format!("terminal `{id}` is already open"),
                ErrorClass::Validation,
                std::io::Error::other("duplicate terminal"),
            ));
        }

        // Opening a pty and spawning into it are both blocking calls that talk
        // to the kernel, so they do not belong on the async runtime's threads.
        let started = tokio::task::spawn_blocking(move || start(&spec))
            .await
            .map_err(|error| {
                TerminalError::host(
                    "the terminal could not be started",
                    ErrorClass::Retryable,
                    std::io::Error::other(error.to_string()),
                )
            })??;

        let Started {
            open,
            reader,
            child,
            pid,
        } = started;
        lock(&self.inner.open).insert(id.clone(), open);

        let observer = Arc::clone(&self.inner.observer);
        let host = Arc::downgrade(&self.inner);
        let watched = id.clone();
        std::thread::Builder::new()
            .name(format!("pushos-terminal-{id}"))
            .spawn(move || pump(&watched, reader, child, &*observer, &host))
            .map_err(|error| {
                // The terminal is running but nothing would ever read it, so it
                // is torn down rather than left as a leak.
                lock(&self.inner.open).remove(&id);
                TerminalError::host(
                    "no thread was available to read the terminal",
                    ErrorClass::Retryable,
                    error,
                )
            })?;

        debug!(%id, pid, "terminal opened");
        Ok(TerminalHandle { id, pid })
    }

    async fn write(&self, terminal: &SessionId, input: &str) -> Result<(), TerminalError> {
        let writer = {
            let open = lock(&self.inner.open);
            let entry = open
                .get(terminal)
                .ok_or_else(|| TerminalError::NoSuchTerminal {
                    terminal: terminal.clone(),
                })?;
            Arc::clone(&entry.writer)
        };

        let bytes = input.as_bytes().to_vec();
        let named = terminal.clone();
        tokio::task::spawn_blocking(move || {
            let mut writer = lock(&writer);
            writer.write_all(&bytes).and_then(|()| writer.flush())
        })
        .await
        .map_err(|error| {
            TerminalError::host(
                format!("writing to terminal `{named}` did not finish"),
                ErrorClass::Retryable,
                std::io::Error::other(error.to_string()),
            )
        })?
        .map_err(|error| {
            TerminalError::host(
                format!("could not type into terminal `{terminal}`"),
                ErrorClass::ComponentFailure,
                error,
            )
        })
    }

    async fn resize(&self, terminal: &SessionId, size: TerminalSize) -> Result<(), TerminalError> {
        let open = lock(&self.inner.open);
        let entry = open
            .get(terminal)
            .ok_or_else(|| TerminalError::NoSuchTerminal {
                terminal: terminal.clone(),
            })?;

        entry.master.resize(pty_size(size)).map_err(|error| {
            TerminalError::host(
                format!("could not resize terminal `{terminal}`"),
                ErrorClass::ComponentFailure,
                std::io::Error::other(format!("{error:#}")),
            )
        })
    }

    async fn close(&self, terminal: &SessionId) -> Result<(), TerminalError> {
        // Only the means of ending it is taken. The terminal itself stays open
        // until the reading thread reports the exit, which is what removes it:
        // dropping the write end here would send an end-of-file the terminal
        // echoes, leaving `^D` as the last thing the operator saw it say.
        let mut killer = {
            let mut open = lock(&self.inner.open);
            let entry = open
                .get_mut(terminal)
                .ok_or_else(|| TerminalError::NoSuchTerminal {
                    terminal: terminal.clone(),
                })?;
            entry.killer.take().ok_or_else(|| TerminalError::Finished {
                terminal: terminal.clone(),
            })?
        };

        // The reading thread notices the pty close and reports the exit; this
        // only asks the program to stop.
        killer.kill().map_err(|error| {
            TerminalError::host(
                format!("could not end terminal `{terminal}`"),
                ErrorClass::ComponentFailure,
                error,
            )
        })
    }
}

/// Everything one freshly started terminal produced.
struct Started {
    open: Open,
    reader: Box<dyn Read + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: Option<u32>,
}

/// Opens a pty and starts the program in it. Blocking.
fn start(spec: &TerminalSpec) -> Result<Started, TerminalError> {
    let pair = native_pty_system()
        .openpty(pty_size(spec.size))
        .map_err(|error| host_failure("could not open a terminal", &error))?;

    let mut command = CommandBuilder::new(&spec.program);
    command.args(&spec.args);
    command.cwd(&spec.cwd);
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    // Programs ask the terminal what it can do. Claiming more than PushOS
    // reports back would have them draw things nothing will interpret.
    command.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| host_failure(&format!("could not start `{}`", spec.program), &error))?;

    // The slave end is dropped so that the master sees the terminal close when
    // the program exits. Holding it would leave the read loop waiting forever.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| host_failure("could not read the terminal", &error))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| host_failure("could not write to the terminal", &error))?;

    let pid = child.process_id();
    Ok(Started {
        open: Open {
            writer: Arc::new(Mutex::new(writer)),
            killer: Some(child.clone_killer()),
            master: pair.master,
        },
        reader,
        child,
        pid,
    })
}

/// Reads a terminal until it closes, then reports how it ended.
///
/// Runs on its own thread for the whole life of the terminal.
fn pump(
    id: &SessionId,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    observer: &dyn TerminalObserver,
    host: &Weak<Inner>,
) {
    let mut filter = OutputFilter::new();
    let mut buffer = [0_u8; READ_CHUNK];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let text = filter.push(&buffer[..read]);
                if !text.is_empty() {
                    observer.observe(id, TerminalEvent::Output { text });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // A closed pty reports an input/output error on some systems and an
            // ordinary end of file on others. Both mean the same thing here.
            Err(error) => {
                debug!(%id, %error, "the terminal closed");
                break;
            }
        }
    }

    let code = match child.wait() {
        Ok(status) if status.signal().is_some() => None,
        Ok(status) => i32::try_from(status.exit_code()).ok(),
        Err(error) => {
            warn!(%id, %error, "could not collect the terminal's exit status");
            None
        }
    };

    // The program is gone, so nothing should be able to type into it.
    if let Some(inner) = host.upgrade() {
        lock(&inner.open).remove(id);
    }

    debug!(%id, code, "terminal finished");
    observer.observe(id, TerminalEvent::Exited { code });
}

fn validate(spec: &TerminalSpec) -> Result<(), TerminalError> {
    if spec.program.trim().is_empty() {
        return Err(TerminalError::host(
            "a terminal needs a program to run",
            ErrorClass::Validation,
            std::io::Error::other("no program"),
        ));
    }

    if !spec.cwd.is_dir() {
        // Spawning into a directory that is not there fails deep inside the
        // child with nothing useful to say, so it is caught here instead.
        return Err(TerminalError::host(
            format!("`{}` is not a directory", spec.cwd.display()),
            ErrorClass::Validation,
            std::io::Error::other("no such directory"),
        ));
    }

    Ok(())
}

const fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Wraps a host failure, which arrives as a chain rather than an error type.
fn host_failure(context: &str, error: &impl std::fmt::Display) -> TerminalError {
    TerminalError::host(
        context.to_owned(),
        ErrorClass::ComponentFailure,
        std::io::Error::other(format!("{error:#}")),
    )
}

/// Takes a lock without treating a panic elsewhere as a reason to panic here.
///
/// The state behind these locks is a handful of handles; a thread that died
/// while holding one has not left it inconsistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
