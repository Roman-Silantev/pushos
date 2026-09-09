//! The shape of a PushOS configuration file.
//!
//! These types mirror the TOML exactly and do nothing else. Turning them into
//! something the runtime can use is a separate, fallible step, so a malformed
//! file is rejected with a message about the file rather than half-applied.

use std::collections::BTreeMap;

use pushos_domain::action::ParamValue;
use pushos_domain::permissions::Permission;
use serde::{Deserialize, Serialize};

/// One parsed configuration file, or the merge of several.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Runtime-wide settings.
    #[serde(default)]
    pub runtime: RuntimeSection,
    /// Gesture timing.
    #[serde(default)]
    pub gestures: GestureSection,
    /// Capabilities the operator has granted.
    #[serde(default)]
    pub permissions: PermissionSection,
    /// The pages that exist.
    #[serde(default)]
    pub pages: Vec<PageEntry>,
    /// The bindings in force.
    #[serde(default)]
    pub bindings: Vec<BindingEntry>,
    /// The agent roles that exist.
    #[serde(default)]
    pub agents: Vec<AgentEntry>,
    /// The agent providers PushOS may start.
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
    /// The projects that exist.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    /// The workflows that exist.
    #[serde(default)]
    pub workflows: Vec<WorkflowEntry>,
    /// Push to talk.
    #[serde(default)]
    pub voice: VoiceSection,
    /// Notes PushOS can capture and find.
    #[serde(default)]
    pub memory: MemorySection,
    /// The terminals the operator already had open.
    #[serde(default)]
    pub sessions: SessionSection,
}

impl ConfigFile {
    /// Folds another file into this one.
    ///
    /// Pages and bindings accumulate so a configuration can be split across
    /// files. Scalar settings are last-writer-wins, and files are merged in a
    /// deterministic order, so the result never depends on directory listing
    /// order.
    pub fn merge(&mut self, other: Self) {
        self.runtime.merge(other.runtime);
        self.gestures.merge(other.gestures);
        self.permissions.granted.extend(other.permissions.granted);
        self.pages.extend(other.pages);
        self.bindings.extend(other.bindings);
        self.agents.extend(other.agents);
        self.providers.extend(other.providers);
        self.workspaces.extend(other.workspaces);
        self.workflows.extend(other.workflows);
        self.voice.merge(other.voice);
        self.memory.merge(other.memory);
        self.sessions.merge(&other.sessions);
    }
}

/// Runtime-wide settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    /// The page PushOS starts on and returns to.
    pub home_page: Option<String>,
    /// The media player to control.
    pub media_player: Option<String>,
    /// Where agents work when a role does not name a workspace.
    ///
    /// A leading `~` is expanded. Defaults to wherever PushOS was started.
    pub workspace_root: Option<String>,
    /// The program a terminal runs when a binding does not name one.
    ///
    /// Defaults to the operator's own shell.
    pub shell: Option<String>,
}

impl RuntimeSection {
    fn merge(&mut self, other: Self) {
        if other.home_page.is_some() {
            self.home_page = other.home_page;
        }
        if other.media_player.is_some() {
            self.media_player = other.media_player;
        }
        if other.workspace_root.is_some() {
            self.workspace_root = other.workspace_root;
        }
        if other.shell.is_some() {
            self.shell = other.shell;
        }
    }
}

/// The terminals the operator already had open.
///
/// Off unless switched on. PushOS asks another application what is open, which
/// macOS will ask the operator to allow, and doing that unbidden on a machine
/// with no interest in the feature would be rude.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSection {
    /// Whether to watch them at all.
    #[serde(default)]
    pub watch: bool,
    /// How often to ask what is open, in milliseconds.
    ///
    /// Asking costs a subprocess. Defaults to every three seconds, which is
    /// faster than an operator opens windows and slower than the display is
    /// redrawn.
    pub poll_ms: Option<u64>,
}

impl SessionSection {
    fn merge(&mut self, other: &Self) {
        self.watch |= other.watch;
        if other.poll_ms.is_some() {
            self.poll_ms = other.poll_ms;
        }
    }
}

