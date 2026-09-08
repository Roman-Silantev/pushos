//! What the operator said, and what PushOS does about it.
//!
//! Voice on a physical surface is not a wake word and a guess. A control is
//! held, which is what starts listening, and released, which is what stops it.
//! The operator always knows whether PushOS is listening, because their finger
//! is on the thing that makes it listen.
//!
//! What comes back is routed in two layers. A short deterministic phrase means
//! exactly one thing and runs it. Anything else is a request, and goes to
//! whichever agent the operator was already working with.

use std::fmt;
use std::time::Duration;

use crate::action::ActionDefinition;

/// What was heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Utterance {
    /// The words, as transcribed.
    pub text: String,
    /// How sure the transcriber was, where it says.
    ///
    /// Between zero and one. Absent when the engine does not report one, which
    /// is not the same as being certain.
    pub confidence: Option<f32>,
    /// How long the operator held the control.
    pub held_for: Duration,
}

impl Utterance {
    /// Builds an utterance with no confidence reported.
    pub fn new(text: impl Into<String>, held_for: Duration) -> Self {
        Self {
            text: text.into(),
            confidence: None,
            held_for,
        }
    }

    /// Whether anything was actually said.
    ///
    /// A held control that produced nothing is the common case of a mis-press,
    /// and running something on an empty transcript would be the worst possible
    /// answer to it.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The words, reduced to what matching cares about.
    pub fn normalised(&self) -> String {
        normalise(&self.text)
    }
}

/// Reduces words to what matching cares about.
///
/// Lower case, no punctuation, single spaces. "Approve." and "approve" are the
/// same instruction, and an operator should not have to know that PushOS thinks
/// otherwise. Configured phrases go through this too, so a phrase written with
/// a capital or a comma still matches what was heard.
pub fn normalise(text: &str) -> String {
    let mut words = String::with_capacity(text.len());
    let mut spaced = true;

    for character in text.chars() {
        if character.is_alphanumeric() {
            words.extend(character.to_lowercase());
            spaced = false;
        } else if !spaced {
            words.push(' ');
            spaced = true;
        }
    }

    words.trim_end().to_owned()
}

/// A phrase that means exactly one thing.
///
/// The first layer of routing. These are short, they are configured, and they
/// never reach an agent: "stop" must stop, immediately, whatever else is going
/// on.
#[derive(Clone, Debug, PartialEq)]
pub struct SpokenCommand {
    /// What the operator says, already normalised.
    pub phrase: String,
    /// What it runs.
    pub action: ActionDefinition,
    /// Whether it needs a deliberate press before it happens.
    ///
    /// Transcription is not authorisation. Anything that deploys, merges,
    /// deletes or sends should be confirmed with a finger, not with a word.
    pub confirm: bool,
}

/// Where an utterance was sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Routing {
    /// It matched a configured phrase and ran it.
    Command {
        /// The phrase that matched.
        phrase: String,
        /// What it runs.
        action: Box<ActionDefinition>,
    },
    /// It matched a phrase that will not happen without a press.
    NeedsConfirming {
        /// The phrase that matched.
        phrase: String,
        /// What it would run.
        action: Box<ActionDefinition>,
    },
    /// It was a request, and went to an agent.
    Request {
        /// What was asked.
        text: String,
    },
    /// Nothing was said.
    Nothing,
}

impl fmt::Display for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command { phrase, .. } => write!(f, "{phrase}"),
            Self::NeedsConfirming { phrase, .. } => write!(f, "{phrase}?"),
            Self::Request { text } => f.write_str(text),
            Self::Nothing => f.write_str("nothing heard"),
        }
    }
}

/// Which engine works out what was said.
///
/// Both run on this Mac and neither sends audio anywhere. They differ in what
/// they cost the operator to set up: one is already installed, the other is a
/// file they have to fetch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VoiceEngine {
    /// macOS's own Speech framework, with on-device recognition required.
    ///
    /// The default because it is already there. Nothing to download, no model
    /// to keep, and it knows sixty-odd languages out of the box.
    #[default]
    Apple,
    /// Whisper, run locally through Metal.
    ///
    /// For an operator who wants a model they chose rather than the one Apple
    /// ships. Costs a download and the disk to keep it on.
    Whisper,
}

impl VoiceEngine {
    /// How it is written in configuration.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Whisper => "whisper",
        }
    }

    /// Whether it needs a model file the operator has to supply.
    pub const fn needs_model(self) -> bool {
        matches!(self, Self::Whisper)
    }
}

