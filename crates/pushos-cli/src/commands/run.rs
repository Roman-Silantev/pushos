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
use pushos_runtime::{Backoff, Runtime, Shutdown};
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

    if fake {
        run_once_on_fake_surface(&config, &storage, shutdown.clone()).await?;
    } else {
        serve_hardware(&config, &storage, &shutdown).await;
    }

    shutdown.stop().await;
    Ok(())
}

/// Connects, serves, and waits for the surface to come back.
async fn serve_hardware(config: &Arc<ConfigStore>, storage: &StorageWriter, shutdown: &Shutdown) {
    let mut backoff = Backoff::new();

    while !shutdown.is_cancelled() {
        match Push2Device::connect(PortRole::User) {
            Ok((device, input)) => {
                backoff.reset();
                let output: Arc<dyn PushOutput> = Arc::new(device);

                match PushRenderer::new() {
                    Ok(renderer) => {
                        build_runtime(config, storage)
                            .run(Box::new(input), output, renderer, shutdown.clone())
                            .await;
                        info!("the Push 2 went away; waiting for it to return");
                    }
                    Err(error) => {
                        warn!(%error, "the renderer could not be built");
                        return;
                    }
                }
            }
            Err(error) if error.is_absent() => {
                // Not a fault. PushOS is expected to run with nothing plugged in.
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
    config: &Arc<ConfigStore>,
    storage: &StorageWriter,
    shutdown: Shutdown,
) -> Result<(), String> {
    info!("running against a simulated surface; no hardware is being used");
    let (surface, input) = FakePush::new();
    let renderer = PushRenderer::new().map_err(|error| error.to_string())?;

    build_runtime(config, storage)
        .run(Box::new(input), Arc::new(surface), renderer, shutdown)
        .await;
    Ok(())
}

fn build_runtime(config: &Arc<ConfigStore>, storage: &StorageWriter) -> Runtime {
    let mut runtime = Runtime::new(Arc::clone(config)).with_storage(storage.handle());

    // Without a socket PushOS still runs; it simply cannot be configured from
    // Studio. That is worth saying rather than refusing to start over.
    match open_control_socket() {
        Ok(server) => runtime = runtime.with_control_socket(server),
        Err(reason) => warn!(reason, "PushOS Studio will not be able to connect"),
    }

    for provider in host::providers(&config.current()) {
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