/// Notes PushOS can capture and find.
///
/// Absent means memory is off. Notes are Markdown files in directories the
/// operator chose, so this says which directories rather than describing a
/// store: there is no store, only their files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySection {
    /// What a briefing is handed to, written as `provider.verb`.
    ///
    /// The notes arrive as `text`. Absent means a briefing shows what it found
    /// and sends it nowhere.
    pub brief: Option<String>,
    /// Where notes are kept, in the order they are searched.
    #[serde(default)]
    pub sources: Vec<MemorySourceEntry>,
}

impl MemorySection {
    fn merge(&mut self, other: Self) {
        if other.brief.is_some() {
            self.brief = other.brief;
        }
        self.sources.extend(other.sources);
    }
}

/// One directory of notes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySourceEntry {
    /// What it is called. Becomes the first part of every note's identity.
    pub id: String,
    /// Where it is. A leading `~` is expanded.
    pub path: String,
    /// Whether PushOS may add notes to it.
    ///
    /// Off unless said otherwise. A directory the operator pointed PushOS at is
    /// theirs, and reading it is not permission to write in it.
    #[serde(default)]
    pub writable: bool,
}

/// Push to talk.
///
/// Absent means voice is off. There is no wake word here and there will not be
/// one: PushOS listens while a control is held and at no other time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSection {
    /// Which engine works out what was said: `apple` or `whisper`.
    ///
    /// Defaults to `apple`, which is already installed.
    pub engine: Option<String>,
    /// The model file, for engines that need one.
    ///
    /// A leading `~` is expanded. Only `whisper` uses this.
    pub model: Option<String>,
    /// What a phrase that is not a configured command runs.
    ///
    /// Written as `provider.verb`. The words go to it as `text`, which is what
    /// `agent.prompt` and `terminal.run` already read. Absent means anything
    /// unrecognised is reported and nothing else.
    pub request: Option<String>,
    /// The phrases that mean exactly one thing.
    #[serde(default)]
    pub commands: Vec<VoiceCommandEntry>,
}

impl VoiceSection {
    fn merge(&mut self, other: Self) {
        if other.engine.is_some() {
            self.engine = other.engine;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.request.is_some() {
            self.request = other.request;
        }
        self.commands.extend(other.commands);
    }
}

/// One spoken phrase and what it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceCommandEntry {
    /// What the operator says.
    pub phrase: String,
    /// What it runs, written as `provider.verb`.
    pub action: String,
    /// What the action acts on.
    pub target: Option<String>,
    /// Anything else the action needs.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// Whether it waits for a deliberate press before it happens.
    ///
    /// Transcription is not authorisation. Anything that deploys, merges,
    /// deletes or sends belongs here.
    #[serde(default)]
    pub confirm: bool,
}

/// Gesture timing, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureSection {
    /// How long a press must last to become a hold.
    pub hold_threshold_ms: Option<u64>,
    /// How close two taps must be to become a double tap.
    pub double_tap_window_ms: Option<u64>,
}

impl GestureSection {
    fn merge(&mut self, other: Self) {
        if other.hold_threshold_ms.is_some() {
            self.hold_threshold_ms = other.hold_threshold_ms;
        }
        if other.double_tap_window_ms.is_some() {
            self.double_tap_window_ms = other.double_tap_window_ms;
        }
    }
}

/// Capabilities the operator has granted.
///
/// Absent means nothing is granted. There is deliberately no wildcard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSection {
    /// The granted capabilities, by dotted name.
    #[serde(default)]
    pub granted: Vec<Permission>,
}

/// A page declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEntry {
    /// Stable identity, referenced by bindings.
    pub id: String,
    /// Name shown on the display.
    pub name: String,
    /// Longer description, shown in Studio.
    pub description: Option<String>,
}

/// An agent role.
///
/// A role, not a process: which provider fills it can change without any
/// binding changing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEntry {
    /// Stable identity, referenced by bindings.
    pub id: String,
    /// Name shown on the display.
    pub name: String,
    /// What the role is for, given to the provider as its standing instruction.
    #[serde(default)]
    pub objective: String,
    /// Which providers may fill it, best first. Empty accepts any.
    #[serde(default)]
    pub preferred: Vec<String>,
    /// What this role is allowed to do.
    ///
    /// Applied on top of what the provider itself permits: the stricter of the
    /// two wins, so a role can narrow a provider but never widen it.
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

