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
    let running = build_runtime(&config, &storage, worktrees)
        .start(&shutdown)
        .await;

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

    while !shutdown.is_cancelled() {
        match Push2Device::connect(PortRole::User) {
            Ok((device, input)) => {
                backoff.reset();
                announced = false;
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
                warn!(%error, "could not take the Push 2");
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

    // Push to talk. Built before the providers, because one of them is the
    // namespace that holds it, and after the rest, because what it hears is
    // carried out through the same dispatcher a finger uses.
    let listening = host::voice(&current);
    if let Some(listener) = &listening {
        runtime = runtime.with_voice(Arc::clone(listener));
    }

    // Without a socket PushOS still runs; it simply cannot be configured from
    // Studio. That is worth saying rather than refusing to start over.
    match open_control_socket() {
        Ok(server) => runtime = runtime.with_control_socket(server),
        Err(reason) => warn!(reason, "PushOS Studio will not be able to connect"),
    }

    for provider in host::providers(
        &current,
        agents.as_ref(),
        &terminals,
        &workspaces,
        workflows.as_ref(),
        listening.as_ref(),
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

fn open_control_socket() -> Result<ControlServer, String> {
    let path = paths::control_socket()?;
    ControlServer::bind(path).map_err(|error| error.to_string())
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
            result = tokio::signal::ctrl_c() => {
                if result.is_ok() {
                    info!("interrupted");
                }
                // Begin rather than stop: this task is registered with the same
                // coordinator, so waiting here would wait for itself.
                interrupted.begin();
            }
        }
    });
}

async fn wait(backoff: &mut Backoff, shutdown: &Shutdown) {
    let delay = backoff.next_delay();
    tokio::select! {
        biased;
        () = shutdown.cancelled() => {}
        () = tokio::time::sleep(delay) => {}
    }
}
