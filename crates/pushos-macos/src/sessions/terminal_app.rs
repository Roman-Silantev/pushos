//! Watching the terminals the operator already had open in Terminal.
//!
//! Terminal is scriptable, and every tab reports the device it is attached to
//! and what it calls itself. That is enough to put eight coding sessions on
//! eight pads without closing any of them.
//!
//! PushOS does not own these. It cannot know their exit status, it is not told
//! when one closes, and what it reads is what is on screen rather than a
//! stream. Every script here is fixed text with values passed as arguments, so
//! nothing an operator or a session titles itself can change what runs.

use std::sync::Arc;

use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachError, Key, ProcessRunner};
use tracing::warn;

use crate::script::ScriptRunner;

/// The application PushOS watches.
pub(super) const APPLICATION: &str = "Terminal";

/// Separates the fields of one tab in a reply.
///
/// A unit separator cannot appear in a device name or a window title, so the
/// reply can be split without ambiguity however the title is written.
const FIELD: char = '\u{1f}';

/// Separates one tab from the next.
const RECORD: char = '\u{1e}';

/// Lists every tab, with what it is called and the last of what is on it.
///
/// One script for all of them rather than one per tab: asking costs a
/// subprocess and this runs every few seconds, so eight windows must not mean
/// nine processes. Nothing is asked of Terminal when it is not running, because
/// asking would start it, and an operator who quit it wants it quit.
const DISCOVER: &str = r#"on run argv
  set most to (item 1 of argv) as integer
  if application "Terminal" is not running then return ""
  set out to ""
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        try
          set seen to ""
          try
            set h to history of t
            if (count of h) > most then
              set seen to text -most thru -1 of h
            else
              set seen to h
            end if
          end try
          set out to out & ((tty of t) as text) & (ASCII character 31) & ((custom title of t) as text) & (ASCII character 31) & seen & (ASCII character 30)
        end try
      end repeat
    end repeat
  end tell
  return out
end run"#;

/// How much of each screen to read while looking at what is open.
///
/// Enough to hold the prompt and a question above it, and no more. This is
/// read for every window at once, and a reply that outgrows what a subprocess
/// can hand back is truncated from the front, which loses whole windows. The
/// focused view asks for its own screen, one window at a time, where there is
/// room to spare.
const GLANCE: usize = 220;

/// What a terminal device path begins with.
///
/// Checked rather than assumed. A reply too long to hand back is cut from the
/// front, and the surviving fragment starts in the middle of somebody's screen;
/// without this it would become a session named after whatever it landed on.
const DEVICE_PREFIX: &str = "/dev/";

/// Returns the tail of what one tab has on screen.
const READ: &str = r#"on run argv
  set wanted to item 1 of argv
  set most to (item 2 of argv) as integer
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if ((tty of t) as text) is wanted then
          set h to history of t
          if (count of h) > most then return text -most thru -1 of h
          return h
        end if
      end repeat
    end repeat
  end tell
  return ""
end run"#;

/// Types into one tab.
const SEND: &str = r#"on run argv
  set wanted to item 1 of argv
  set what to item 2 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if ((tty of t) as text) is wanted then
          do script what in t
          return "sent"
        end if
      end repeat
    end repeat
  end tell
  return ""
end run"#;

/// Presses one key in a tab, after bringing its window to the front.
///
/// A key press goes to whatever is in front, so the window is brought forward
/// first. That is visible and deliberate: the operator sees which session they
/// are pressing a key in.
const PRESS: &str = r#"on run argv
  set wanted to item 1 of argv
  set code to (item 2 of argv) as integer
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if ((tty of t) as text) is wanted then
          set selected tab of w to t
          set index of w to 1
          activate
          tell application "System Events" to key code code
          return "pressed"
        end if
      end repeat
    end repeat
  end tell
  return ""
end run"#;

/// Brings one tab's window to the front.
const FOCUS: &str = r#"on run argv
  set wanted to item 1 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if ((tty of t) as text) is wanted then
          set selected tab of w to t
          set index of w to 1
          activate
          return "focused"
        end if
      end repeat
    end repeat
  end tell
  return ""
end run"#;

/// Opens a new window running one command line.
///
/// The line is built by PushOS from a program's path and a validated name,
/// each quoted for the shell, and passed in as an argument like everything
/// else here.
const OPEN: &str = r#"on run argv
  set command to item 1 of argv
  tell application "Terminal"
    do script command
    activate
  end tell
  return "opened"
end run"#;

/// One tab, as Terminal described it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tab {
    /// The device it is attached to.
    pub(super) device: String,
    /// What it calls itself.
    pub(super) title: String,
    /// The last of what is on it.
    pub(super) screen: String,
}

/// The terminals the operator already had open.
#[derive(Debug, Clone)]
pub(super) struct TerminalApp {
    scripts: ScriptRunner,
}

