//! Adding PushOS to the hooks Claude Code and Codex run, and taking it out.
//!
//! Both keep hooks in a JSON file of the same shape, event by event, so one
//! edit serves both. Only PushOS's own entry is touched: it is recognised by
//! the command it runs, replaced when installed again and removed on its own,
//! and whatever else the operator configured is left where it was.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::question::Agent;

/// The event PushOS answers.
const EVENT: &str = "PermissionRequest";

/// What marks a hook entry as PushOS's.
const MARKER: &str = " hook permission --agent ";

/// How long an agent waits for PushOS before deciding without it, in seconds.
///
/// Both agents' default, stated so it does not change under PushOS.
const TIMEOUT_SECONDS: u64 = 600;

/// The file an agent reads its hooks from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookFile {
    pub(crate) agent: Agent,
    pub(crate) path: PathBuf,
}

impl HookFile {
    /// Every agent installed for this user, with the file its hooks go in.
    ///
    /// An agent whose directory does not exist is not installed, and is not
    /// given a configuration directory by PushOS.
    pub(crate) fn installed() -> Vec<Self> {
        let home = pushos_config::paths::home_directory();
        let directory = |variable: &str, default: &str| {
            std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(default)))
        };

        [
            (
                Agent::Claude,
                directory("CLAUDE_CONFIG_DIR", ".claude"),
                "settings.json",
            ),
            (
                Agent::Codex,
                directory("CODEX_HOME", ".codex"),
                "hooks.json",
            ),
        ]
        .into_iter()
        .filter_map(|(agent, directory, file)| {
            let directory = directory?;
            directory.is_dir().then(|| Self {
                agent,
                path: directory.join(file),
            })
        })
        .collect()
    }

    /// Whether PushOS's hook is in the file.
    pub(crate) fn has_pushos(&self) -> bool {
        read(&self.path).is_ok_and(|document| {
            let (_, removed) = without_pushos(document);
            removed
        })
    }
}

/// The command line a hook runs, quoted for the shell that runs it.
pub(crate) fn command(program: &Path, agent: Agent) -> String {
    let program = program.to_string_lossy().replace('\'', r"'\''");
    let agent = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    format!("'{program}'{MARKER}{agent}")
}

/// The entry PushOS adds.
pub(crate) fn entry(command: &str) -> Value {
    json!({
        "matcher": "*",
        "hooks": [{ "type": "command", "command": command, "timeout": TIMEOUT_SECONDS }],
    })
}

/// The document with PushOS's entry in it, replacing one already there.
pub(crate) fn with_pushos(document: Value, command: &str) -> Result<Value, String> {
    let (document, _) = without_pushos(document);
    let mut root = match document {
        Value::Object(root) => root,
        Value::Null => Map::new(),
        _ => return Err("it is not a JSON object".to_owned()),
    };

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(hooks) = hooks else {
        return Err("its `hooks` is not an object".to_owned());
    };
    let entries = hooks
        .entry(EVENT)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(entries) = entries else {
        return Err(format!("its `hooks.{EVENT}` is not a list"));
    };
    entries.push(entry(command));
    Ok(Value::Object(root))
}

/// The document without PushOS's entry, and whether there was one.
///
/// A list or table left empty by the removal is removed too, so installing and
/// uninstalling leaves the file as it was found.
pub(crate) fn without_pushos(document: Value) -> (Value, bool) {
    let Value::Object(mut root) = document else {
        return (document, false);
    };
    let Some(Value::Object(hooks)) = root.get_mut("hooks") else {
        return (Value::Object(root), false);
    };
    let Some(Value::Array(entries)) = hooks.get_mut(EVENT) else {
        return (Value::Object(root), false);
    };

    let before = entries.len();
    entries.retain(|entry| !is_pushos(entry));
    let removed = entries.len() != before;

    if entries.is_empty() {
        hooks.remove(EVENT);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    (Value::Object(root), removed)
}

fn is_pushos(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| command.contains(MARKER))
            })
        })
}

