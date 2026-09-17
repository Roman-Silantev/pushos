//! Threads Codex keeps for PushOS inside one app server.
//!
//! Codex holds many conversations in a single process, which is what makes a
//! pad per thread affordable: measured on an M4, a server holding a thread is
//! around 20 MB in total, where a Claude Code session is a few hundred
//! megabytes of its own. A thread nobody is using is unloaded by the server
//! itself a minute after PushOS stops subscribing to it, and asking for it
//! again loads it back.
//!
//! A thread is also nameable, so a seat's name is the thread's name, and an
//! operator sees the same name in Codex's own listing.

mod rpc;

use std::path::{Path, PathBuf};

use pushos_domain::attached::Activity;
use pushos_domain::ports::AttachError;
use serde_json::{Value, json};
use tracing::debug;

use self::rpc::AppServer;

/// How many threads are asked about in one listing.
///
/// PushOS shows only the threads its seats hold, and matches them out of this
/// page. Generous enough that a seat used today is always in it, bounded so a
/// listing can never become the largest thing PushOS holds.
const LISTED: u64 = 200;

/// One thread Codex is keeping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Thread {
    /// What Codex calls it, and what `codex resume` takes.
    pub(super) id: String,
    /// The name it was given, which is its seat's name.
    pub(super) name: String,
    /// What it appears to be doing.
    pub(super) activity: Activity,
    /// Whether the server is holding it in memory now.
    pub(super) loaded: bool,
}

/// Everything PushOS does with Codex threads.
#[derive(Debug)]
pub(super) struct CodexThreads {
    server: AppServer,
}

impl CodexThreads {
    /// Speaks to the Codex installed for this user.
    ///
    /// `lean` leaves out what a thread on a pad does not use, which is most of
    /// what a thread costs; see [`rpc`].
    pub(super) fn new(lean: bool) -> Self {
        Self {
            server: AppServer::new(lean),
        }
    }

    /// Speaks to a particular program that serves the same protocol.
    #[cfg(test)]
    pub(super) fn served_by(program: impl Into<PathBuf>, arguments: &[&str]) -> Self {
        Self {
            server: AppServer::running(program, arguments),
        }
    }

    /// Where Codex is installed, if it is.
    pub(super) fn program(&self) -> Option<PathBuf> {
        self.server.program()
    }

    /// The threads Codex knows about, newest first.
    ///
    /// Never a failure: a Mac without Codex, or a server that would not start,
    /// simply has no threads to put on the surface.
    pub(super) async fn threads(&self) -> Vec<Thread> {
        match self.ask_for_threads().await {
            Ok(threads) => threads,
            Err(error) => {
                debug!(%error, "Codex did not say what its threads are doing");
                Vec::new()
            }
        }
    }

    async fn ask_for_threads(&self) -> Result<Vec<Thread>, AttachError> {
        // From what the server has written down rather than by reading every
        // conversation on disk: PushOS wants names and states, not history.
        let listed = self
            .server
            .call(
                "thread/list",
                json!({"limit": LISTED, "useStateDbOnly": true}),
            )
            .await?;
        // What the server is holding now. A thread it started this minute is
        // here and not yet in the listing, which is written when the thread
        // has something in it; without this, a seat just started would show
        // as nothing at all.
        let held = self.server.call("thread/loaded/list", json!({})).await?;
        let held: Vec<&str> = held
            .get("data")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        let mut threads: Vec<Thread> = listed
            .get("data")
            .and_then(Value::as_array)
            .map(|threads| threads.iter().filter_map(read_thread).collect())
            .unwrap_or_default();

        for thread in &mut threads {
            if held.contains(&thread.id.as_str()) {
                thread.loaded = true;
                if thread.activity == Activity::Quiet {
                    thread.activity = Activity::Ready;
                }
            }
        }
        for id in held {
            if threads.iter().any(|thread| thread.id == id) {
                continue;
            }
            // Asked for by name, because the listing has nothing to say about
            // a thread this new. Only ever the handful the server is holding
            // that the listing has not caught up with, which is normally none.
            let read = self
                .server
                .call(
                    "thread/read",
                    json!({"threadId": id, "includeTurns": false}),
                )
                .await
                .ok()
                .and_then(|answer| read_thread(answer.get("thread")?));
            threads.push(read.map_or_else(
                || Thread {
                    id: id.to_owned(),
                    name: id.to_owned(),
                    activity: Activity::Ready,
                    loaded: true,
                },
                |mut thread| {
                    thread.loaded = true;
                    if thread.activity == Activity::Quiet {
                        thread.activity = Activity::Ready;
                    }
                    thread
                },
            ));
        }
        Ok(threads)
    }

