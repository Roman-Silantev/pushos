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
use super::supervised::{self, Supervised};

/// How long an unchanged answer is believed for.
///
/// A session that crashes leaves its file behind unchanged, so the files alone
/// would show it forever. Asking once a minute regardless clears it.
const RECONCILE: Duration = Duration::from_mins(1);

/// How long asking may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// The least time between two asks.
///
/// Asking starts Claude Code, which is a tenth of a second of work and, for a
/// moment, a hundred and fifty megabytes. With a pad each, sessions change
/// state often enough that PushOS would do that every time it looked. Nothing
/// urgent is lost by waiting this long: a session's question reaches the Push
/// through its hook the moment it is asked, not through this.
const ASK_AT_MOST_EVERY: Duration = Duration::from_secs(5);

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

/// Everything Claude Code said in one look.
#[derive(Clone, Debug, Default)]
pub(super) struct Look {
    /// Sessions running in a terminal somewhere.
    pub(super) interactive: Vec<ClaudeSession>,
    /// Sessions Claude Code keeps itself, running or put away.
    pub(super) kept: Vec<Supervised>,
    /// Whether Claude Code actually answered.
    ///
    /// `false` means PushOS does not know what is there, which is a different
    /// thing from knowing there is nothing. Anything that decides a session
    /// has gone must read this first: a Mac mid-update, or one too busy to
    /// answer in time, would otherwise look like one where every session
    /// ended at once.
    pub(super) answered: bool,
}

/// Asks Claude Code about its sessions, and remembers the answer.
#[derive(Debug)]
pub(super) struct ClaudeCode {
    processes: Arc<dyn ProcessRunner>,
    /// Where Claude Code keeps a file for each running session.
    registry: Option<PathBuf>,
    /// Where it writes down the sessions it is keeping itself, and what each
    /// of them is doing. Looked at, never read: what they hold is Claude
    /// Code's business, and their names and times are enough to tell that
    /// something has changed.
    kept: Vec<PathBuf>,
    /// Where Claude Code is, when that is already known. Looked for otherwise.
    program: Option<PathBuf>,
    remembered: Mutex<Remembered>,
    /// Whether a failure to ask has been reported, so it is said once.
    complained: AtomicBool,
}

#[derive(Debug, Default)]
struct Remembered {
    /// The files as they looked when the answer was given.
    files: Option<Vec<(String, SystemTime)>>,
    look: Look,
    asked: Option<Instant>,
}

impl ClaudeCode {
    /// Asks the Claude Code installed for this user.
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            processes,
            registry: registry_directory(),
            kept: kept_paths(),
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
            kept: Vec::new(),
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
        self.look().await.interactive
    }

    /// Every session Claude Code is keeping for PushOS, running or put away.
    pub(super) async fn kept(&self) -> Look {
        self.look().await
    }

    /// Forgets the last answer, so the next look asks Claude Code again.
    ///
    /// For when PushOS has just changed something Claude Code would report:
    /// the answer from a moment ago is about a world that no longer exists.
    pub(super) async fn look_again(&self) {
        let mut remembered = self.remembered.lock().await;
        remembered.asked = None;
        remembered.files = None;
    }

    /// What Claude Code last said, asking again only when something changed.
    async fn look(&self) -> Look {
        let mut remembered = self.remembered.lock().await;
        let files = self.files().await;

        let unchanged = remembered.files.as_ref() == Some(&files)
            && remembered
                .asked
                .is_some_and(|asked| asked.elapsed() < RECONCILE);
        // Asked again only if something changed and the last answer is not
        // seconds old: a busy session rewrites its file constantly, and every
        // rewrite would otherwise be another Claude Code started.
        let too_soon = remembered
            .asked
            .is_some_and(|asked| asked.elapsed() < ASK_AT_MOST_EVERY);
        if unchanged || too_soon {
            return remembered.look.clone();
        }

        match self.ask().await {
            Ok(look) => {
                self.complained.store(false, Ordering::Relaxed);
                remembered.look = look;
            }
            Err(reason) => {
                if !self.complained.swap(true, Ordering::Relaxed) {
                    warn!(%reason, "cannot ask Claude Code what its sessions are doing");
                }
                // What was known before is kept. An answer nobody gave is not
                // an answer of "none", and throwing the last one away would
                // have PushOS conclude that every session had ended.
                remembered.look.answered = false;
            }
        }
        remembered.files = Some(files);
        remembered.asked = Some(Instant::now());
        remembered.look.clone()
    }

    /// The names and times of every file that changes when a session does.
    async fn files(&self) -> Vec<(String, SystemTime)> {
        let mut seen = Vec::new();
        for directory in self.registry.iter().chain(self.kept.iter()) {
            seen.extend(snapshot(directory).await);
        }
        seen.sort();
        seen
    }

    async fn ask(&self) -> Result<Look, String> {
        let Some(program) = self.program() else {
            // Not installed is not worth a warning on every Mac without it.
            self.complained.store(true, Ordering::Relaxed);
            return Ok(Look::default());
        };
        // `--all` so a session that was put away is still listed: it costs
        // nothing, it keeps its conversation, and its pad still stands for it.
        let spec = ProcessSpec::new(
            program.to_string_lossy(),
            ["agents".to_owned(), "--json".to_owned(), "--all".to_owned()],
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
        let mut look = parse(&outcome.stdout_tail)?;
        look.answered = true;
        debug!(
            sessions = look.interactive.len(),
            kept = look.kept.len(),
            "Claude Code described its sessions"
        );
        Ok(look)
    }
}

