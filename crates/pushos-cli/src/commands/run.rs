//! Running PushOS.
//!
//! The runtime outlives the hardware. If no Push 2 is attached it waits; when
//! one appears it takes it; when one goes away it waits again. Configuration is
//! watched throughout, and an invalid edit is reported without interrupting
//! anything.

use std::path::Path;
use std::sync::Arc;

use pushos_api::ControlServer;
use pushos_config::{ConfigStore, ConfigWatcher};
use pushos_domain::ports::PushOutput;
use pushos_push2::{PortRole, Push2Device};
use pushos_runtime::{Backoff, RunningRuntime, Runtime, Shutdown};
use pushos_storage::StorageWriter;
use pushos_testkit::FakePush;
use pushos_ui::PushRenderer;
use tracing::{info, warn};

use crate::host;

use super::{check, paths};

/// Runs PushOS until interrupted.
pub(crate) async fn execute(requested: Option<&Path>, fake: bool) -> Result<(), String> {
    // Taken before anything else. Another PushOS holding it means two copies
    // would fight over the Push 2, and the operator would see a surface that
    // half works rather than one that says what is wrong.
    let control = match open_control_socket() {
        Ok(server) => Some(server),
        Err(SocketProblem::AlreadyRunning) => {
            return Err(
                "PushOS is already running. Stop it first, or run `pushos status` \
                        to see what it is doing."
                    .to_owned(),
            );
        }
        Err(SocketProblem::Unavailable(reason)) => {
            warn!(reason, "PushOS Studio will not be able to connect");
            None
        }
    };

    let root = paths::config_root(requested)?;
    let config = Arc::new(ConfigStore::load(&root).map_err(|error| check::describe(&error))?);
    info!(root = %root.display(), bindings = config.current().bindings.len(), "configuration loaded");

    let storage = StorageWriter::open(&paths::state_file()?)
        .map_err(|error| format!("could not open the PushOS database: {error}"))?;

    let shutdown = Shutdown::new();
    watch_configuration(Arc::clone(&config), &shutdown);
    stop_on_interrupt(&shutdown);

    let worktrees = paths::worktree_root()?;

    // Started before any surface, and kept for the life of the process. Agents,
    // terminals, projects and the control socket are not the hardware's to own:
    // a Push 2 unplugged and plugged back in must find its work still running,
    // and PushOS with nothing attached must still be configurable.
    // Kept for as long as PushOS runs. Dropping it stops the listening, and a
    // Mac that sleeps unheard leaves the Push 2 lit until morning.
    let (_sleep_watch, power) = listen_for_sleep();

    let mut runtime = build_runtime(&config, &storage, worktrees, control);
    if let Some(power) = power {
        runtime = runtime.with_power(power);
    }
    let running = runtime.start(&shutdown).await;

    if fake {
        run_once_on_fake_surface(&running, shutdown.clone()).await?;
    } else {
        serve_hardware(&running, &shutdown).await;
    }

    running.stop().await;
    shutdown.stop().await;
    Ok(())
}

/// Takes the surface whenever there is one, and waits when there is not.
///
/// Only the surface comes and goes. Everything else was started before this and
/// is still there between connections.
async fn serve_hardware(running: &RunningRuntime, shutdown: &Shutdown) {
    let mut backoff = Backoff::new();
    let mut announced = false;
    let mut last_claim_failure: Option<String> = None;

    while !shutdown.is_cancelled() {
        match Push2Device::connect(PortRole::User) {
            Ok((device, input)) => {
                backoff.reset();
                announced = false;
                last_claim_failure = None;
                let output: Arc<dyn PushOutput> = Arc::new(device);

                match PushRenderer::new() {
                    Ok(renderer) => {
                        running
                            .serve(Box::new(input), output, renderer, shutdown)
                            .await;
                        info!("the Push 2 went away; everything else is still running");
                    }
                    Err(error) => {
                        warn!(%error, "the renderer could not be built");
                        return;
                    }
                }
            }
            Err(error) if error.is_absent() => {
                // Not a fault. PushOS is expected to run with nothing plugged
                // in, and stays configurable from Studio while it waits. Said
                // once rather than on every attempt.
                if !announced {
                    info!("no Push 2 attached; waiting for one, and answering Studio meanwhile");
                    announced = true;
                }
                wait(&mut backoff, shutdown).await;
            }
            Err(error) => {
                // Retried every few seconds for as long as, say, another
                // application holds it. The same reason is said once.
                let reason = error.to_string();
                if last_claim_failure.as_deref() != Some(reason.as_str()) {
                    warn!(%error, "could not take the Push 2; trying again quietly");
                    last_claim_failure = Some(reason);
                }
                wait(&mut backoff, shutdown).await;
            }
        }
    }
}