impl TerminalApp {
    /// Builds a watcher over Terminal.
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            scripts: ScriptRunner::new(processes),
        }
    }

    /// Runs one of the fixed scripts, classifying a refusal.
    async fn ask(&self, script: &str, arguments: &[String]) -> Result<String, AttachError> {
        self.scripts
            .run(script, arguments)
            .await
            .map_err(into_attach_error)
    }

    /// Every tab, in order of device.
    pub(super) async fn tabs(&self) -> Result<Vec<Tab>, AttachError> {
        let reply = self.ask(DISCOVER, &[GLANCE.to_string()]).await?;

        let records = reply.split(RECORD).filter(|r| !r.trim().is_empty()).count();
        let mut found: Vec<Tab> = reply.split(RECORD).filter_map(parse).collect();

        // By device, which is the one thing a window cannot change about
        // itself. Terminal answers in whatever order it likes, and a pad that
        // meant a different session each time it was polled would be worse
        // than no pad at all.
        found.sort_by(|left, right| left.device.cmp(&right.device));

        // A reply too long to hand back is cut from the front, which loses
        // whole windows silently. Saying so beats an operator counting seven
        // pads where there are eight.
        if found.len() < records {
            warn!(
                kept = found.len(),
                records, "some terminals could not be read; the reply was too long"
            );
        }
        Ok(found)
    }

    /// The last of what one tab has on screen.
    pub(super) async fn read(&self, device: &str, most: usize) -> Result<String, AttachError> {
        let reply = self
            .ask(READ, &[device.to_owned(), most.max(1).to_string()])
            .await?;
        found(reply, device)
    }

    /// Types a line into one tab.
    pub(super) async fn send(&self, device: &str, text: &str) -> Result<(), AttachError> {
        let reply = self
            .ask(SEND, &[device.to_owned(), text.to_owned()])
            .await?;
        found(reply, device).map(drop)
    }

    /// Presses a key in one tab, bringing it to the front.
    pub(super) async fn press(&self, device: &str, key: Key) -> Result<(), AttachError> {
        let reply = self
            .ask(PRESS, &[device.to_owned(), key_code(key).to_string()])
            .await?;
        found(reply, device).map(drop)
    }

    /// Brings one tab to the front.
    pub(super) async fn focus(&self, device: &str) -> Result<(), AttachError> {
        let reply = self.ask(FOCUS, &[device.to_owned()]).await?;
        found(reply, device).map(drop)
    }

    /// Opens a new window running `arguments` as one command.
    pub(super) async fn open_window(&self, arguments: &[String]) -> Result<(), AttachError> {
        let line = arguments
            .iter()
            .map(|argument| quoted(argument))
            .collect::<Vec<_>>()
            .join(" ");
        // `exec`, so the window is the command: leaving it leaves no idle shell
        // behind, and the session it showed carries on without it.
        self.ask(OPEN, &[format!("exec {line}")]).await.map(drop)
    }
}

/// An empty reply means the script looked and found no such tab.
fn found(reply: String, device: &str) -> Result<String, AttachError> {
    if reply.is_empty() {
        return Err(AttachError::Gone {
            session: AttachedId::new(device),
        });
    }
    Ok(reply)
}

/// Reads one tab out of a reply.
///
/// A record missing its fields is skipped rather than failing the listing: one
/// tab PushOS cannot read should not hide the other seven.
fn parse(record: &str) -> Option<Tab> {
    let mut fields = record.trim().split(FIELD);
    let device = fields.next()?.trim();
    if !device.starts_with(DEVICE_PREFIX) {
        return None;
    }

    let title = fields.next().unwrap_or_default().trim();
    // The rest of the record is the screen, which may contain anything at all
    // including the field separator if a program drew one, so it is whatever
    // is left rather than the next field.
    let screen: String = fields.collect::<Vec<_>>().join(&FIELD.to_string());

    Some(Tab {
        device: device.to_owned(),
        title: title.to_owned(),
        screen,
    })
}

/// Quotes one argument for the shell a new window runs.
///
/// Single quotes, inside which a shell interprets nothing, with any single
/// quote in the argument closed, escaped and reopened.
fn quoted(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', r"'\''"))
}

/// The virtual key code macOS uses for a key.
///
/// Fixed numbers rather than characters, because `key code` is what presses a
/// key rather than typing one, and Tab typed as a character is not Tab.
const fn key_code(key: Key) -> u8 {
    match key {
        Key::Tab => 48,
        Key::Enter => 36,
        Key::Escape => 53,
        Key::Up => 126,
        Key::Down => 125,
    }
}

