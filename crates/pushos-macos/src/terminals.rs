//! Watching the terminals the operator already had open.
//!
//! Terminal.app is scriptable, and every tab reports the device it is attached
//! to, whether something is running in it, and what it calls itself. That is
//! enough to put eight coding sessions on eight pads without closing any of
//! them.
//!
//! PushOS does not own these. It cannot know their exit status, it is not told
//! when one closes, and what it reads is what is on screen rather than a
//! stream. Every script here is fixed text with values passed as arguments, so
//! nothing an operator or a session titles itself can change what runs.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::attached::{Attached, activity_of};
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachError, AttachedSessions, Key, ProcessRunner};
use tracing::{debug, warn};

use crate::script::ScriptRunner;

/// The application PushOS watches.
const APPLICATION: &str = "Terminal";

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
/// nine processes.
const DISCOVER: &str = r#"on run argv
  set most to (item 1 of argv) as integer
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
          set out to out & ((tty of t) as text) & (ASCII character 31) & ((busy of t) as text) & (ASCII character 31) & ((custom title of t) as text) & (ASCII character 31) & seen & (ASCII character 30)
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

/// The terminals the operator already had open.
#[derive(Debug, Clone)]
pub struct TerminalAppSessions {
    scripts: ScriptRunner,
}

impl TerminalAppSessions {
    /// Builds a watcher over Terminal.app.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
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
}

#[async_trait]
impl AttachedSessions for TerminalAppSessions {
    async fn discover(&self) -> Result<Vec<Attached>, AttachError> {
        let reply = self.ask(DISCOVER, &[GLANCE.to_string()]).await?;

        let records = reply.split(RECORD).filter(|r| !r.trim().is_empty()).count();
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();

        // A reply too long to hand back is cut from the front, which loses
        // whole windows silently. Saying so beats an operator counting seven
        // pads where there are eight.
        if found.len() < records {
            warn!(
                kept = found.len(),
                records, "some terminals could not be read; the reply was too long"
            );
        }

        debug!(sessions = found.len(), "found terminals already open");
        Ok(found)
    }

    async fn read(&self, session: &AttachedId, most: usize) -> Result<String, AttachError> {
        let reply = self
            .ask(READ, &[session.to_string(), most.max(1).to_string()])
            .await?;
        if reply.is_empty() {
            return Err(AttachError::Gone {
                session: session.clone(),
            });
        }
        Ok(reply)
    }

    async fn send(&self, session: &AttachedId, text: &str) -> Result<(), AttachError> {
        let reply = self
            .ask(SEND, &[session.to_string(), text.to_owned()])
            .await?;
        if reply.is_empty() {
            return Err(AttachError::Gone {
                session: session.clone(),
            });
        }
        Ok(())
    }

    async fn press(&self, session: &AttachedId, key: Key) -> Result<(), AttachError> {
        let reply = self
            .ask(PRESS, &[session.to_string(), key_code(key).to_string()])
            .await?;
        if reply.is_empty() {
            return Err(AttachError::Gone {
                session: session.clone(),
            });
        }
        Ok(())
    }

    async fn focus(&self, session: &AttachedId) -> Result<(), AttachError> {
        let reply = self.ask(FOCUS, &[session.to_string()]).await?;
        if reply.is_empty() {
            return Err(AttachError::Gone {
                session: session.clone(),
            });
        }
        Ok(())
    }

    fn describe(&self) -> &str {
        APPLICATION
    }
}