/// Runs against a simulated surface, for working away from the hardware.
async fn run_once_on_fake_surface(
    running: &RunningRuntime,
    shutdown: Shutdown,
) -> Result<(), String> {
    info!("running against a simulated surface; no hardware is being used");
    let (surface, input) = FakePush::new();
    let renderer = PushRenderer::new().map_err(|error| error.to_string())?;

    running
        .serve(Box::new(input), Arc::new(surface), renderer, &shutdown)
        .await;
    Ok(())
}

fn build_runtime(
    config: &Arc<ConfigStore>,
    storage: &StorageWriter,
    worktree_root: std::path::PathBuf,
    control: Option<ControlServer>,
) -> Runtime {
    let mut runtime = Runtime::new(Arc::clone(config)).with_storage(storage.handle());

    let current = config.current();

    // Built first, because everything else asks it where to work.
    let workspaces = host::workspaces(
        &current,
        Arc::new(storage.handle()),
        Arc::new(pushos_macos::SystemProcessRunner::new()),
        worktree_root,
    );
    let context: Arc<dyn pushos_domain::ports::WorkspaceContext> = Arc::clone(&workspaces) as _;

    // The trees agents worked in last time are free now, and the clean ones go.
    // In the background: it runs git once per tree, and nothing waits on it.
    let tidying = Arc::clone(&workspaces);
    tokio::spawn(async move { tidying.tidy().await });

    // Agent and terminal activity is reported from the thread reading a
    // program's output, so each arrives on its own stream rather than through
    // the surface.
    let (reporter, updates) = pushos_runtime::AgentReporter::new();
    let agents = host::agents(&current, Arc::new(reporter), Arc::clone(&context));

    if let Some(supervisor) = &agents {
        info!(roles = current.agents.len(), "agent roles available");
        runtime = runtime.with_agents(Arc::clone(supervisor), updates);
    }

    let (terminal_reporter, terminal_updates) = pushos_runtime::TerminalReporter::new();
    let terminals = host::terminals(&current, Arc::new(terminal_reporter), context);
    runtime = runtime.with_terminals(Arc::clone(&terminals), terminal_updates);
    runtime = runtime.with_workspaces(Arc::clone(&workspaces));

    // Workflows keep their runs in the same database everything else uses, so
    // a run survives PushOS stopping.
    let (run_reporter, run_updates) = pushos_runtime::RunReporter::new();
    let workflows = host::workflows(
        &current,
        Arc::new(storage.handle()),
        Arc::new(run_reporter),
        agents.as_ref(),
        &terminals,
    );
    if let Some(engine) = &workflows {
        runtime = runtime.with_workflows(Arc::clone(engine), run_updates);
    }

    // So the number of sessions left running follows what the Mac can
    // actually spare, not only what the configuration asked for.
    runtime = runtime.with_memory_pressure(Arc::new(pushos_macos::MacMemoryPressure::new(
        Arc::new(pushos_macos::SystemProcessRunner::new()),
    )));

    // Push to talk. Built before the providers, because one of them is the
    // namespace that holds it, and after the rest, because what it hears is
    // carried out through the same dispatcher a finger uses.
    let listening = host::voice(&current);
    if let Some(listener) = &listening {
        runtime = runtime.with_voice(Arc::clone(listener));
    }

    // Notes. The same reasoning: a briefing reaches an agent through the
    // dispatcher, so the provider needs it and the runtime is what gives it.
    let notes = host::memory(&current, Some(Arc::new(storage.handle())));
    if let Some(provider) = &notes {
        runtime = runtime.with_memory(Arc::clone(provider));
    }

    // The terminals the operator already had open. PushOS did not start these
    // and cannot own them, but it can show what they are doing and type into
    // them, which is most of what a pad is for.
    let sessions = host::attached(&current);
    if let Some(provider) = &sessions {
        runtime = runtime.with_attached(Arc::clone(provider));
    }

    // Several actions behind one gesture. Built here rather than among the
    // rest, because every step goes back through the dispatcher and the
    // runtime is what hands it over once the dispatcher exists.
    let runs = host::sequences(&current);
    if let Some(provider) = &runs {
        runtime = runtime.with_sequences(Arc::clone(provider));
    }

    // Without a socket PushOS still runs; it simply cannot be configured from
    // Studio. That is worth saying rather than refusing to start over.
    if let Some(server) = control {
        runtime = runtime.with_control_socket(server);
    }

    for provider in host::providers(
        &current,
        agents.as_ref(),
        &terminals,
        &workspaces,
        workflows.as_ref(),
        listening.as_ref(),
        notes.as_ref(),
        sessions.as_ref(),
        runs.as_ref(),
    ) {
        let name = provider.name();
        match runtime.with_provider(provider) {
            Ok(next) => runtime = next,
            Err(error) => {
                // Cannot happen with the shipped set, which is covered by a
                // test; reported rather than ignored in case that changes.
                warn!(%error, %name, "a provider was not installed");
                runtime = Runtime::new(Arc::clone(config)).with_storage(storage.handle());
            }
        }
    }

    runtime
}