    /// Starts a thread under a name, on the work it is for.
    pub(super) async fn start(
        &self,
        directory: &Path,
        name: &str,
        work: &str,
    ) -> Result<String, AttachError> {
        let started = self
            .server
            .call("thread/start", json!({"cwd": directory.to_string_lossy()}))
            .await?;
        let id = started
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AttachError::unavailable("Codex started a thread but did not say which")
            })?
            .to_owned();

        // Named before it is given work, so it is never briefly nameless on
        // the surface, and so Codex's own listing says whose it is.
        let _ = self
            .server
            .call("thread/name/set", json!({"threadId": id, "name": name}))
            .await;
        self.instruct(&id, work).await?;

        debug!(thread = %id, name, directory = %directory.display(), "started a Codex thread");
        Ok(id)
    }

    /// Gives a thread its next instruction, loading it again if it was unloaded.
    pub(super) async fn instruct(&self, thread: &str, text: &str) -> Result<(), AttachError> {
        self.server
            .call(
                "turn/start",
                json!({"threadId": thread, "input": [{"type": "text", "text": text}]}),
            )
            .await
            .map(drop)
    }

    /// Stops following a thread, so the server may give its memory back.
    ///
    /// The server unloads it a minute later if nobody else wants it, which is
    /// its own rule rather than one PushOS imposes.
    pub(super) async fn put_away(&self, thread: &str) -> Result<(), AttachError> {
        self.server
            .call("thread/unsubscribe", json!({"threadId": thread}))
            .await
            .map(drop)
    }

    /// The last of what a thread has said, as text for the display.
    ///
    /// Its items rather than a screen: a thread has no terminal, so what it
    /// said is all there is to show.
    pub(super) async fn read(&self, thread: &str, most: usize) -> Result<String, AttachError> {
        let listed = self
            .server
            .call(
                "thread/items/list",
                json!({"threadId": thread, "limit": SHOWN_ITEMS, "sortDirection": "desc"}),
            )
            .await?;

        let mut said: Vec<String> = listed
            .get("data")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(spoken).collect())
            .unwrap_or_default();
        said.reverse();

        let text = said.join("\n");
        let from = text.len().saturating_sub(most);
        Ok(text[text.floor_char_boundary(from)..].to_owned())
    }

    /// What to run in a window to take one over.
    pub(super) fn attach_command(&self, thread: &str) -> Option<Vec<String>> {
        let program = self.program()?;
        Some(vec![
            program.to_string_lossy().into_owned(),
            "resume".to_owned(),
            thread.to_owned(),
        ])
    }
}

/// How many of a thread's items are read for the display.
///
/// The display is 160 pixels tall, so this is already more than it can show;
/// asking for the whole conversation would be asking for megabytes.
const SHOWN_ITEMS: u64 = 12;

/// What one item of a thread says, when it says anything printable.
fn spoken(item: &Value) -> Option<String> {
    let text = item
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| item.get("message").and_then(Value::as_str))
        .or_else(|| item.get("command").and_then(Value::as_str))?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// One listed thread, as far as a pad is concerned.
fn read_thread(listed: &Value) -> Option<Thread> {
    let id = listed.get("id").and_then(Value::as_str)?.to_owned();
    let status = listed
        .get("status")
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str);
    Some(Thread {
        name: listed
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map_or_else(
                || {
                    listed
                        .get("preview")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned()
                },
                ToOwned::to_owned,
            ),
        activity: activity_of(status),
        loaded: status.is_some_and(|status| status != "notLoaded"),
        id,
    })
}

