//! Answering coding agents' permission questions from the Push.
//!
//! `pushos hook permission` is the command Claude Code and Codex run when they
//! are about to ask permission. It puts the question to the running PushOS and
//! prints the operator's answer the way the agent reads one. Whatever goes
//! wrong, it prints nothing: that leaves the agent's own prompt, on screen the
//! whole time, to decide, which is exactly what would have happened without
//! PushOS.
//!
//! `pushos hook install` adds that command to each installed agent's hooks,
//! after saying what it will change.

pub(crate) mod question;
mod settings;

use std::io::{IsTerminal as _, Write as _};
use std::time::Duration;

use pushos_api::ControlClient;
use pushos_macos::SystemProcessRunner;
use tokio::io::AsyncReadExt as _;

use self::question::Agent;
use self::settings::HookFile;
use super::paths;

/// How much a hook's input may be. A permission request is a few kilobytes.
const MOST_INPUT: u64 = 1024 * 1024;

/// How long to wait for the operator.
///
/// Under the ten minutes an agent waits for the hook, so an answer given at
/// the last moment still arrives while the agent is listening.
const PATIENCE: Duration = Duration::from_secs(590);

/// Puts the question on stdin to the Push, and prints the answer if one comes.
pub(crate) async fn permission(agent: Agent) -> Result<(), String> {
    let mut input = String::new();
    if tokio::io::stdin()
        .take(MOST_INPUT)
        .read_to_string(&mut input)
        .await
        .is_err()
    {
        return Ok(());
    }
    let Some(mut question) = question::read(agent, &input) else {
        return Ok(());
    };

    question.device = pushos_macos::terminal_above(
        &SystemProcessRunner::new(),
        std::os::unix::process::parent_id(),
    )
    .await;

    let Ok(socket) = paths::control_socket() else {
        return Ok(());
    };
    let Ok(mut client) = ControlClient::connect(socket).await else {
        return Ok(());
    };
    if client.handshake().await.is_err() {
        return Ok(());
    }

    if let Ok(answer) = client.ask(question, PATIENCE).await
        && let Some(decision) = answer.decision
    {
        println!("{}", question::decision(decision));
    }
    Ok(())
}

/// Adds PushOS's hook to every agent installed here.
pub(crate) fn install(yes: bool) -> Result<(), String> {
    let files = HookFile::installed();
    if files.is_empty() {
        return Err("neither Claude Code nor Codex is installed for this user".to_owned());
    }
    let program = program()?;

    println!("PushOS will answer permission questions from the Push for:");
    for file in &files {
        println!("  {:<12} {}", file.agent.name(), file.path.display());
    }
    println!();
    println!("Each file gets one hook, running:");
    for file in &files {
        println!("  {}", settings::command(&program, file.agent));
    }
    println!();
    println!("Nothing else in the files changes, and a copy of each is kept beside it.");
    println!("Each agent still shows its own prompt; whichever is answered first counts.");

    if !confirmed(yes)? {
        return Err("nothing was changed".to_owned());
    }

    for file in &files {
        let document = settings::read(&file.path)?;
        let installed =
            settings::with_pushos(document, &settings::command(&program, file.agent))
                .map_err(|reason| format!("{} was left alone: {reason}", file.path.display()))?;
        settings::write(&file.path, &installed)?;
        println!("added to {}", file.path.display());
    }

    if files.iter().any(|file| file.agent == Agent::Codex) {
        println!();
        println!("Codex runs a new hook only once you trust it: open Codex, run /hooks,");
        println!("and trust the PushOS one.");
    }
    Ok(())
}

/// Takes PushOS's hook out of every agent installed here.
pub(crate) fn uninstall() -> Result<(), String> {
    for file in HookFile::installed() {
        let document = settings::read(&file.path)?;
        let (removed, was_there) = settings::without_pushos(document);
        if was_there {
            settings::write(&file.path, &removed)?;
            println!("removed from {}", file.path.display());
        }
    }
    Ok(())
}

/// Every agent installed here, and whether PushOS answers for it.
pub(crate) fn installed() -> Vec<(Agent, bool)> {
    HookFile::installed()
        .into_iter()
        .map(|file| (file.agent, file.has_pushos()))
        .collect()
}

/// The pushos the hook should run.
///
/// The installed app when there is one, because that is the PushOS that is
/// running and it outlives any build directory. This one otherwise.
fn program() -> Result<std::path::PathBuf, String> {
    let home = pushos_config::paths::home_directory()
        .ok_or_else(|| "could not work out your home directory; set HOME".to_owned())?;
    let app = pushos_macos::app::AppLayout::for_user(&home).executable();
    if app.is_file() {
        return Ok(app);
    }
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("could not find this pushos: {error}"))
}

/// Whether the operator agreed. With no terminal there is nobody to ask.
fn confirmed(yes: bool) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "nothing was changed: there is nobody to ask. Pass --yes to go ahead.".to_owned(),
        );
    }
    print!("\nadd the hooks? [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("could not ask: {error}"))?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("could not read the answer: {error}"))?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}