/// Why the control socket could not be taken.
enum SocketProblem {
    /// Another PushOS holds it.
    AlreadyRunning,
    /// Something else, which PushOS can run without.
    Unavailable(String),
}

fn open_control_socket() -> Result<ControlServer, SocketProblem> {
    let path = paths::control_socket().map_err(SocketProblem::Unavailable)?;
    ControlServer::bind(path).map_err(|error| {
        if error.is_already_running() {
            SocketProblem::AlreadyRunning
        } else {
            SocketProblem::Unavailable(error.to_string())
        }
    })
}

fn watch_configuration(config: Arc<ConfigStore>, shutdown: &Shutdown) {
    let mut watcher = match ConfigWatcher::start(Arc::clone(&config)) {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!(%error, "configuration will not reload automatically");
            return;
        }
    };

    let watching = shutdown.clone();
    shutdown.spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = watching.cancelled() => break,
                reloaded = watcher.next_reload() => match reloaded {
                    Some(true) => info!(
                        bindings = config.current().bindings.len(),
                        "configuration reloaded"
                    ),
                    // The failure has already been reported with its problems;
                    // the running surface is untouched.
                    Some(false) => {}
                    None => break,
                },
            }
        }
    });
}

fn stop_on_interrupt(shutdown: &Shutdown) {
    let interrupted = shutdown.clone();
    shutdown.spawn(async move {
        tokio::select! {
            biased;
            () = interrupted.cancelled() => {}
            asked = asked_to_stop() => {
                info!(signal = asked, "asked to stop");
                // Begin rather than stop: this task is registered with the same
                // coordinator, so waiting here would wait for itself.
                interrupted.begin();
            }
        }
    });
}

/// Starts hearing the Mac go to sleep, when this Mac will say.
///
/// Reported and carried on without when it will not: PushOS still rests the
/// surface on its own timers, it simply cannot put it out ahead of a sleep.
#[cfg(target_os = "macos")]
fn listen_for_sleep() -> (
    Option<pushos_macos::SleepWatch>,
    Option<tokio::sync::watch::Receiver<pushos_domain::rest::MachinePower>>,
) {
    match pushos_macos::SleepWatch::start() {
        Ok((watch, power)) => (Some(watch), Some(power)),
        Err(error) => {
            warn!(%error, "the Push 2 will not be put out before the Mac sleeps");
            (None, None)
        }
    }
}

#[cfg(not(target_os = "macos"))]
const fn listen_for_sleep() -> (
    Option<()>,
    Option<tokio::sync::watch::Receiver<pushos_domain::rest::MachinePower>>,
) {
    (None, None)
}

/// Waits for any of the ordinary ways a process is told to stop.
///
/// Not only Ctrl-C. Closing the terminal window PushOS runs in sends a hangup,
/// and `kill`, logging out and a service manager all send terminate. Any of
/// those without a handler ends the process on the spot, before the Push 2 is
/// told anything, and a Push 2 holds the last lights it was given for as long
/// as it has power: every pad lit, all night. Handled, they all go the same way
/// Ctrl-C does, which puts the lights out and hands the device back.
///
/// Only a kill that cannot be caught still leaves them on, and nothing in a
/// process can answer that.
async fn asked_to_stop() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // A signal that cannot be listened for is left out rather than taking
        // the others with it.
        let listen = |kind: SignalKind| {
            signal(kind)
                .inspect_err(|error| warn!(%error, "a stop signal cannot be listened for"))
                .ok()
        };
        let mut terminate = listen(SignalKind::terminate());
        let mut hangup = listen(SignalKind::hangup());

        tokio::select! {
            _ = tokio::signal::ctrl_c() => "interrupt",
            () = next_of(terminate.as_mut()) => "terminate",
            () = next_of(hangup.as_mut()) => "hangup",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "interrupt"
    }
}

/// Waits for a signal, or forever when it could not be listened for.
#[cfg(unix)]
async fn next_of(stream: Option<&mut tokio::signal::unix::Signal>) {
    match stream {
        Some(stream) => {
            stream.recv().await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn wait(backoff: &mut Backoff, shutdown: &Shutdown) {
    let delay = backoff.next_delay();
    tokio::select! {
        biased;
        () = shutdown.cancelled() => {}
        () = tokio::time::sleep(delay) => {}
    }
}
