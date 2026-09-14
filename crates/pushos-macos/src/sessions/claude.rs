//! What Claude Code says its sessions are doing.
//!
//! Claude Code knows which of its sessions is working, which is waiting and
//! what for, wherever each one runs: a terminal, an editor's terminal, an
//! editor's panel. `claude agents --json` is its supported way to ask, which
//! makes this a fact rather than a reading of somebody's screen.
//!
//! Asking starts a program, which costs a tenth of a second of work, and
//! PushOS looks every few seconds for as long as it runs. So it asks only when
//! something may have changed: Claude Code keeps one small file per running
//! session and rewrites it when that session's state changes, and seeing
//! those files unchanged costs a directory listing. The files are only ever
//! looked at, never read; what they hold is Claude Code's business.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use pushos_domain::attached::Activity;
use pushos_domain::ports::{ProcessRunner, ProcessSpec};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{debug, warn};

use super::programs;

/// How long an unchanged answer is believed for.
///
/// A session that crashes leaves its file behind unchanged, so the files alone
/// would show it forever. Asking once a minute regardless clears it.
const RECONCILE: Duration = Duration::from_mins(1);

/// How long asking may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// One Claude Code session that is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClaudeSession {
    /// Its process.
    pub(super) pid: u32,
    /// Claude Code's identity for it.
    pub(super) id: String,
    /// The name Claude Code shows for it.
    pub(super) name: String,
    /// What it says it is doing.
    pub(super) activity: Activity,
}

/// Asks Claude Code about its sessions, and remembers the answer.
#[derive(Debug)]
pub(super) struct ClaudeCode {
    processes: Arc<dyn ProcessRunner>,
    /// Where Claude Code keeps a file for each running session.
    registry: Option<PathBuf>,
    /// Where Claude Code is, when that is already known. Looked for otherwise.
    program: Option<PathBuf>,
    remembered: Mutex<Remembered>,
    /// Whether a failure to ask has been reported, so it is said once.
    complained: AtomicBool,
}

#[derive(Debug, Default)]
struct Remembered {
    /// The registry as it looked when the answer was given.
    registry: Option<Vec<(String, SystemTime)>>,
    sessions: Vec<ClaudeSession>,
    asked: Option<Instant>,
}

impl ClaudeCode {
    /// Asks the Claude Code installed for this user.
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            processes,
            registry: registry_directory(),
            program: None,
            remembered: Mutex::new(Remembered::default()),
            complained: AtomicBool::new(false),
        }
    }

    /// Asks a particular Claude Code, watching a particular registry.
    #[cfg(test)]
    pub(super) fn at(processes: Arc<dyn ProcessRunner>, program: &str, registry: &Path) -> Self {
        Self {
            program: Some(PathBuf::from(program)),
            registry: Some(registry.to_owned()),
            ..Self::new(processes)
        }
    }

    /// Where Claude Code is installed, if it is.
    pub(super) fn program(&self) -> Option<PathBuf> {
        self.program.clone().or_else(|| programs::find("claude"))
    }

    /// Every running session, as Claude Code last described them.
    ///
    /// Never a failure: a Mac without Claude Code, or one where asking went
    /// wrong, simply has no sessions to add to what the terminals show.
    pub(super) async fn sessions(&self) -> Vec<ClaudeSession> {
        let mut remembered = self.remembered.lock().await;
        let registry = match &self.registry {
            Some(directory) => snapshot(directory).await,
            None => Vec::new(),
        };

        let unchanged = remembered.registry.as_ref() == Some(&registry)
            && remembered
                .asked
                .is_some_and(|asked| asked.elapsed() < RECONCILE);
        if unchanged {
            return remembered.sessions.clone();
        }

        match self.ask().await {
            Ok(sessions) => {
                self.complained.store(false, Ordering::Relaxed);
                remembered.sessions = sessions;
            }
            Err(reason) => {
                if !self.complained.swap(true, Ordering::Relaxed) {
                    warn!(%reason, "cannot ask Claude Code what its sessions are doing");
                }
                remembered.sessions.clear();
            }
        }
        remembered.registry = Some(registry);
        remembered.asked = Some(Instant::now());
        remembered.sessions.clone()
    }

    async fn ask(&self) -> Result<Vec<ClaudeSession>, String> {
        let Some(program) = self.program() else {
            // Not installed is not worth a warning on every Mac without it.
            self.complained.store(true, Ordering::Relaxed);
            return Ok(Vec::new());
        };
        let spec = ProcessSpec::new(
            program.to_string_lossy(),
            ["agents".to_owned(), "--json".to_owned()],
        )
        .within(PATIENCE)
        .capturing(1024 * 1024);

        let outcome = self
            .processes
            .run(&spec)
            .await
            .map_err(|error| error.to_string())?;
        if !outcome.succeeded() {
            return Err(outcome.stderr_tail.trim().to_owned());
        }
        let sessions = parse(&outcome.stdout_tail)?;
        debug!(
            sessions = sessions.len(),
            "Claude Code described its sessions"
        );
        Ok(sessions)
    }
}

/// Where Claude Code keeps its per-session files: in its configuration
/// directory, which the operator can move.
fn registry_directory() -> Option<PathBuf> {
    let configuration = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;
    Some(configuration.join("sessions"))
}

