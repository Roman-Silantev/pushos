//! The description of a binding that an editor works with.
//!
//! Deliberately separate from the domain's [`pushos_domain::binding::Binding`]:
//! this is what an editing tool sends and receives, and it names a binding the
//! way the configuration file does, before anything has been resolved.

use std::collections::BTreeMap;

use pushos_domain::action::ParamValue;
use serde::{Deserialize, Serialize};

/// Identifies a binding the way a person would point at one.
///
/// Control, gesture and scope together are exactly what precedence considers,
/// so two bindings sharing a key are the ambiguity the validator refuses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BindingAddress {
    /// The control, such as `pad.23`.
    pub control: String,
    /// The gesture, such as `hold`.
    pub gesture: String,
    /// The page it is restricted to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// The workspace it is restricted to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

impl BindingAddress {
    /// Builds a global address.
    pub fn new(control: impl Into<String>, gesture: impl Into<String>) -> Self {
        Self {
            control: control.into(),
            gesture: gesture.into(),
            page: None,
            workspace: None,
        }
    }

    /// Restricts the address to a page.
    #[must_use]
    pub fn on_page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Restricts the address to a workspace.
    #[must_use]
    pub fn in_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Whether this address points at the same binding as another.
    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }
}

/// A binding as an editing tool describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BindingSpec {
    /// Where the binding lives.
    #[serde(flatten)]
    pub address: BindingAddress,
    /// The action, written as `provider.verb`.
    pub action: String,
    /// Shorthand for the `target` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Everything else the action needs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamValue>,
    /// The caption shown on the display and in Studio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Tie-break within a scope.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u16,
}

impl BindingSpec {
    /// Builds a specification for an action with no parameters.
    pub fn new(address: BindingAddress, action: impl Into<String>) -> Self {
        Self {
            address,
            action: action.into(),
            target: None,
            params: BTreeMap::new(),
            label: None,
            priority: 0,
        }
    }

    /// Sets the `target` shorthand.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Sets the caption.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u16) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_differing_only_in_scope_are_different_bindings() {
        let global = BindingAddress::new("pad.0", "tap");
        let on_page = BindingAddress::new("pad.0", "tap").on_page("music");

        assert!(!global.matches(&on_page));
        assert!(global.matches(&BindingAddress::new("pad.0", "tap")));
    }

    #[test]
    fn a_specification_round_trips_through_json() {
        let spec = BindingSpec::new(
            BindingAddress::new("pad.4", "hold").on_page("development"),
            "shell.run",
        )
        .with_label("Tests");

        let json = serde_json::to_string(&spec).expect("serialisable");
        let parsed: BindingSpec = serde_json::from_str(&json).expect("deserialisable");
        assert_eq!(parsed, spec);
    }

    #[test]
    fn empty_fields_are_left_out_of_the_serialised_form() {
        let spec = BindingSpec::new(BindingAddress::new("pad.0", "tap"), "page.next");
        let fields: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&serde_json::to_string(&spec).expect("serialisable"))
                .expect("an object");

        let mut names: Vec<_> = fields.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["action", "control", "gesture"],
            "absent scope, caption and default priority should not be written out"
        );
    }
}
