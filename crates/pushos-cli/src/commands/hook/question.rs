//! Reading what an agent asks, and writing back what the operator decided.
//!
//! Claude Code and Codex send the same shape to a permission hook and accept
//! the same shape back, documented by each: the tool, what it would do, and
//! the session asking. Anything missing is left out rather than refused,
//! because a question that cannot be read in full is still one the operator
//! can answer, and a hook that failed would leave the agent to its own prompt
//! anyway.

use pushos_domain::attached::{Decision, SessionQuestion};
use serde_json::{Value, json};

/// How much of what a tool would do is shown. The display has one line for it.
const DETAIL: usize = 300;

/// The agent a hook was installed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Agent {
    /// Claude Code.
    Claude,
    /// Codex, from `OpenAI`.
    Codex,
}

impl Agent {
    /// What the operator calls it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

/// Reads what a permission hook was given.
///
/// `None` for input that is not a permission request at all.
pub(crate) fn read(agent: Agent, input: &str) -> Option<SessionQuestion> {
    let value: Value = serde_json::from_str(input).ok()?;
    let event = value.get("hook_event_name").and_then(Value::as_str);
    if event.is_some_and(|event| event != "PermissionRequest") {
        return None;
    }

    let tool = value
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("a tool")
        .to_owned();
    let detail = value.get("tool_input").map(describe).unwrap_or_default();

    Some(SessionQuestion {
        agent: agent.name().to_owned(),
        session: value
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        device: None,
        tool,
        detail: shortened(&detail),
    })
}

/// What a tool would do, in the words most worth reading.
///
/// The command for a shell, the file for an edit, the address for a fetch,
/// and the tool's own description when there is nothing more specific.
fn describe(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "notebook_path",
        "path",
        "url",
        "pattern",
        "query",
    ] {
        match input.get(key) {
            Some(Value::String(text)) => return text.clone(),
            // Codex may give a command as its arguments.
            Some(Value::Array(parts)) => {
                return parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            _ => {}
        }
    }
    input
        .get("description")
        .and_then(Value::as_str)
        .map_or_else(|| input.to_string(), str::to_owned)
}

/// One line, no longer than the display can use.
fn shortened(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= DETAIL {
        return line;
    }
    let mut cut: String = line.chars().take(DETAIL - 1).collect();
    cut.push('…');
    cut
}

/// What a permission hook prints to give a decision.
pub(crate) fn decision(decision: Decision) -> String {
    let answer = match decision {
        Decision::Allow => json!({ "behavior": "allow" }),
        Decision::Deny => json!({ "behavior": "deny", "message": "Denied from the Push." }),
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": answer,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_request_becomes_a_question() {
        // As a PermissionRequest hook received it from a running session.
        let input = r#"{"session_id":"cf658ff0","cwd":"/work","permission_mode":"default",
            "hook_event_name":"PermissionRequest","tool_name":"Bash",
            "tool_input":{"command":"touch /tmp/probe-file","description":"Create a file"},
            "permission_suggestions":[]}"#;
        let question = read(Agent::Claude, input).expect("a permission request");
        assert_eq!(question.agent, "Claude Code");
        assert_eq!(question.session.as_deref(), Some("cf658ff0"));
        assert_eq!(question.tool, "Bash");
        assert_eq!(question.detail, "touch /tmp/probe-file");
    }

    #[test]
    fn codexs_request_becomes_a_question_whatever_shape_its_command_takes() {
        let input = r#"{"session_id":"019e","turn_id":"t1","hook_event_name":"PermissionRequest",
            "tool_name":"Bash","tool_input":{"command":["bash","-lc","cargo test"],"description":null}}"#;
        let question = read(Agent::Codex, input).expect("a permission request");
        assert_eq!(question.agent, "Codex");
        assert_eq!(question.detail, "bash -lc cargo test");
    }

    #[test]
    fn an_edit_is_described_by_its_file() {
        let input = r#"{"hook_event_name":"PermissionRequest","tool_name":"Write",
            "tool_input":{"file_path":"/work/notes.md","content":"a long body"}}"#;
        assert_eq!(
            read(Agent::Claude, input).expect("read").detail,
            "/work/notes.md"
        );
    }

    #[test]
    fn anything_but_a_permission_request_is_not_a_question() {
        assert!(read(Agent::Claude, r#"{"hook_event_name":"Stop"}"#).is_none());
        assert!(read(Agent::Claude, "not json").is_none());
    }

    #[test]
    fn a_long_command_is_one_short_line() {
        let long = format!(
            r#"{{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{{"command":"{}\n{}"}}}}"#,
            "a".repeat(400),
            "b"
        );
        let detail = read(Agent::Claude, &long).expect("read").detail;
        assert!(!detail.contains('\n'));
        assert_eq!(detail.chars().count(), DETAIL);
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn a_decision_is_written_the_way_both_agents_read_it() {
        let allow: Value = serde_json::from_str(&decision(Decision::Allow)).expect("json");
        assert_eq!(
            allow["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        assert_eq!(allow["hookSpecificOutput"]["decision"]["behavior"], "allow");

        let deny: Value = serde_json::from_str(&decision(Decision::Deny)).expect("json");
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(deny["hookSpecificOutput"]["decision"]["message"].is_string());
    }
}
