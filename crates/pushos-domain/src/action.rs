//! The action model.
//!
//! An action is addressed as `provider.verb`. Core never enumerates verbs, so a
//! new provider can be registered without touching the binding resolver, the
//! renderer or the hardware adapter.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{ActionVerb, CorrelationId, ExecutionId, ProviderName};

/// Addresses one action within one provider namespace.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActionSelector {
    /// The provider that owns the verb, such as `media`.
    pub provider: ProviderName,
    /// The verb itself, such as `play_pause`.
    pub verb: ActionVerb,
}

impl ActionSelector {
    /// Builds a selector from its parts.
    pub fn new(provider: impl Into<ProviderName>, verb: impl Into<ActionVerb>) -> Self {
        Self {
            provider: provider.into(),
            verb: verb.into(),
        }
    }
}

impl fmt::Display for ActionSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.provider, self.verb)
    }
}

impl fmt::Debug for ActionSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for ActionSelector {
    type Err = MalformedSelector;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, verb) = s
            .split_once('.')
            .ok_or_else(|| MalformedSelector(s.to_owned()))?;
        if provider.is_empty() || verb.is_empty() {
            return Err(MalformedSelector(s.to_owned()));
        }
        Ok(Self::new(provider, verb))
    }
}

impl TryFrom<String> for ActionSelector {
    type Error = MalformedSelector;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ActionSelector> for String {
    fn from(value: ActionSelector) -> Self {
        value.to_string()
    }
}

/// An action name that is not of the form `provider.verb`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("action `{0}` must be written as `provider.verb`")]
pub struct MalformedSelector(pub String);

/// A configured parameter value.
///
/// Deliberately small: parameters describe intent, not program state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// Textual value.
    Text(Arc<str>),
    /// Whole number.
    Integer(i64),
    /// Fractional number.
    Number(f64),
    /// Flag.
    Flag(bool),
    /// Ordered list of values.
    List(Vec<ParamValue>),
}

impl ParamValue {
    /// Borrows the value as text, if it is textual.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    /// Reads the value as a whole number.
    ///
    /// Text that is a whole number counts. Configuration is written by hand,
    /// `percent = "20"` is what a person types as often as `percent = 20`, and
    /// refusing the quoted one at the moment a pad is pressed would be pedantry
    /// about something PushOS can read perfectly well.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Text(value) => value.trim().parse().ok(),
            _ => None,
        }
    }

    /// Reads the value as a flag.
    ///
    /// Text counts, for the same reason.
    pub fn as_flag(&self) -> Option<bool> {
        match self {
            Self::Flag(value) => Some(*value),
            Self::Text(value) => match value.trim().to_lowercase().as_str() {
                "true" | "yes" | "on" => Some(true),
                "false" | "no" | "off" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }
}

impl From<&str> for ParamValue {
    fn from(value: &str) -> Self {
        Self::Text(Arc::from(value))
    }
}

/// The parameters attached to one action definition.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Params(BTreeMap<String, ParamValue>);

impl Params {
    /// An empty parameter set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks a parameter up by name.
    pub fn get(&self, key: &str) -> Option<&ParamValue> {
        self.0.get(key)
    }

    /// Reads a required textual parameter.
    pub fn require_text(&self, key: &str) -> Result<&str, MissingParam> {
        self.get(key)
            .and_then(ParamValue::as_text)
            .ok_or_else(|| MissingParam {
                key: key.to_owned(),
                expected: "text",
            })
    }

    /// Reads an optional textual parameter.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ParamValue::as_text)
    }

    /// Reads an optional list of strings, accepting a bare string as one item.
    pub fn text_list(&self, key: &str) -> Option<Vec<&str>> {
        match self.get(key)? {
            ParamValue::Text(value) => Some(vec![value.as_ref()]),
            ParamValue::List(values) => values.iter().map(ParamValue::as_text).collect(),
            _ => None,
        }
    }

    /// Inserts or replaces a parameter.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<ParamValue>) {
        self.0.insert(key.into(), value.into());
    }

    /// Whether any parameters are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates the parameters in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }
}

