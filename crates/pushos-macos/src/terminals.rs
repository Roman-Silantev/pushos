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
use pushos_domain::attached::Attached;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::{AttachError, AttachedSessions, ProcessRunner};
use tracing::debug;

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

/// Lists every tab, with its device, whether it is busy and what it is called.
const DISCOVER: &str = r#"on run argv
  set out to ""
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        try
          set out to out & ((tty of t) as text) & (ASCII character 31) & ((busy of t) as text) & (ASCII character 31) & ((custom title of t) as text) & (ASCII character 30)
        end try
      end repeat
    end repeat
  end tell
  return out
end run"#;

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
        let reply = self.ask(DISCOVER, &[]).await?;
        let found: Vec<Attached> = reply.split(RECORD).filter_map(parse).collect();
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
    if device.is_empty() {
        return None;
    }

    let busy = fields.next().unwrap_or("false").trim();
    let title = fields.next().unwrap_or_default().trim();

    Some(Attached {
        id: AttachedId::new(device),
        title: title.to_owned(),
        busy: busy.eq_ignore_ascii_case("true"),
    })
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
        format!("{device}{FIELD}{busy}{FIELD}{title}{RECORD}")
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
        assert_eq!(spawned[0].args.len(), 2, "the script and nothing else");
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