/// Reads one tab out of a reply.
///
/// A record missing its fields is skipped rather than failing the listing: one
/// tab PushOS cannot read should not hide the other seven.
fn parse(record: &str) -> Option<Attached> {
    let mut fields = record.trim().split(FIELD);
    let device = fields.next()?.trim();
    if !device.starts_with(DEVICE_PREFIX) {
        return None;
    }

    let busy = fields.next().unwrap_or("false").trim();
    let title = fields.next().unwrap_or_default().trim();
    // The rest of the record is the screen, which may contain anything at all
    // including the field separator if a program drew one, so it is whatever
    // is left rather than the next field.
    let seen: String = fields.collect::<Vec<_>>().join(&FIELD.to_string());

    Some(Attached {
        id: AttachedId::new(device),
        activity: activity_of(title, &seen),
        title: title.to_owned(),
        busy: busy.eq_ignore_ascii_case("true"),
    })
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

    fn watcher(processes: &FakeProcesses) -> TerminalAppSessions {
        TerminalAppSessions::new(Arc::new(processes.clone()))
    }

    fn record(device: &str, busy: &str, title: &str) -> String {
        with_screen(device, busy, title, "❯ ")
    }

    fn with_screen(device: &str, busy: &str, title: &str, screen: &str) -> String {
        format!("{device}{FIELD}{busy}{FIELD}{title}{FIELD}{screen}{RECORD}")
    }

    #[test]
    fn a_reply_becomes_the_sessions_it_describes() {
        let reply = format!(
            "{}{}",
            record("/dev/ttys003", "true", "✳ Sprint 2 setup"),
            record("/dev/ttys004", "false", "◐ Push OS system")
        );
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id.as_str(), "/dev/ttys003");
        assert!(found[0].busy);
        assert_eq!(found[0].label(), "Sprint 2 setup");
        assert!(!found[1].busy);
    }

    #[test]
    fn what_a_session_is_doing_is_read_while_looking_at_what_is_open() {
        // One script for all of them. Eight windows must not mean nine
        // processes every three seconds.
        let reply = format!(
            "{}{}",
            with_screen("/dev/ttys003", "true", "◐ Working now", "❯ "),
            with_screen(
                "/dev/ttys004",
                "true",
                "✳ Asking",
                "Proceed?\n❯ 1. Yes\n  2. No"
            )
        );
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(
            found[0].activity,
            pushos_domain::attached::Activity::Working
        );
        assert_eq!(
            found[1].activity,
            pushos_domain::attached::Activity::NeedsDecision
        );
    }

    #[test]
    fn a_screen_containing_the_separator_does_not_become_another_session() {
        // A program may draw anything, including the character used to split
        // the fields. The screen is whatever is left, not the next field.
        let odd = format!("some output{FIELD}more output");
        let reply = with_screen("/dev/ttys003", "true", "✳ Odd", &odd);
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_str(), "/dev/ttys003");
    }

    #[test]
    fn a_title_containing_anything_at_all_is_still_one_field() {
        // Titles are whatever a program set them to. A tab called "a, b: c"
        // must not become three sessions.
        let reply = record("/dev/ttys003", "true", "a, b: c | d\ttab");
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "a, b: c | d\ttab");
    }

    #[test]
    fn a_record_with_no_device_is_skipped_rather_than_failing_the_listing() {
        // One tab PushOS cannot read should not hide the other seven.
        let reply = format!(
            "{}{}",
            record("", "true", "broken"),
            record("/dev/ttys003", "true", "fine")
        );
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "fine");
    }

    #[tokio::test]
    async fn discovery_runs_a_fixed_script_with_no_arguments() {
        // Nothing a session titles itself can change what runs.
        let processes = FakeProcesses::new();
        watcher(&processes).discover().await.ok();

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
        let _ = watcher(&processes)
            .send(
                &AttachedId::new("/dev/ttys003"),
                "\" & (do shell script \"id\") & \"",
            )
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
        let _ = watcher(&processes)
            .press(&AttachedId::new("/dev/ttys003"), Key::Tab)
            .await;

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
    async fn a_session_that_has_gone_says_so_rather_than_failing() {
        // A window the operator closed is a fact, not a fault.
        let processes = FakeProcesses::new();
        let error = watcher(&processes)
            .read(&AttachedId::new("/dev/ttys003"), 200)
            .await
            .expect_err("the fake replies with nothing");
        assert!(matches!(error, AttachError::Gone { .. }));
        assert_eq!(error.class(), ErrorClass::Validation);
    }

    #[tokio::test]
    async fn a_refused_automation_permission_says_what_to_allow() {
        let processes = FakeProcesses::new();
        processes.fail_with(ErrorClass::Permission);

        let error = watcher(&processes)
            .discover()
            .await
            .expect_err("permission was refused");
        assert!(matches!(error, AttachError::NotPermitted { .. }));
        assert!(error.to_string().contains("Automation"), "{error}");
    }
}