/// One workflow: a graph of steps that runs itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEntry {
    /// Stable identity, referenced by bindings.
    pub id: String,
    /// What the operator calls it.
    pub name: String,
    /// What it is for, when a name is not enough.
    #[serde(default)]
    pub description: Option<String>,
    /// The step it begins at.
    pub start: String,
    /// How many steps one run may take before it is stopped.
    #[serde(default)]
    pub max_steps: Option<u32>,
    /// How long one run may take, in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// The steps.
    #[serde(default)]
    pub nodes: Vec<NodeEntry>,
}

/// One step of a workflow.
///
/// Flat rather than nested by kind, because a step is a handful of fields and
/// the file is meant to be readable by whoever wrote it. `kind` says which
/// fields apply, and the validator says so plainly when one is missing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeEntry {
    /// What the step is called, in the file and on the display.
    pub id: String,
    /// What it does: `agent`, `terminal`, `action`, `approval`, `condition`,
    /// `delay`, `emit` or `end`.
    pub kind: String,

    /// Where to go next, for the steps that have one way on.
    #[serde(default)]
    pub next: Option<String>,
    /// Where to go when the step did not succeed.
    #[serde(default)]
    pub on_failure: Option<String>,

    /// The role to ask, for an `agent` step.
    #[serde(default)]
    pub agent: Option<String>,
    /// What to ask it.
    #[serde(default)]
    pub prompt: Option<String>,

    /// What the terminal is called, for a `terminal` step.
    #[serde(default)]
    pub terminal: Option<String>,
    /// The program to run in it.
    #[serde(default)]
    pub program: Option<String>,
    /// Its arguments, passed without any shell interpretation.
    #[serde(default)]
    pub args: Vec<String>,

    /// The action to run, for an `action` step, written as `provider.verb`.
    #[serde(default)]
    pub action: Option<String>,
    /// Shorthand for the action's `target` parameter.
    #[serde(default)]
    pub target: Option<String>,
    /// Everything else the action needs.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,

    /// What to ask the operator, for an `approval` step.
    #[serde(default)]
    pub question: Option<String>,
    /// Where to go when they say yes.
    #[serde(default)]
    pub approve: Option<String>,
    /// Where to go when they say no.
    #[serde(default)]
    pub reject: Option<String>,

    /// What to test, for a `condition` step: `succeeded`, `failed` or
    /// `attempts`.
    #[serde(default)]
    pub check: Option<String>,
    /// How many times round is still allowed, for an `attempts` check.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Where to go when the check holds.
    #[serde(default)]
    pub then: Option<String>,
    /// Where to go when it does not.
    #[serde(default)]
    pub otherwise: Option<String>,

    /// How long to wait, for a `delay` step.
    #[serde(default)]
    pub seconds: Option<u64>,

    /// What to say, for an `emit` step.
    #[serde(default)]
    pub message: Option<String>,

    /// How the run ended, for an `end` step: `succeeded`, `failed` or
    /// `cancelled`.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// One project.
///
/// A workspace is what a pad means when it says "Sydclaw": where work happens,
/// what opens it, who fills each role there, and what the surface should show
/// on arrival.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceEntry {
    /// Stable identity, referenced by bindings and targets.
    pub id: String,
    /// Name shown on the display.
    pub name: String,
    /// Where work happens. A leading `~` is expanded.
    pub root: String,
    /// What it is, when a name is not enough.
    #[serde(default)]
    pub description: Option<String>,
    /// The page to show on arrival, when nothing was remembered.
    #[serde(default)]
    pub home_page: Option<String>,
    /// Whether each agent here gets its own working tree.
    ///
    /// Two coding agents editing one checkout produce a conflict nobody can
    /// untangle, so a project that runs several says so.
    #[serde(default)]
    pub isolate_agents: bool,
    /// What each application should open, by the name a binding uses.
    #[serde(default)]
    pub apps: BTreeMap<String, String>,
    /// Which provider fills each role here.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    /// Environment given to everything started here.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// An agent provider PushOS may start.
