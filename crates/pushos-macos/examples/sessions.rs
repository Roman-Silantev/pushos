//! Lists the terminals the operator already had open, and reads one.
//!
//! ```text
//! cargo run -p pushos-macos --example sessions
//! cargo run -p pushos-macos --example sessions -- /dev/ttys003
//! ```
//!
//! A development tool. It reads and never types, so running it cannot disturb
//! whatever those sessions are doing.

use std::sync::Arc;

use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachedSessions, ProcessRunner};
use pushos_macos::{SystemProcessRunner, TerminalAppSessions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let processes: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner::new());
    let watcher = TerminalAppSessions::new(processes);

    let sessions = watcher.discover().await?;
    println!("{} session(s) in {}", sessions.len(), watcher.describe());
    for session in &sessions {
        println!(
            "  {:<10} {:<7} {}",
            session.device(),
            if session.busy { "busy" } else { "idle" },
            session.label()
        );
    }

    if let Some(wanted) = std::env::args().nth(1) {
        let last = watcher.read(&AttachedId::new(&wanted), 300).await?;
        println!("\nlast of {wanted}:");
        for line in last.lines().filter(|line| !line.trim().is_empty()) {
            println!("  {line}");
        }
    }
    Ok(())
}
