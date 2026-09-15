//! A question a session PushOS did not start puts to the operator.
//!
//! Claude Code and Codex can each run a command of the operator's choosing
//! when they are about to ask permission for something, and take its answer
//! as the operator's. PushOS is that command: the question goes to the Push,
//! and a press answers it. The agent shows its own prompt at the same time, so
//! whichever is answered first is the answer.

use serde::{Deserialize, Serialize};

use super::Attached;

/// What a session is asking to be allowed to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQuestion {
    /// Which agent is asking, as the operator knows it: `Claude Code`, `Codex`.
    pub agent: String,
    /// The agent's own identity for the session, when it gave one.
    #[serde(default)]
    pub session: Option<String>,
    /// The terminal device the session runs on, when it has one.
    #[serde(default)]
    pub device: Option<String>,
    /// The tool it wants to use, such as `Bash`.
    pub tool: String,
    /// What exactly: the command, the file, the address.
    #[serde(default)]
    pub detail: String,
}

impl SessionQuestion {
    /// Whether this question comes from a session that is open.
    ///
    /// By the device both are on, or, for a session on no device, by the
    /// identity its host gave it.
    pub fn is_from(&self, session: &Attached) -> bool {
        let on_device = self
            .device
            .as_deref()
            .is_some_and(|device| session.id.as_str() == device);
        let by_identity = self
            .session
            .as_deref()
            .is_some_and(|id| session.id.as_str() == format!("claude:{id}"));
        on_device || by_identity
    }

    /// The question in one line, for the display.
    pub fn describe(&self) -> String {
        let detail = self.detail.trim();
        if detail.is_empty() {
            format!("{} wants to use {}", self.agent, self.tool)
        } else {
            format!("{} wants to use {}: {detail}", self.agent, self.tool)
        }
    }
}

/// What the operator decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Let it go ahead.
    Allow,
    /// Do not.
    Deny,
}

#[cfg(test)]
mod tests {
    use super::super::Activity;
    use super::*;

    fn question(device: Option<&str>, session: Option<&str>) -> SessionQuestion {
        SessionQuestion {
            agent: "Claude Code".to_owned(),
            session: session.map(str::to_owned),
            device: device.map(str::to_owned),
            tool: "Bash".to_owned(),
            detail: "touch notes.md".to_owned(),
        }
    }

    #[test]
    fn a_question_belongs_to_the_session_on_its_device() {
        let tab = Attached::new("/dev/ttys003", "work", Activity::NeedsDecision, "Terminal");
        assert!(question(Some("/dev/ttys003"), None).is_from(&tab));
        assert!(!question(Some("/dev/ttys004"), None).is_from(&tab));
    }

    #[test]
    fn a_question_from_an_editors_panel_belongs_to_the_session_of_that_identity() {
        let panel = Attached::new(
            "claude:0d2c",
            "panel",
            Activity::NeedsDecision,
            "Visual Studio Code",
        )
        .watched_only();
        assert!(question(None, Some("0d2c")).is_from(&panel));
        assert!(!question(None, Some("ffff")).is_from(&panel));
    }

    #[test]
    fn a_question_says_who_wants_what_in_one_line() {
        assert_eq!(
            question(None, None).describe(),
            "Claude Code wants to use Bash: touch notes.md"
        );
    }
}