/// Reads a hook file. One that does not exist yet is empty.
pub(crate) fn read(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Value::Null),
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            format!(
                "{} is not valid JSON ({error}); leaving it alone",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/// Writes a hook file so a reader never sees half of it.
///
/// The first time PushOS changes a file that already existed, a copy of what
/// was there is kept beside it.
pub(crate) fn write(path: &Path, document: &Value) -> Result<(), String> {
    let backup = path.with_extension("json.before-pushos");
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup)
            .map_err(|error| format!("could not keep a copy of {}: {error}", path.display()))?;
    }

    let mut text = serde_json::to_string_pretty(document)
        .map_err(|error| format!("could not write the hooks: {error}"))?;
    text.push('\n');
    let staging = path.with_extension("json.installing");
    std::fs::write(&staging, text)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    std::fs::rename(&staging, path)
        .map_err(|error| format!("could not put {} in place: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str =
        "'/Users/me/Applications/PushOS.app/Contents/MacOS/pushos' hook permission --agent claude";

    #[test]
    fn the_hook_is_added_beside_whatever_the_operator_already_had() {
        let existing = json!({
            "model": "opus",
            "hooks": {
                "PermissionRequest": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "/my/check.sh" }] }
                ],
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }]
            }
        });
        let installed = with_pushos(existing, COMMAND).expect("an object");

        assert_eq!(installed["model"], "opus");
        assert_eq!(
            installed["hooks"]["Stop"][0]["hooks"][0]["command"],
            "say done"
        );
        let entries = installed["hooks"]["PermissionRequest"]
            .as_array()
            .expect("a list");
        assert_eq!(entries.len(), 2, "theirs and PushOS's");
        assert_eq!(entries[1]["hooks"][0]["command"], COMMAND);
        assert_eq!(entries[1]["hooks"][0]["timeout"], TIMEOUT_SECONDS);
    }

    #[test]
    fn installing_twice_leaves_one_hook() {
        let once = with_pushos(Value::Null, COMMAND).expect("empty is fine");
        let twice = with_pushos(once, COMMAND).expect("still fine");
        assert_eq!(
            twice["hooks"]["PermissionRequest"]
                .as_array()
                .expect("a list")
                .len(),
            1
        );
    }

    #[test]
    fn uninstalling_leaves_the_file_as_it_was_found() {
        let original = json!({ "model": "opus", "hooks": { "Stop": [{ "hooks": [] }] } });
        let installed = with_pushos(original.clone(), COMMAND).expect("an object");
        let (removed, was_there) = without_pushos(installed);
        assert!(was_there);
        assert_eq!(removed, original);

        let (untouched, was_there) = without_pushos(original.clone());
        assert!(!was_there);
        assert_eq!(untouched, original);
    }

    #[test]
    fn a_file_that_is_not_what_it_should_be_is_refused_rather_than_rewritten() {
        assert!(with_pushos(json!([1, 2]), COMMAND).is_err());
        assert!(with_pushos(json!({ "hooks": "nope" }), COMMAND).is_err());
    }

    #[test]
    fn the_command_is_quoted_for_the_shell_that_runs_it() {
        assert_eq!(
            command(Path::new("/Apps/it's here/pushos"), Agent::Codex),
            r"'/Apps/it'\''s here/pushos' hook permission --agent codex"
        );
    }

    #[test]
    fn the_file_is_written_whole_and_the_original_kept() {
        let directory = std::env::temp_dir().join(format!("pushos-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("writable");
        let path = directory.join("settings.json");
        std::fs::write(&path, "{\"model\":\"opus\"}").expect("writable");

        let installed = with_pushos(read(&path).expect("json"), COMMAND).expect("object");
        write(&path, &installed).expect("written");

        assert_eq!(read(&path).expect("json"), installed);
        assert_eq!(
            std::fs::read_to_string(directory.join("settings.json.before-pushos")).expect("kept"),
            "{\"model\":\"opus\"}"
        );
        std::fs::remove_dir_all(directory).ok();
    }
}