/// What a thread's status means for a pad.
fn activity_of(status: Option<&str>) -> Activity {
    match status {
        Some("active") => Activity::Working,
        // Something went wrong in it that nobody has looked at.
        Some("systemError") => Activity::NeedsDecision,
        Some("idle") => Activity::Ready,
        // Not loaded, or a status a later Codex invented: nothing is running
        // for it, and nothing is being asked of anyone.
        _ => Activity::Quiet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_thread_becomes_what_a_pad_shows() {
        let listed = json!({
            "id": "01a0a897-55d0-7ba0-a293-a22dae38c179",
            "name": "invoices",
            "preview": "review the invoices module",
            "status": {"type": "active"}
        });
        let thread = read_thread(&listed).expect("a thread");
        assert_eq!(thread.name, "invoices");
        assert_eq!(thread.activity, Activity::Working);
        assert!(thread.loaded);
    }

    #[test]
    fn a_thread_with_no_name_is_shown_by_what_it_was_asked_to_do() {
        let listed = json!({
            "id": "abc",
            "name": null,
            "preview": "review the invoices module",
            "status": {"type": "idle"}
        });
        let thread = read_thread(&listed).expect("a thread");
        assert_eq!(thread.name, "review the invoices module");
        assert_eq!(thread.activity, Activity::Ready);
    }

    #[test]
    fn a_thread_the_server_is_not_holding_costs_nothing_and_says_so() {
        let listed = json!({"id": "abc", "status": {"type": "notLoaded"}});
        let thread = read_thread(&listed).expect("a thread");
        assert_eq!(thread.activity, Activity::Quiet);
        assert!(!thread.loaded, "nothing is running for it");
        assert_eq!(thread.name, "abc", "with nothing else to call it by");
    }

    #[test]
    fn what_each_status_means() {
        assert_eq!(activity_of(Some("active")), Activity::Working);
        assert_eq!(activity_of(Some("idle")), Activity::Ready);
        assert_eq!(activity_of(Some("systemError")), Activity::NeedsDecision);
        assert_eq!(activity_of(Some("notLoaded")), Activity::Quiet);
        assert_eq!(activity_of(Some("invented-later")), Activity::Quiet);
        assert_eq!(activity_of(None), Activity::Quiet);
    }

    #[test]
    fn what_a_thread_said_is_read_from_its_items() {
        assert_eq!(
            spoken(&json!({"text": "  ready  "})).as_deref(),
            Some("ready")
        );
        assert_eq!(
            spoken(&json!({"command": "cargo test"})).as_deref(),
            Some("cargo test")
        );
        assert_eq!(spoken(&json!({"text": "   "})), None, "nothing to show");
        assert_eq!(spoken(&json!({"kind": "image"})), None);
    }

    /// A stand-in app server: it answers the handful of methods PushOS uses,
    /// in the shapes Codex answers them in, and writes down what it was asked.
    fn stub(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "pushos-codex-{label}-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        let script = directory.join("server.py");
        let asked = directory.join("asked.jsonl");
        std::fs::write(
            &script,
            format!(
                r#"import json, sys
asked = open({asked:?}, "a")
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps(request), file=asked, flush=True)
    method = request["method"]
    if method == "thread/start":
        result = {{"thread": {{"id": "thread-1"}}}}
    elif method == "thread/list":
        result = {{"data": [
            {{"id": "thread-1", "name": "invoices", "status": {{"type": "active"}}}},
            {{"id": "thread-2", "name": "old", "status": {{"type": "notLoaded"}}}}
        ]}}
    elif method == "thread/items/list":
        result = {{"data": [{{"text": "second"}}, {{"text": "first"}}]}}
    else:
        result = {{}}
    print(json.dumps({{"jsonrpc": "2.0", "id": request["id"], "result": result}}), flush=True)
"#,
                asked = asked.to_string_lossy()
            ),
        )
        .expect("writable");
        (directory, script, asked)
    }

    fn requests(asked: &Path) -> Vec<Value> {
        std::fs::read_to_string(asked)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[tokio::test]
    async fn the_threads_a_server_lists_become_what_the_pads_show() {
        let (directory, script, _asked) = stub("list");
        let codex = CodexThreads::served_by("/usr/bin/python3", &[&script.to_string_lossy()]);

        let threads = codex.threads().await;

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].name, "invoices");
        assert_eq!(threads[0].activity, Activity::Working);
        assert!(threads[0].loaded);
        assert_eq!(threads[1].activity, Activity::Quiet, "nobody is holding it");
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn starting_a_seat_names_its_thread_and_gives_it_the_work() {
        let (directory, script, asked) = stub("start");
        let codex = CodexThreads::served_by("/usr/bin/python3", &[&script.to_string_lossy()]);

        let id = codex
            .start(Path::new("/tmp/work"), "invoices", "review the invoices")
            .await
            .expect("started");

        assert_eq!(id, "thread-1");
        let sent = requests(&asked);
        let methods: Vec<&str> = sent
            .iter()
            .filter_map(|request| request.get("method").and_then(Value::as_str))
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "thread/start",
                "thread/name/set",
                "turn/start"
            ],
            "one hello, then the work"
        );
        let named = &sent[2]["params"];
        assert_eq!(named["name"], "invoices");
        assert_eq!(named["threadId"], "thread-1");
        let turn = &sent[3]["params"];
        assert_eq!(turn["input"][0]["text"], "review the invoices");
        assert_eq!(sent[1]["params"]["cwd"], "/tmp/work");
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn putting_a_thread_away_asks_the_server_to_stop_holding_it() {
        let (directory, script, asked) = stub("away");
        let codex = CodexThreads::served_by("/usr/bin/python3", &[&script.to_string_lossy()]);

        codex.put_away("thread-1").await.expect("put away");

        let sent = requests(&asked);
        let last = sent.last().expect("something was asked");
        assert_eq!(last["method"], "thread/unsubscribe");
        assert_eq!(last["params"]["threadId"], "thread-1");
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn what_a_thread_said_comes_back_oldest_first() {
        let (directory, script, _asked) = stub("read");
        let codex = CodexThreads::served_by("/usr/bin/python3", &[&script.to_string_lossy()]);

        let said = codex.read("thread-1", 1_000).await.expect("read");

        assert_eq!(said, "first\nsecond", "the server lists newest first");
        std::fs::remove_dir_all(directory).ok();
    }

    /// Against the Codex installed on this Mac, rather than a stand-in.
    ///
    /// Ignored, because it needs Codex, a network and an account, and it
    /// starts a thread that does a little work. Run it by hand after changing
    /// anything here:
    ///
    /// ```text
    /// cargo test -p pushos-macos -- --ignored against_the_codex_installed
    /// ```
    #[tokio::test]
    #[ignore = "needs the real Codex, and spends a little of its allowance"]
    async fn against_the_codex_installed_on_this_mac() {
        let codex = CodexThreads::new(true);
        assert!(codex.program().is_some(), "Codex is not installed");

        let directory = std::env::temp_dir().join(format!(
            "pushos-codex-live-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");

        let id = codex
            .start(&directory, "pushos-live-check", "Reply with exactly: ok")
            .await
            .expect("started a thread");

        let mine = codex
            .threads()
            .await
            .into_iter()
            .find(|thread| thread.id == id)
            .expect("the thread it just started is listed");
        assert_eq!(mine.name, "pushos-live-check", "named after its seat");
        assert!(mine.loaded, "the server is holding it");

        codex.put_away(&id).await.expect("put away");
        std::fs::remove_dir_all(&directory).ok();
        println!("started, named, listed and put away {id}");
    }

    #[tokio::test]
    async fn a_mac_without_codex_has_no_threads_rather_than_a_failure() {
        let codex = CodexThreads::served_by("/definitely/not/a/program", &[]);
        assert!(codex.threads().await.is_empty());
    }

    #[test]
    fn a_listing_entry_with_no_identifier_is_no_thread() {
        assert!(read_thread(&json!({"name": "nameless"})).is_none());
    }
}
