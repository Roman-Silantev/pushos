//! The PushOS command line.

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod cli;
mod commands;
mod host;
mod logging;
mod presets;

use clap::Parser;

use crate::cli::{AppCommand, Cli, Command, HookCommand, PackCommand};

/// Exit status when PushOS could not do what it was asked.
const FAILURE: i32 = 1;

// Two workers rather than one per core. PushOS spends its life waiting on a
// MIDI port, a USB endpoint, a socket and a timer; the few things that truly
// block, speech recognition and pseudo-terminals, already run on blocking
// threads of their own. Ten idle workers were ten threads to wake and nothing
// to give them.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    // An agent reads what its hook prints, and shows what it logs. The hook
    // speaks only through its answer.
    if !matches!(
        cli.command,
        Command::Hook {
            command: HookCommand::Permission { .. }
        }
    ) {
        logging::install(cli.log.filter());
    }

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
        Command::Pack { command } => run_pack(command, cli.config.as_deref()),
        Command::App { command } => run_app(command, cli.config.as_deref()).await,
        Command::Hook { command } => match command {
            HookCommand::Install { yes } => commands::hook::install(yes),
            HookCommand::Uninstall => commands::hook::uninstall(),
            HookCommand::Permission { agent } => commands::hook::permission(agent).await,
        },
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

/// Carries out one app command.
async fn run_app(command: AppCommand, config: Option<&std::path::Path>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        match command {
            AppCommand::Install { sign } => commands::app::install(config, sign.as_deref()).await,
            AppCommand::Uninstall => commands::app::uninstall(),
            AppCommand::Status => commands::app::status().await,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (command, config);
        Err("the PushOS app is for macOS".to_owned())
    }
}

/// Carries out one pack command.
fn run_pack(command: PackCommand, config: Option<&std::path::Path>) -> Result<(), String> {
    use pushos_domain::pack::PackState;

    match command {
        PackCommand::List => commands::pack::list(config),
        PackCommand::Show { path } => commands::pack::show(config, &path),
        PackCommand::Install { path, yes, force } => {
            commands::pack::install(config, &path, yes, force)
        }
        PackCommand::Remove { pack } => commands::pack::remove(config, &pack),
        PackCommand::Enable { pack } => {
            commands::pack::set_state(config, &pack, PackState::Enabled)
        }
        PackCommand::Disable { pack } => {
            commands::pack::set_state(config, &pack, PackState::Disabled)
        }
    }
}