impl FromIterator<(String, ParamValue)> for Params {
    fn from_iter<T: IntoIterator<Item = (String, ParamValue)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// A required parameter that a provider could not find.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("missing required {expected} parameter `{key}`")]
pub struct MissingParam {
    /// The parameter name.
    pub key: String,
    /// The type the provider expected.
    pub expected: &'static str,
}

/// A fully resolved instruction, ready for a provider to execute.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionDefinition {
    /// Which provider and verb to invoke.
    pub selector: ActionSelector,
    /// The arguments for the verb.
    pub params: Params,
}

impl ActionDefinition {
    /// Builds a definition.
    pub fn new(selector: ActionSelector, params: Params) -> Self {
        Self { selector, params }
    }

    /// Builds a definition with no parameters.
    pub fn bare(selector: ActionSelector) -> Self {
        Self::new(selector, Params::new())
    }
}

/// How an action finished, or how far it got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    /// Accepted and running; completion arrives later as an event.
    Started,
    /// Finished successfully.
    Completed,
    /// Blocked on the operator.
    Waiting,
    /// Finished unsuccessfully.
    Failed,
    /// Stopped before finishing.
    Cancelled,
}

impl ActionStatus {
    /// Whether the action has reached a state it will not leave on its own.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// The light that communicates this status.
    pub const fn status_color(self) -> crate::color::StatusColor {
        match self {
            Self::Started => crate::color::StatusColor::Working,
            Self::Completed => crate::color::StatusColor::Complete,
            Self::Waiting => crate::color::StatusColor::Waiting,
            Self::Failed => crate::color::StatusColor::Failed,
            Self::Cancelled => crate::color::StatusColor::Idle,
        }
    }
}

/// What an action asks the display to show, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayIntent {
    /// Show a transient banner, then restore the previous view.
    Toast {
        /// Headline text.
        title: String,
        /// Optional supporting line.
        detail: Option<String>,
    },
    /// Move to a page.
    Page(crate::page::PageTarget),
    /// Offer the operator a choice. Never steals focus on its own.
    Prompt {
        /// What is being asked.
        question: String,
        /// The available answers, in display order.
        choices: Vec<String>,
    },
    /// Show one thing across the whole panel, until the operator looks away.
    ///
    /// Distinct from a report in the one way that matters: it does not go away
    /// on its own. Somebody who asked to look closely at one of eight things
    /// is reading, and a panel that reverted underneath them would be one they
    /// could not use.
    Focus {
        /// What it is, above the name.
        kind: String,
        /// What it is called.
        title: String,
        /// What it is doing, in a word.
        state: String,
        /// What it last said, most recent last.
        lines: Vec<String>,
    },
    /// Show a list of things the operator asked for.
    ///
    /// Distinct from a prompt: these are results to read, not answers to
    /// choose between, and nothing is waiting on them.
    Report {
        /// A short label above the headline, such as what was searched for.
        kind: String,
        /// The headline.
        title: String,
        /// The list, in the order it should be read.
        lines: Vec<String>,
    },
}

/// The normalised outcome every action returns.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionResult {
    /// How the action finished, or how far it got.
    pub status: ActionStatus,
    /// A short human-readable note.
    pub message: Option<String>,
    /// What the display should do about it.
    pub display: Option<DisplayIntent>,
}

impl ActionResult {
    /// A successful result with no display change.
    pub const fn completed() -> Self {
        Self {
            status: ActionStatus::Completed,
            message: None,
            display: None,
        }
    }

    /// An accepted result whose work continues in the background.
    pub const fn started() -> Self {
        Self {
            status: ActionStatus::Started,
            message: None,
            display: None,
        }
    }

    /// Attaches a human-readable note.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attaches a display instruction.
    #[must_use]
    pub fn with_display(mut self, display: DisplayIntent) -> Self {
        self.display = Some(display);
        self
    }
}