/// The names and modification times of everything in a directory, in order.
///
/// Nothing when the directory does not exist, which is the same as no sessions.
async fn snapshot(directory: &Path) -> Vec<(String, SystemTime)> {
    let mut seen = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return seen;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let modified = entry
            .metadata()
            .await
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        seen.push((entry.file_name().to_string_lossy().into_owned(), modified));
    }
    seen.sort();
    seen
}

/// One entry of `claude agents --json`, keeping only what PushOS uses.
///
/// Every field optional: which are present depends on the kind of session and
/// whether it is still running, and a field added or dropped in a later
/// version must not lose the rest.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    pid: Option<u32>,
    session_id: Option<String>,
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    state: Option<String>,
}

/// Reads what `claude agents --json` printed.
fn parse(text: &str) -> Result<Vec<ClaudeSession>, String> {
    let listed: Vec<Listed> = serde_json::from_str(text)
        .map_err(|error| format!("Claude Code's session list did not parse: {error}"))?;

    Ok(listed
        .into_iter()
        .filter_map(|entry| {
            // Only a session whose process is alive has somewhere to be.
            let pid = entry.pid?;
            let activity = activity(entry.status.as_deref(), entry.state.as_deref())?;
            let id = entry.session_id.or(entry.id)?;
            Some(ClaudeSession {
                pid,
                name: entry.name.unwrap_or_else(|| id.clone()),
                id,
                activity,
            })
        })
        .collect())
}

/// What a session's reported status means for a pad.
///
/// `waiting` is anything from an approval to a question to a dialog left open,
/// and every one of them is waiting on a person. A background session with no
/// status reports a state instead; one that has finished is not running.
fn activity(status: Option<&str>, state: Option<&str>) -> Option<Activity> {
    match (status, state) {
        (Some("busy"), _) | (None, Some("working")) => Some(Activity::Working),
        (Some("waiting"), _) | (None, Some("blocked")) => Some(Activity::NeedsDecision),
        (Some("idle"), _) => Some(Activity::Ready),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    /// What `claude agents --json` printed on this Mac, with the paths and
    /// names changed.
    const LISTED: &str = r#"[
      {"pid":1622,"sessionId":"a1","cwd":"/work/one","kind":"interactive","status":"idle","name":"one-b5","startedAt":1},
      {"pid":1929,"sessionId":"b2","cwd":"/work/two","kind":"interactive","status":"busy","name":"two-0a","startedAt":2},
      {"pid":61989,"sessionId":"c3","cwd":"/work/three","kind":"interactive","status":"waiting","waitingFor":"dialog open","name":"three-95","startedAt":3},
      {"id":"bg1","cwd":"/work/four","kind":"background","state":"done","startedAt":4},
      {"pid":7001,"id":"bg2","cwd":"/work/five","kind":"background","state":"blocked","startedAt":5,"someFieldAddedLater":true}
    ]"#;

    #[test]
    fn the_list_becomes_the_sessions_that_are_running() {
        let sessions = parse(LISTED).expect("parses");
        let described: Vec<(u32, &str, Activity)> = sessions
            .iter()
            .map(|session| (session.pid, session.name.as_str(), session.activity))
            .collect();

        assert_eq!(
            described,
            [
                (1622, "one-b5", Activity::Ready),
                (1929, "two-0a", Activity::Working),
                (61989, "three-95", Activity::NeedsDecision),
                (7001, "bg2", Activity::NeedsDecision),
            ],
            "a finished background session is not running"
        );
    }

    #[test]
    fn a_list_that_does_not_parse_says_so() {
        assert!(parse("not json").is_err());
    }

    fn registry(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pushos-claude-registry-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        directory
    }

    fn asked(processes: &FakeProcesses) -> usize {
        processes.spawned().len()
    }

    #[tokio::test(start_paused = true)]
    async fn claude_code_is_asked_again_only_when_a_session_changed() {
        let directory = registry("changes");
        std::fs::write(directory.join("1622.json"), "{}").expect("writable");

        let processes = FakeProcesses::new();
        processes.reply_with(|_| FakeProcesses::printed(LISTED));
        let claude = ClaudeCode::at(Arc::new(processes.clone()), "/bin/claude", &directory);

        assert_eq!(claude.sessions().await.len(), 4);
        claude.sessions().await;
        claude.sessions().await;
        assert_eq!(
            asked(&processes),
            1,
            "nothing changed, so nothing was asked"
        );

        // A session starting writes a file of its own.
        std::fs::write(directory.join("1929.json"), "{}").expect("writable");
        claude.sessions().await;
        assert_eq!(asked(&processes), 2);

        // And a minute on, it asks regardless, for a session that crashed.
        tokio::time::advance(RECONCILE).await;
        claude.sessions().await;
        assert_eq!(asked(&processes), 3);

        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn claude_code_failing_to_answer_is_no_sessions_rather_than_an_error() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(1);
        let directory = registry("failing");
        let claude = ClaudeCode::at(Arc::new(processes.clone()), "/bin/claude", &directory);
        assert!(claude.sessions().await.is_empty());
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn claude_code_is_asked_with_its_supported_command() {
        let processes = FakeProcesses::new();
        processes.reply_with(|_| FakeProcesses::printed("[]"));
        let directory = registry("command");
        let claude = ClaudeCode::at(Arc::new(processes.clone()), "/bin/claude", &directory);
        claude.sessions().await;

        let spawned = processes.spawned();
        assert_eq!(spawned[0].program, "/bin/claude");
        assert_eq!(spawned[0].args, ["agents", "--json"]);
        std::fs::remove_dir_all(directory).ok();
    }
}