impl fmt::Display for VoiceEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for VoiceEngine {
    type Err = UnknownEngine;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_lowercase().as_str() {
            "apple" | "macos" | "system" => Ok(Self::Apple),
            "whisper" => Ok(Self::Whisper),
            _ => Err(UnknownEngine {
                named: text.to_owned(),
            }),
        }
    }
}

/// A configuration named an engine PushOS does not have.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{named}` is not a speech engine; PushOS has `apple` and `whisper`")]
pub struct UnknownEngine {
    /// What the configuration said.
    pub named: String,
}

/// Whether PushOS is listening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Listening {
    /// Not listening. Nothing is being recorded.
    #[default]
    Idle,
    /// The control is held and audio is being kept.
    Recording,
    /// The control was released and the words are being worked out.
    Transcribing,
}

impl Listening {
    /// The light that communicates this state.
    pub const fn status_color(self) -> crate::color::StatusColor {
        use crate::color::StatusColor;
        match self {
            Self::Idle => StatusColor::Idle,
            // The one that matters: an operator must never be unsure whether
            // the microphone is on.
            Self::Recording => StatusColor::Waiting,
            Self::Transcribing => StatusColor::Working,
        }
    }

    /// How it reads in the status bar, where there is room for a word.
    ///
    /// Nothing when idle: a badge that was always there would stop being read,
    /// and this is the one an operator has to notice.
    pub const fn badge(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Recording => Some("LISTENING"),
            Self::Transcribing => Some("HEARD YOU"),
        }
    }

    /// How it reads on the display.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Recording => "Listening",
            Self::Transcribing => "Working out what you said",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Utterance {
        Utterance::new(text, Duration::from_secs(1))
    }

    #[test]
    fn punctuation_and_case_do_not_change_what_was_meant() {
        // An operator should not have to know how the transcriber punctuates.
        assert_eq!(said("Approve.").normalised(), "approve");
        assert_eq!(said("  NEXT   page!  ").normalised(), "next page");
        assert_eq!(said("Stop, now").normalised(), "stop now");
    }

    #[test]
    fn a_held_control_that_produced_nothing_is_empty() {
        // The common mis-press. Running something on this would be the worst
        // possible answer to it.
        assert!(said("").is_empty());
        assert!(said("   ").is_empty());
        assert!(!said("stop").is_empty());
    }

    #[test]
    fn normalising_nothing_produces_nothing() {
        assert_eq!(said("...").normalised(), "");
        assert_eq!(said("").normalised(), "");
    }

    #[test]
    fn numbers_survive_normalising() {
        // "page 2" is a real command, and dropping the 2 would make it another.
        assert_eq!(said("Page 2").normalised(), "page 2");
    }

    #[test]
    fn an_utterance_reports_no_confidence_rather_than_pretending_to_be_sure() {
        assert_eq!(said("stop").confidence, None);
    }

    #[test]
    fn recording_and_idle_never_look_the_same() {
        // If these could be confused, an operator could not tell whether the
        // microphone was on, which is the one thing they must always know.
        assert_ne!(
            Listening::Recording.status_color(),
            Listening::Idle.status_color()
        );
        assert!(!Listening::Recording.describe().is_empty());
        assert!(Listening::Idle.describe().is_empty());
        assert!(Listening::Recording.badge().is_some());
        assert!(
            Listening::Idle.badge().is_none(),
            "a badge that is always there stops being read"
        );
    }

    #[test]
    fn an_engine_survives_being_written_down_and_read_back() {
        for engine in [VoiceEngine::Apple, VoiceEngine::Whisper] {
            assert_eq!(engine.as_str().parse(), Ok(engine));
        }
    }

    #[test]
    fn the_engine_that_needs_no_setup_is_the_default() {
        // An operator who writes nothing gets the one already installed.
        assert_eq!(VoiceEngine::default(), VoiceEngine::Apple);
        assert!(!VoiceEngine::default().needs_model());
        assert!(VoiceEngine::Whisper.needs_model());
    }

    #[test]
    fn an_engine_nobody_has_is_reported_by_name() {
        let error = "vosk".parse::<VoiceEngine>().expect_err("not an engine");
        assert!(error.to_string().contains("vosk"));
        assert!(error.to_string().contains("whisper"));
    }

    #[test]
    fn routing_says_what_happened_in_words() {
        assert_eq!(Routing::Nothing.to_string(), "nothing heard");
        assert_eq!(
            Routing::Request {
                text: "carry on".to_owned()
            }
            .to_string(),
            "carry on"
        );
    }
}