/// Everything a provider is given when it executes.
#[derive(Clone, Debug)]
pub struct ActionContext {
    /// The instruction to carry out.
    pub definition: ActionDefinition,
    /// Ties this execution to the gesture that caused it.
    pub correlation_id: CorrelationId,
    /// Deduplicates externally visible effects across retries.
    pub execution_id: ExecutionId,
    /// The surface state at the moment the gesture was resolved.
    pub surface: crate::context::SurfaceContext,
}

impl ActionContext {
    /// Builds a context for a fresh execution.
    pub fn new(
        definition: ActionDefinition,
        correlation_id: CorrelationId,
        surface: crate::context::SurfaceContext,
    ) -> Self {
        Self {
            definition,
            correlation_id,
            execution_id: ExecutionId::generate(),
            surface,
        }
    }

    /// Shorthand for the action's parameters.
    pub fn params(&self) -> &Params {
        &self.definition.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_written_with_quotes_round_it_is_still_a_number() {
        // `percent = "20"` is what a person types as often as `percent = 20`,
        // and finding out otherwise when they press the pad is no use.
        assert_eq!(ParamValue::Text("20".into()).as_integer(), Some(20));
        assert_eq!(ParamValue::Text(" -3 ".into()).as_integer(), Some(-3));
        assert_eq!(ParamValue::Integer(20).as_integer(), Some(20));
    }

    #[test]
    fn text_that_is_not_a_number_is_still_not_a_number() {
        assert_eq!(ParamValue::Text("loud".into()).as_integer(), None);
        assert_eq!(ParamValue::Text("2.5".into()).as_integer(), None);
        assert_eq!(ParamValue::Text(String::new().into()).as_integer(), None);
    }

    #[test]
    fn a_flag_may_be_written_the_way_people_write_flags() {
        for (written, expected) in [
            ("true", true),
            ("Yes", true),
            ("on", true),
            ("false", false),
            ("NO", false),
            ("off", false),
        ] {
            assert_eq!(
                ParamValue::Text(written.into()).as_flag(),
                Some(expected),
                "`{written}`"
            );
        }
        assert_eq!(ParamValue::Text("maybe".into()).as_flag(), None);
    }

    #[test]
    fn selectors_parse_and_render_symmetrically() {
        let selector: ActionSelector = "media.play_pause".parse().expect("well-formed selector");
        assert_eq!(selector.provider.as_str(), "media");
        assert_eq!(selector.verb.as_str(), "play_pause");
        assert_eq!(selector.to_string(), "media.play_pause");
    }

    #[test]
    fn selectors_without_both_halves_are_rejected() {
        for candidate in ["media", "media.", ".play_pause", ""] {
            assert!(
                candidate.parse::<ActionSelector>().is_err(),
                "`{candidate}` should not parse as a selector"
            );
        }
    }

    #[test]
    fn a_verb_may_contain_further_dots() {
        let selector: ActionSelector = "agent.session.resume".parse().expect("splits once");
        assert_eq!(selector.provider.as_str(), "agent");
        assert_eq!(selector.verb.as_str(), "session.resume");
    }

    #[test]
    fn required_parameters_report_their_own_name() {
        let params = Params::new();
        let error = params
            .require_text("target")
            .expect_err("parameter is absent");
        assert_eq!(error.key, "target");
    }

    #[test]
    fn a_bare_string_reads_as_a_single_item_list() {
        let mut params = Params::new();
        params.set("args", ParamValue::from("test"));
        assert_eq!(params.text_list("args"), Some(vec!["test"]));
    }

    #[test]
    fn only_settled_statuses_are_terminal() {
        assert!(ActionStatus::Completed.is_terminal());
        assert!(ActionStatus::Failed.is_terminal());
        assert!(ActionStatus::Cancelled.is_terminal());
        assert!(!ActionStatus::Started.is_terminal());
        assert!(!ActionStatus::Waiting.is_terminal());
    }
}
