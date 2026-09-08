//! Talks to a real agent over the protocol, to check the adapter against
//! something other than a fake.
//!
//! ```text
//! cargo run -p pushos-acp --example talk -- claude "what is 2 + 2?"
//! ```
//!
//! Needs the agent's adapter to be installed and signed in, so it is an example
//! rather than a test: the suite must keep running on a machine with neither.

use std::sync::{Arc, Mutex};

use pushos_acp::{AcpBackend, AgentCommand};
use pushos_domain::ids::SessionId;
use pushos_domain::ports::{AgentBackend, AgentEvent, AgentObserver, SessionRequest};

#[derive(Debug, Default)]
struct Printer {
    finished: Mutex<bool>,
}

impl AgentObserver for Printer {
    fn observe(&self, session: &SessionId, event: AgentEvent) {
        match &event {
            AgentEvent::Said { text } => println!("  said: {text}"),
            AgentEvent::UsedTool { title, finished } => {
                println!(
                    "  tool: {title} ({})",
                    if *finished { "done" } else { "running" }
                );
            }
            AgentEvent::AskedPermission {
                question, options, ..
            } => {
                let names: Vec<_> = options.iter().map(|o| o.label.as_str()).collect();
                println!("  asks: {question} [{}]", names.join(", "));
            }
            AgentEvent::StateChanged { state } => println!("  state: {state}"),
            AgentEvent::Ended { reason } => {
                println!("  ended: {reason:?}");
                if let Ok(mut finished) = self.finished.lock() {
                    *finished = true;
                }
            }
        }
        let _ = session;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "claude".to_owned());
    let prompt = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Reply with exactly: ok".to_owned());

    let command = AgentCommand::known(&provider)
        .ok_or_else(|| format!("no default command for `{provider}`"))?;
    println!("starting {}", command.describe());

    let printer = Arc::new(Printer::default());
    let backend = AcpBackend::new(provider.as_str(), command, printer.clone());

    let handle = backend
        .start(SessionRequest {
            agent: pushos_domain::ids::AgentId::new("probe"),
            workspace: None,
            cwd: std::env::current_dir()?,
            objective: String::new(),
        })
        .await?;
    println!("session {} opened", handle.id);

    println!("prompting: {prompt}");
    backend.prompt(&handle, &prompt).await?;

    for _ in 0..600 {
        if *printer.finished.lock().map_err(|_| "poisoned")? {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    backend.stop(&handle).await?;
    println!("done");
    Ok(())
}
