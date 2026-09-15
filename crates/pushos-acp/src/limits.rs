//! Keeping an agent inside what its role allows.
//!
//! Three places, strongest first, because an agent that is never asked cannot
//! be refused:
//!
//! - **At the start**, the tools a role does not allow are taken away where
//!   the agent can be told so. Claude Code's adapter accepts tools to disallow
//!   when a session opens, and it cannot use what it does not have, whatever
//!   its own settings say.
//! - **The mode**, where an agent offers a read-only one, is chosen for a role
//!   that may not write.
//! - **Each question** the agent asks is sorted by what the tool does, and one
//!   the role does not allow is refused on the spot rather than put to the
//!   operator, who would only have to say no.
//!
//! The names of Claude Code's tools come from its tools reference. A tool it
//! adds later that a role would not allow is still caught by the question,
//! whenever it asks one.

use agent_client_protocol::schema::v1::{Meta, SessionModeState, ToolKind};
use pushos_domain::permissions::{Permission, PermissionSet};
use pushos_domain::tool_use::ToolUse;
use serde_json::{Value, json};

/// Claude Code's tools, by the permission each needs.
///
/// Git's commands are rules on its shell tool, so a role that may run commands
/// but not push still cannot push.
const CLAUDE_TOOLS: [(Permission, &[&str]); 8] = [
    (Permission::FilesystemRead, &["Read", "Glob", "Grep", "LSP"]),
    (
        Permission::FilesystemWrite,
        &["Edit", "Write", "NotebookEdit"],
    ),
    (Permission::ShellExecute, &["Bash", "PowerShell", "Monitor"]),
    (Permission::Network, &["WebFetch", "WebSearch"]),
    (
        Permission::ExternalSend,
        &[
            "Artifact",
            "PushNotification",
            "RemoteTrigger",
            "SendUserFile",
        ],
    ),
    (Permission::GitCommit, &["Bash(git commit *)"]),
    (Permission::GitPush, &["Bash(git push *)"]),
    (Permission::GitMerge, &["Bash(git merge *)"]),
];

/// The mode an agent offers for working without changing anything.
const READ_ONLY_MODES: [&str; 1] = ["read-only"];

/// Sorts a tool the agent described into what it does.
pub(crate) const fn tool_use(kind: ToolKind) -> ToolUse {
    match kind {
        ToolKind::Read | ToolKind::Search => ToolUse::Reading,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => ToolUse::Writing,
        ToolKind::Execute => ToolUse::Running,
        ToolKind::Fetch => ToolUse::Fetching,
        _ => ToolUse::Other,
    }
}

/// What to tell a provider about a role's limits when its session opens.
///
/// Only Claude Code's adapter takes this, so only it is told. Its sessions
/// are also kept out of the mode that asks about nothing, which a role that
/// lists its permissions would otherwise be one setting away from.
pub(crate) fn session_meta(provider: &str, permissions: &PermissionSet) -> Option<Meta> {
    if provider != "claude" {
        return None;
    }
    let disallowed: Vec<Value> = CLAUDE_TOOLS
        .iter()
        .filter(|(needed, _)| !permissions.allows(*needed))
        .flat_map(|(_, tools)| tools.iter().map(|tool| Value::from(*tool)))
        .collect();

    let meta = json!({
        "claudeCode": {
            "options": {
                "disallowedTools": disallowed,
                "allowDangerouslySkipPermissions": false,
            }
        }
    });
    match meta {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// The read-only mode to switch to, for a role that may not write files.
pub(crate) fn read_only_mode(
    modes: &SessionModeState,
    permissions: &PermissionSet,
) -> Option<String> {
    if permissions.allows(Permission::FilesystemWrite) {
        return None;
    }
    modes
        .available_modes
        .iter()
        .map(|mode| mode.id.to_string())
        .find(|id| READ_ONLY_MODES.contains(&id.as_str()))
        .filter(|id| *id != modes.current_mode_id.to_string())
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::SessionMode;

    use super::*;

    fn researcher() -> PermissionSet {
        [Permission::FilesystemRead, Permission::Network]
            .into_iter()
            .collect()
    }

    fn disallowed(meta: &Meta) -> Vec<String> {
        meta["claudeCode"]["options"]["disallowedTools"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|tool| tool.as_str().expect("a name").to_owned())
            .collect()
    }

    #[test]
    fn a_research_role_on_claude_loses_its_shell_and_its_pen_and_keeps_its_eyes() {
        let meta = session_meta("claude", &researcher()).expect("claude is told");
        let gone = disallowed(&meta);

        for taken in [
            "Bash",
            "PowerShell",
            "Monitor",
            "Edit",
            "Write",
            "NotebookEdit",
        ] {
            assert!(
                gone.iter().any(|tool| tool == taken),
                "{taken} is taken away"
            );
        }
        for kept in ["Read", "Grep", "WebFetch", "WebSearch"] {
            assert!(!gone.iter().any(|tool| tool == kept), "{kept} is kept");
        }
        assert_eq!(
            meta["claudeCode"]["options"]["allowDangerouslySkipPermissions"], false,
            "and it can never be put in the mode that asks about nothing"
        );
    }

    #[test]
    fn a_role_that_may_run_commands_but_not_push_still_cannot_push() {
        let builder: PermissionSet = [
            Permission::FilesystemRead,
            Permission::FilesystemWrite,
            Permission::ShellExecute,
            Permission::GitCommit,
        ]
        .into_iter()
        .collect();
        let gone = disallowed(&session_meta("claude", &builder).expect("told"));
        assert!(!gone.iter().any(|tool| tool == "Bash"));
        assert!(gone.iter().any(|tool| tool == "Bash(git push *)"));
        assert!(!gone.iter().any(|tool| tool == "Bash(git commit *)"));
    }

    #[test]
    fn only_claude_code_is_told_at_the_start() {
        assert!(session_meta("codex", &researcher()).is_none());
        assert!(session_meta("something-else", &researcher()).is_none());
    }

    fn modes(current: &str, available: &[&str]) -> SessionModeState {
        SessionModeState::new(
            current.to_owned(),
            available
                .iter()
                .map(|id| SessionMode::new((*id).to_owned(), (*id).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn a_role_that_may_not_write_is_put_in_the_read_only_mode_when_there_is_one() {
        // Codex's modes, as its adapter names them.
        let codex = modes("agent", &["read-only", "agent", "agent-full-access"]);
        assert_eq!(
            read_only_mode(&codex, &researcher()).as_deref(),
            Some("read-only")
        );

        let writer: PermissionSet = [Permission::FilesystemWrite].into_iter().collect();
        assert_eq!(read_only_mode(&codex, &writer), None);

        let already = modes("read-only", &["read-only", "agent"]);
        assert_eq!(read_only_mode(&already, &researcher()), None);

        let claude = modes("default", &["default", "acceptEdits", "plan"]);
        assert_eq!(
            read_only_mode(&claude, &researcher()),
            None,
            "planning is not the same as reading"
        );
    }

    #[test]
    fn every_kind_of_tool_is_sorted() {
        assert_eq!(tool_use(ToolKind::Execute), ToolUse::Running);
        assert_eq!(tool_use(ToolKind::Edit), ToolUse::Writing);
        assert_eq!(tool_use(ToolKind::Delete), ToolUse::Writing);
        assert_eq!(tool_use(ToolKind::Fetch), ToolUse::Fetching);
        assert_eq!(tool_use(ToolKind::Search), ToolUse::Reading);
        assert_eq!(tool_use(ToolKind::Think), ToolUse::Other);
        assert_eq!(tool_use(ToolKind::Other), ToolUse::Other);
    }
}