///
/// Named as a command rather than a vendor, so any agent speaking the protocol
/// can be used without a change to PushOS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEntry {
    /// What bindings and roles call it.
    pub id: String,
    /// The program to run.
    ///
    /// May be left out for a provider PushOS already knows how to start, so
    /// naming one is enough. Naming it is still required: PushOS does not start
    /// a subprocess the operator never mentioned.
    #[serde(default)]
    pub program: Option<String>,
    /// Its arguments, passed without any shell interpretation.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment to set for it.
    ///
    /// For pointing at an installation, not for secrets: PushOS never persists
    /// a credential in configuration.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A binding declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingEntry {
    /// Optional stable identity. One is derived when absent.
    pub id: Option<String>,
    /// The control, such as `pad.23` or `button.play`.
    pub control: String,
    /// The gesture, such as `tap` or `hold`.
    pub gesture: String,
    /// The action, written as `provider.verb`.
    pub action: String,
    /// Restricts the binding to one page.
    pub page: Option<String>,
    /// Restricts the binding to one workspace.
    pub workspace: Option<String>,
    /// Asserts the intended scope. Checked against `page` and `workspace`.
    pub scope: Option<String>,
    /// Shorthand for the `target` parameter, which most actions take.
    pub target: Option<String>,
    /// Everything else the action needs.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// Tie-break within a scope. Higher wins.
    #[serde(default)]
    pub priority: u16,
    /// Label shown on the display and in Studio.
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ConfigFile {
        toml::from_str(text).expect("the test configuration is well-formed")
    }

    #[test]
    fn a_minimal_file_parses_to_empty_defaults() {
        let config = parse("");
        assert!(config.pages.is_empty());
        assert!(config.bindings.is_empty());
        assert!(config.permissions.granted.is_empty());
    }

    #[test]
    fn a_binding_parses_with_only_its_required_fields() {
        let config = parse(
            r#"
            [[bindings]]
            control = "button.play"
            gesture = "press"
            action = "media.play_pause"
            "#,
        );
        let binding = &config.bindings[0];
        assert_eq!(binding.control, "button.play");
        assert_eq!(binding.priority, 0);
        assert!(binding.page.is_none());
        assert!(binding.params.is_empty());
    }

    #[test]
    fn action_parameters_carry_their_types() {
        let config = parse(
            r#"
            [[bindings]]
            control = "pad.1"
            gesture = "tap"
            action = "shell.run"
            params = { program = "cargo", args = ["test", "--all"], quiet = true }
            "#,
        );
        let params = &config.bindings[0].params;
        assert_eq!(params["program"], ParamValue::from("cargo"));
        assert_eq!(params["quiet"], ParamValue::Flag(true));
        assert!(matches!(params["args"], ParamValue::List(ref values) if values.len() == 2));
    }

    #[test]
    fn a_misspelled_key_is_rejected_rather_than_ignored() {
        let result: Result<ConfigFile, _> = toml::from_str(
            r#"
            [[bindings]]
            control = "pad.1"
            gesture = "tap"
            action = "page.next"
            labell = "typo"
            "#,
        );
        assert!(
            result.is_err(),
            "an unknown key must not be silently dropped"
        );
    }

    #[test]
    fn permissions_use_their_dotted_names() {
        let config = parse(
            r#"
            [permissions]
            granted = ["media.control", "shell.execute"]
            "#,
        );
        assert_eq!(
            config.permissions.granted,
            [Permission::MediaControl, Permission::ShellExecute]
        );
    }

    #[test]
    fn an_unknown_permission_is_rejected() {
        let result: Result<ConfigFile, _> =
            toml::from_str("[permissions]\ngranted = [\"everything\"]\n");
        assert!(result.is_err());
    }

    #[test]
    fn merging_accumulates_pages_and_bindings_but_overwrites_settings() {
        let mut first = parse(
            r#"
            [runtime]
            home_page = "home"

            [[pages]]
            id = "home"
            name = "Home"
            "#,
        );
        let second = parse(
            r#"
            [runtime]
            home_page = "development"

            [[pages]]
            id = "development"
            name = "Development"
            "#,
        );

        first.merge(second);
        assert_eq!(first.pages.len(), 2);
        assert_eq!(first.runtime.home_page.as_deref(), Some("development"));
    }

    #[test]
    fn merging_leaves_a_setting_alone_when_the_other_file_omits_it() {
        let mut first = parse("[runtime]\nhome_page = \"home\"\n");
        first.merge(parse("[runtime]\nmedia_player = \"Music\"\n"));

        assert_eq!(first.runtime.home_page.as_deref(), Some("home"));
        assert_eq!(first.runtime.media_player.as_deref(), Some("Music"));
    }
}
