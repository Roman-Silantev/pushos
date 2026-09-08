//! Talking to a running PushOS from the command line.
//!
//! The same socket Studio uses. Having it here means the protocol is exercised
//! by something that ships, and that a surface can be inspected without a
//! graphical application.

use pushos_api::protocol::{Request, Response};
use pushos_api::{ClientError, ControlClient};

use super::paths;

/// Prints what a running PushOS is doing.
pub(crate) async fn status() -> Result<(), String> {
    let mut client = connect().await?;
    let status = client.handshake().await.map_err(|error| describe(&error))?;

    println!("PushOS {} on protocol {}", status.version, status.protocol);
    println!("  surface:      {}", status.surface.describe());
    println!(
        "  page:         {}",
        status.page.as_deref().unwrap_or("none")
    );
    println!(
        "  workspace:    {}",
        status.workspace.as_deref().unwrap_or("none")
    );
    println!("  bindings:     {}", status.binding_count);
    println!("  config:       {}", status.config_root);
    Ok(())
}

/// Prints every binding in force, grouped by where it applies.
pub(crate) async fn bindings() -> Result<(), String> {
    let mut client = connect().await?;
    client.handshake().await.map_err(|error| describe(&error))?;

    let Response::Bindings(list) = client
        .send(&Request::Bindings)
        .await
        .map_err(|error| describe(&error))?
    else {
        return Err("PushOS answered with something unexpected".to_owned());
    };

    if list.bindings.is_empty() {
        println!("nothing is bound");
        return Ok(());
    }

    let mut sorted = list.bindings;
    sorted.sort_by(|a, b| {
        scope_of(&a.address)
            .cmp(&scope_of(&b.address))
            .then_with(|| a.address.control.cmp(&b.address.control))
    });

    let mut current = String::new();
    for binding in sorted {
        let scope = scope_of(&binding.address);
        if scope != current {
            println!("\n{scope}");
            current = scope;
        }

        let caption = binding.label.as_deref().unwrap_or("");
        let target = binding
            .target
            .as_deref()
            .map(|t| format!(" {t}"))
            .unwrap_or_default();
        println!(
            "  {:<20} {:<12} {}{}{}",
            binding.address.control,
            binding.address.gesture,
            binding.action,
            target,
            if caption.is_empty() {
                String::new()
            } else {
                format!("   ({caption})")
            }
        );
    }
    Ok(())
}

/// Prints every session, and how a binding would name it.
///
/// The standing target is printed alongside the exact one because they are not
/// interchangeable: binding to a session identifier stops meaning anything the
/// moment that session ends.
pub(crate) async fn sessions() -> Result<(), String> {
    let mut client = connect().await?;
    client.handshake().await.map_err(|error| describe(&error))?;

    let Response::Sessions(list) = client
        .send(&Request::Sessions)
        .await
        .map_err(|error| describe(&error))?
    else {
        return Err("PushOS answered with something unexpected".to_owned());
    };

    if list.sessions.is_empty() {
        println!("nothing is running");
        return Ok(());
    }

    for session in list.sessions {
        let mark = if session.selected { '>' } else { ' ' };
        println!(
            "{mark} {:<9} {:<16} {:<10} {}",
            // The namespace that drives it, which is also what a binding
            // writes, so the listing and the action stay in step.
            session.kind.provider(),
            session.name,
            session.status,
            session.detail.as_deref().unwrap_or("")
        );
        println!(
            "  {:<9} bind to {}",
            "",
            session
                .standing_target
                .as_deref()
                .unwrap_or(session.target.as_str())
        );
    }
    Ok(())
}

/// Prints every project, marking the one in effect.
pub(crate) async fn workspaces() -> Result<(), String> {
    let mut client = connect().await?;
    client.handshake().await.map_err(|error| describe(&error))?;

    let Response::Workspaces(list) = client
        .send(&Request::Workspaces)
        .await
        .map_err(|error| describe(&error))?
    else {
        return Err("PushOS answered with something unexpected".to_owned());
    };

    if list.workspaces.is_empty() {
        println!("no projects are configured");
        return Ok(());
    }

    for workspace in list.workspaces {
        let mark = if workspace.current { '>' } else { ' ' };
        println!("{mark} {:<16} {}", workspace.id, workspace.name);
        println!(
            "  {:<16} {}{}",
            "",
            workspace.root,
            if workspace.root_exists {
                String::new()
            } else {
                "   (missing)".to_owned()
            }
        );

        let mut notes = Vec::new();
        if workspace.isolate_agents {
            notes.push("a tree per agent".to_owned());
        }
        if !workspace.apps.is_empty() {
            notes.push(format!("opens in {}", workspace.apps.join(", ")));
        }
        if let Some(page) = &workspace.home_page {
            notes.push(format!("starts on {page}"));
        }
        if !notes.is_empty() {
            println!("  {:<16} {}", "", notes.join("; "));
        }
    }
    Ok(())
}

fn scope_of(address: &pushos_config::BindingAddress) -> String {
    match (&address.workspace, &address.page) {
        (Some(workspace), Some(page)) => format!("workspace {workspace}, page {page}"),
        (Some(workspace), None) => format!("workspace {workspace}"),
        (None, Some(page)) => format!("page {page}"),
        (None, None) => "everywhere".to_owned(),
    }
}

async fn connect() -> Result<ControlClient, String> {
    ControlClient::connect(paths::control_socket()?)
        .await
        .map_err(|error| describe(&error))
}

/// Renders a client failure, keeping every itemised problem.
fn describe(error: &ClientError) -> String {
    use std::fmt::Write as _;

    let mut report = error.to_string();
    for problem in error.problems() {
        // Writing into a `String` cannot fail.
        let _ = write!(report, "\n  {problem}");
    }
    if error.is_not_running() {
        report.push_str("\n  start it with `pushos run`");
    }
    report
}