/// Turns a script failure into something an operator can act on.
fn into_attach_error(error: ActionError) -> AttachError {
    if error.class() == ErrorClass::Permission {
        return AttachError::NotPermitted {
            application: APPLICATION.to_owned(),
        };
    }
    let class = error.class();
    AttachError::backend("asking Terminal what is open", class, error)
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    fn terminal(processes: &FakeProcesses) -> TerminalApp {
        TerminalApp::new(Arc::new(processes.clone()))
    }

    fn record(device: &str, title: &str, screen: &str) -> String {
        format!("{device}{FIELD}{title}{FIELD}{screen}{RECORD}")
    }

    #[test]
    fn a_reply_becomes_the_tabs_it_describes() {
        let reply = format!(
            "{}{}",
            record("/dev/ttys003", "✳ Sprint 2 setup", "❯ "),
            record("/dev/ttys004", "◐ Push OS system", "Proceed?\n❯ 1. Yes")
        );
        let found: Vec<Tab> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].device, "/dev/ttys003");
        assert_eq!(found[0].title, "✳ Sprint 2 setup");
        assert_eq!(found[1].screen, "Proceed?\n❯ 1. Yes");
    }

    #[test]
    fn a_screen_containing_the_separator_does_not_become_another_tab() {
        // A program may draw anything, including the character used to split
        // the fields. The screen is whatever is left, not the next field.
        let odd = format!("some output{FIELD}more output");
        let reply = record("/dev/ttys003", "✳ Odd", &odd);
        let found: Vec<Tab> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].screen, odd);
    }

    #[test]
    fn a_title_containing_anything_at_all_is_still_one_field() {
        // Titles are whatever a program set them to. A tab called "a, b: c"
        // must not become three sessions.
        let reply = record("/dev/ttys003", "a, b: c | d\ttab", "");
        let found: Vec<Tab> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "a, b: c | d\ttab");
    }

    #[test]
    fn a_record_with_no_device_is_skipped_rather_than_failing_the_listing() {
        // One tab PushOS cannot read should not hide the other seven.
        let reply = format!(
            "{}{}",
            record("", "broken", ""),
            record("/dev/ttys003", "fine", "")
        );
        let found: Vec<Tab> = reply.split(RECORD).filter_map(parse).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "fine");
    }

    #[test]
    fn looking_never_starts_terminal() {
        // An operator who quit Terminal wants it quit, and a question asked of
        // it every three seconds would open it again.
        assert!(DISCOVER.contains("is not running then return"));
    }

    #[tokio::test]
    async fn discovery_runs_a_fixed_script_with_no_arguments() {
        // Nothing a session titles itself can change what runs.
        let processes = FakeProcesses::new();
        terminal(&processes).tabs().await.ok();

        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].program, "/usr/bin/osascript");
        assert_eq!(
            spawned[0].args.len(),
            3,
            "the script, and how much of each screen to read"
        );
    }

    #[tokio::test]
    async fn what_is_typed_is_passed_as_an_argument_never_spliced_into_the_script() {
        let processes = FakeProcesses::new();
        let _ = terminal(&processes)
            .send("/dev/ttys003", "\" & (do shell script \"id\") & \"")
            .await;

        let spawned = processes.spawned();
        let script = &spawned[0].args[1];
        assert!(
            !script.contains("do shell script"),
            "the script must be fixed text: {script}"
        );
        assert_eq!(spawned[0].args[3], "\" & (do shell script \"id\") & \"");
    }

    #[tokio::test]
    async fn a_key_is_pressed_by_its_code_not_typed_as_a_character() {
        // Tab typed as a character is not Tab, and a suggestion would never be
        // accepted.
        let processes = FakeProcesses::new();
        let _ = terminal(&processes).press("/dev/ttys003", Key::Tab).await;

        let spawned = processes.spawned();
        assert!(
            spawned[0].args[1].contains("key code"),
            "a key must be pressed rather than typed"
        );
        assert_eq!(spawned[0].args[3], "48", "the code for tab");
    }

    #[test]
    fn every_key_a_pad_can_press_has_a_code_of_its_own() {
        let mut seen: Vec<u8> = Key::ALL.into_iter().map(key_code).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), Key::ALL.len(), "two keys share a code");
    }

    #[tokio::test]
    async fn a_new_window_runs_each_argument_quoted_for_the_shell() {
        let processes = FakeProcesses::new();
        terminal(&processes)
            .open_window(&[
                "/opt/home brew/bin/tmux".to_owned(),
                "attach-session".to_owned(),
                "-t".to_owned(),
                "=it's; rm -rf ~".to_owned(),
            ])
            .await
            .expect("the fake succeeds");

        let spawned = processes.spawned();
        assert_eq!(
            spawned[0].args[2],
            r"exec '/opt/home brew/bin/tmux' 'attach-session' '-t' '=it'\''s; rm -rf ~'"
        );
    }

    #[test]
    fn quoting_leaves_nothing_for_a_shell_to_interpret() {
        assert_eq!(quoted("plain"), "'plain'");
        assert_eq!(quoted("$(id) `id` \"x\""), "'$(id) `id` \"x\"'");
        assert_eq!(quoted("it's"), r"'it'\''s'");
    }

    #[tokio::test]
    async fn a_tab_that_has_gone_says_so_rather_than_failing() {
        // A window the operator closed is a fact, not a fault.
        let processes = FakeProcesses::new();
        let error = terminal(&processes)
            .read("/dev/ttys003", 200)
            .await
            .expect_err("the fake replies with nothing");
        assert!(matches!(error, AttachError::Gone { .. }));
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_refused_automation_permission_says_what_to_allow() {
        let processes = FakeProcesses::new();
        processes.fail_with(ErrorClass::Permission);

        let error = terminal(&processes)
            .tabs()
            .await
            .expect_err("permission was refused");
        assert!(matches!(error, AttachError::NotPermitted { .. }));
        assert!(error.to_string().contains("Automation"), "{error}");
    }
}
