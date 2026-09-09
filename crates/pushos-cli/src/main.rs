//! The PushOS command line.

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod cli;
mod commands;
mod host;
mod presets;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

/// Exit status when PushOS could not do what it was asked.
const FAILURE: i32 = 1;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    install_logging(cli.log.filter());

    let outcome = match cli.command {
        Command::Run { fake } => commands::run::execute(cli.config.as_deref(), fake).await,
        Command::Check => commands::check::execute(cli.config.as_deref()),
        Command::Doctor => commands::doctor::execute(cli.config.as_deref()).await,
        Command::Status => commands::control::status().await,
        Command::Bindings => commands::control::bindings().await,
        Command::Sessions => commands::control::sessions().await,
        Command::Workspaces => commands::control::workspaces().await,
        Command::Presets => {
            commands::init::list();
            Ok(())
        }
        Command::Init { preset, force } => {
            commands::init::execute(cli.config.as_deref(), &preset, force)
        }
    };

    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("{report}");
            std::process::ExitCode::from(u8::try_from(FAILURE).unwrap_or(1))
        }
    }
}

/// Sets up logging, letting `RUST_LOG` override the chosen level.
fn install_logging(default: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
}
