//! Lists every coding session on this Mac that PushOS did not start, and reads one.
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
use pushos_macos::{MacSessions, SystemProcessRunner};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let processes: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner::new());
    let watcher = MacSessions::new(processes);

    for report in watcher.report().await {
        let mark = if report.usable { "ok" } else { "--" };
        println!("{mark} {:<12} {}", report.source, report.detail);
    }
    println!();

    let sessions = watcher.discover().await?;
    println!("{} session(s) in {}", sessions.len(), watcher.describe());
    for session in &sessions {
        println!(
            "  {:<14} {:<8} {:<24} {}",
            session.id.as_str(),
            session.activity.describe(),
            session.label(),
            session.detail()
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