/// Where Claude Code writes down what the sessions it keeps are doing.
///
/// One directory per session, and a roster of the ones running. Both are
/// inside its configuration directory, beside the registry.
fn kept_paths() -> Vec<PathBuf> {
    let Some(sessions) = registry_directory() else {
        return Vec::new();
    };
    let Some(configuration) = sessions.parent() else {
        return Vec::new();
    };
    vec![configuration.join("jobs"), configuration.join("daemon")]
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
    kind: Option<String>,
    cwd: Option<String>,
    pid: Option<u32>,
    session_id: Option<String>,
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    state: Option<String>,
}

/// Reads what `claude agents --json --all` printed.
///
/// The two kinds are different things to a pad and are kept apart here: one
/// runs in somebody's terminal and can only be watched and typed at, the other
/// Claude Code keeps for PushOS and can be put away and asked for again.
fn parse(text: &str) -> Result<Look, String> {
    let listed: Vec<Listed> = serde_json::from_str(text)
        .map_err(|error| format!("Claude Code's session list did not parse: {error}"))?;

    let mut look = Look::default();
    for entry in listed {
        if entry.kind.as_deref() == Some("background") {
            if let Some(kept) = supervised_from(entry) {
                look.kept.push(kept);
            }
            continue;
        }
        // Only a session whose process is alive has somewhere to be.
        let Some(pid) = entry.pid else { continue };
        let Some(activity) = activity(entry.status.as_deref(), entry.state.as_deref()) else {
            continue;
        };
        let Some(id) = entry.session_id.or(entry.id) else {
            continue;
        };
        look.interactive.push(ClaudeSession {
            pid,
            name: entry.name.unwrap_or_else(|| id.clone()),
            id,
            activity,
        });
    }
    Ok(look)
}

/// One listed session Claude Code is keeping.
fn supervised_from(entry: Listed) -> Option<Supervised> {
    let id = entry.id?;
    let running = entry.pid.is_some();
    Some(Supervised {
        activity: supervised::activity_of(entry.state.as_deref(), running),
        name: entry.name.unwrap_or_else(|| id.clone()),
        session: entry.session_id.unwrap_or_else(|| id.clone()),
        directory: entry.cwd.map(PathBuf::from).unwrap_or_default(),
        id,
        running,
    })
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
    fn the_list_becomes_the_sessions_running_in_somebody_s_terminal() {
        let look = parse(LISTED).expect("parses");
        let described: Vec<(u32, &str, Activity)> = look
            .interactive
            .iter()
            .map(|session| (session.pid, session.name.as_str(), session.activity))
            .collect();

        assert_eq!(
            described,
            [
                (1622, "one-b5", Activity::Ready),
                (1929, "two-0a", Activity::Working),
                (61989, "three-95", Activity::NeedsDecision),
            ],
            "a session Claude Code keeps is not one of these"
        );
    }

    #[test]
    fn the_sessions_claude_code_keeps_come_back_whether_or_not_they_are_running() {
        let look = parse(LISTED).expect("parses");
        let described: Vec<(&str, bool, Activity)> = look
            .kept
            .iter()
            .map(|kept| (kept.id.as_str(), kept.running, kept.activity))
            .collect();

        assert_eq!(
            described,
            [
                ("bg1", false, Activity::Quiet),
                ("bg2", true, Activity::NeedsDecision),
            ],
            "one put away still belongs on a pad, and costs nothing"
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
    async fn claude_code_is_not_asked_twice_in_the_same_few_seconds() {
        // A session that is working rewrites its file constantly, and every
        // ask is another Claude Code started for a tenth of a second.
        let directory = registry("often");
        std::fs::write(directory.join("1622.json"), "{}").expect("writable");

        let processes = FakeProcesses::new();
        processes.reply_with(|_| FakeProcesses::printed(LISTED));
        let claude = ClaudeCode::at(Arc::new(processes.clone()), "/bin/claude", &directory);
        claude.sessions().await;

        for change in 0..5 {
            std::fs::write(directory.join(format!("{change}.json")), "{}").expect("writable");
            claude.sessions().await;
        }
        assert_eq!(asked(&processes), 1, "five changes, one ask");

        tokio::time::advance(ASK_AT_MOST_EVERY).await;
        std::fs::write(directory.join("later.json"), "{}").expect("writable");
        claude.sessions().await;
        assert_eq!(asked(&processes), 2, "and it does catch up");

        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test(start_paused = true)]
    async fn claude_code_is_asked_again_only_when_a_session_changed() {
        let directory = registry("changes");
        std::fs::write(directory.join("1622.json"), "{}").expect("writable");

        let processes = FakeProcesses::new();
        processes.reply_with(|_| FakeProcesses::printed(LISTED));
        let claude = ClaudeCode::at(Arc::new(processes.clone()), "/bin/claude", &directory);

        assert_eq!(claude.sessions().await.len(), 3);
        claude.sessions().await;
        claude.sessions().await;
        assert_eq!(
            asked(&processes),
            1,
            "nothing changed, so nothing was asked"
        );

        // A session starting writes a file of its own.
        tokio::time::advance(ASK_AT_MOST_EVERY).await;
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
        assert_eq!(spawned[0].args, ["agents", "--json", "--all"]);
        std::fs::remove_dir_all(directory).ok();
    }
}
