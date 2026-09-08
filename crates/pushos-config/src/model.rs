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
}

impl RuntimeSection {
    fn merge(&mut self, other: Self) {
        if other.home_page.is_some() {
            self.home_page = other.home_page;
        }
        if other.media_player.is_some() {
            self.media_player = other.media_player;
        }
    }
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
