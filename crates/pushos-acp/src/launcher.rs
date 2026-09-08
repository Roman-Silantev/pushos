//! How an agent process is started.
//!
//! A provider is a command, not a vendor. PushOS ships defaults for the tools
//! most people have, and every one of them can be replaced in configuration
//! without changing any code here.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// What to run to get an agent that speaks the protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    /// The program to run.
    pub program: String,
    /// Its arguments, passed without any shell interpretation.
    pub args: Vec<String>,
    /// Environment to set for it.
    ///
    /// For pointing at an installation, never for a credential: PushOS does not
    /// persist secrets, and a provider signs in for itself.
    pub env: BTreeMap<String, String>,
}

impl AgentCommand {
    /// Builds a command.
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            env: BTreeMap::new(),
        }
    }

    /// Sets an environment variable for the process.
    #[must_use]
    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }

    /// The adapter that fronts Claude Code.
    ///
    /// A default, not a hard-coded dependency: it is the same shape as anything
    /// an operator writes in their own configuration.
    pub fn claude() -> Self {
        Self::new(
            "npx",
            [
                "-y".to_owned(),
                "@agentclientprotocol/claude-agent-acp@latest".to_owned(),
            ],
        )
    }

    /// The adapter that fronts Codex.
    pub fn codex() -> Self {
        Self::new(
            "npx",
            [
                "-y".to_owned(),
                "@agentclientprotocol/codex-acp@latest".to_owned(),
            ],
        )
    }

    /// The defaults PushOS knows, by the name a role would prefer.
    pub fn known(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::claude()),
            "codex" => Some(Self::codex()),
            _ => None,
        }
    }

    /// How the command reads, for logs and diagnostics.
    pub fn describe(&self) -> String {
        if self.args.is_empty() {
            return self.program.clone();
        }
        format!("{} {}", self.program, self.args.join(" "))
    }

    /// Whether the program is named at all.
    pub fn is_runnable(&self) -> bool {
        !self.program.trim().is_empty()
    }
}

/// Where a session does its work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The directory the agent is pointed at.
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_defaults_are_runnable_commands() {
        for name in ["claude", "codex"] {
            let command = AgentCommand::known(name).expect("PushOS ships a default");
            assert!(command.is_runnable());
            assert!(
                !command.args.is_empty(),
                "an adapter needs its package named"
            );
        }
    }

    #[test]
    fn an_unknown_provider_has_no_default_and_must_be_configured() {
        assert!(AgentCommand::known("something-else").is_none());
    }

    #[test]
    fn a_command_reads_as_it_would_be_typed() {
        let command = AgentCommand::new("my-agent", ["--acp".to_owned()]);
        assert_eq!(command.describe(), "my-agent --acp");
        assert_eq!(AgentCommand::new("bare", []).describe(), "bare");
    }

    #[test]
    fn a_command_with_nothing_to_run_is_not_runnable() {
        assert!(!AgentCommand::new("   ", []).is_runnable());
        assert!(!AgentCommand::new("", []).is_runnable());
    }

    #[test]
    fn environment_is_carried_but_stays_the_operators_business() {
        let command = AgentCommand::claude().with_env("CLAUDE_HOME", "/opt/claude");
        assert_eq!(
            command.env.get("CLAUDE_HOME").map(String::as_str),
            Some("/opt/claude")
        );
    }
}
