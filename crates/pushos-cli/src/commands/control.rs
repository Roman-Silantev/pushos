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
