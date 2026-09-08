//! Where what was said goes.
//!
//! Two layers, and the order matters. A short configured phrase means exactly
//! one thing and runs it, because "stop" must stop whatever else is going on.
//! Anything that is not one of those is a request, and goes to whichever agent
//! the operator was already working with.
//!
//! Nothing here guesses. A phrase either matches something the operator wrote
//! down or it does not, and the second layer is a deliberate fallback rather
//! than a interpretation.

use pushos_domain::voice::{Routing, SpokenCommand, Utterance};

/// The phrases PushOS knows, in the order they were configured.
#[derive(Debug, Default)]
pub struct VoiceRouter {
    commands: Vec<SpokenCommand>,
}

impl VoiceRouter {
    /// Builds a router over the configured phrases.
    ///
    /// A repeated phrase keeps the first, so an operator who wrote one twice
    /// gets the one they read first rather than a coin toss.
    pub fn new(commands: impl IntoIterator<Item = SpokenCommand>) -> Self {
        let mut kept: Vec<SpokenCommand> = Vec::new();
        for command in commands {
            if !kept
                .iter()
                .any(|existing| existing.phrase == command.phrase)
            {
                kept.push(command);
            }
        }
        Self { commands: kept }
    }

    /// Every phrase, in configured order.
    pub fn commands(&self) -> &[SpokenCommand] {
        &self.commands
    }

    /// Whether anything is configured.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Works out what to do with what was said.
    pub fn route(&self, said: &Utterance) -> Routing {
        if said.is_empty() {
            return Routing::Nothing;
        }

        let spoken = said.normalised();
        if let Some(command) = self.matching(&spoken) {
            let action = Box::new(command.action.clone());
            return if command.confirm {
                // Transcription is not authorisation. Something irreversible
                // waits for a finger.
                Routing::NeedsConfirming {
                    phrase: command.phrase.clone(),
                    action,
                }
            } else {
                Routing::Command {
                    phrase: command.phrase.clone(),
                    action,
                }
            };
        }

        Routing::Request {
            text: said.text.trim().to_owned(),
        }
    }

    /// The phrase that fits, preferring the most specific.
    ///
    /// A transcriber adds words: "stop" is often heard as "stop." or "stop it".
    /// A phrase therefore matches when the utterance begins with it, and the
    /// longest such phrase wins, so "stop music" is never taken for "stop".
    fn matching(&self, spoken: &str) -> Option<&SpokenCommand> {
        self.commands
            .iter()
            .filter(|command| begins_with_phrase(spoken, &command.phrase))
            .max_by_key(|command| command.phrase.len())
    }
}

/// Whether an utterance begins with a phrase, on a word boundary.
///
/// The boundary matters: "approve" must not match "approved the plan", which is
/// something an operator is describing rather than asking for.
fn begins_with_phrase(spoken: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let Some(rest) = spoken.strip_prefix(phrase) else {
        return false;
    };
    rest.is_empty() || rest.starts_with(' ')
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pushos_domain::action::{ActionDefinition, ActionSelector, Params};

    use super::*;

    fn command(phrase: &str, action: &str, confirm: bool) -> SpokenCommand {
        let selector: ActionSelector = action.parse().expect("a valid selector");
        SpokenCommand {
            phrase: phrase.to_owned(),
            action: ActionDefinition::new(selector, Params::new()),
            confirm,
        }
    }

    fn router() -> VoiceRouter {
        VoiceRouter::new([
            command("stop", "agent.cancel", false),
            command("stop music", "media.play_pause", false),
            command("approve", "agent.approve", false),
            command("next page", "page.next", false),
            command("deploy", "shortcut.run", true),
        ])
    }

    fn said(text: &str) -> Utterance {
        Utterance::new(text, Duration::from_secs(1))
    }

    #[test]
    fn a_configured_phrase_runs_rather_than_reaching_an_agent() {
        // "stop" must stop, whatever else is going on.
        let routed = router().route(&said("Stop."));
        let Routing::Command { phrase, .. } = routed else {
            panic!("expected a command, got {routed:?}");
        };
        assert_eq!(phrase, "stop");
    }

    #[test]
    fn the_longer_phrase_wins_so_stop_music_is_never_taken_for_stop() {
        let routed = router().route(&said("stop music"));
        let Routing::Command { phrase, .. } = routed else {
            panic!("expected a command, got {routed:?}");
        };
        assert_eq!(phrase, "stop music");
    }

    #[test]
    fn a_transcriber_adding_words_does_not_stop_a_phrase_matching() {
        // "stop" comes back as "stop it" and "stop." often enough that an exact
        // match would make the feature feel broken.
        for heard in ["stop it", "Stop!", "stop, please"] {
            assert!(
                matches!(router().route(&said(heard)), Routing::Command { .. }),
                "`{heard}` should still stop"
            );
        }
    }

    #[test]
    fn a_phrase_inside_a_longer_word_does_not_match() {
        // "approved the plan" is something being described, not asked for.
        let routed = router().route(&said("approved the plan yesterday"));
        assert!(matches!(routed, Routing::Request { .. }), "{routed:?}");
    }

    #[test]
    fn anything_that_is_not_a_phrase_becomes_a_request() {
        let routed = router().route(&said("Ask Codex to review what Claude changed"));
        let Routing::Request { text } = routed else {
            panic!("expected a request");
        };
        assert_eq!(text, "Ask Codex to review what Claude changed");
    }

    #[test]
    fn a_request_keeps_the_words_as_they_were_said() {
        // The agent gets the sentence, not the flattened form matching used.
        let routed = router().route(&said("  Tell Claude to continue authentication.  "));
        let Routing::Request { text } = routed else {
            panic!("expected a request");
        };
        assert_eq!(text, "Tell Claude to continue authentication.");
    }

    #[test]
    fn something_irreversible_waits_for_a_finger() {
        // Transcription is not authorisation.
        let routed = router().route(&said("deploy"));
        assert!(
            matches!(routed, Routing::NeedsConfirming { .. }),
            "{routed:?}"
        );
    }

    #[test]
    fn a_held_control_that_produced_nothing_does_nothing() {
        assert_eq!(router().route(&said("")), Routing::Nothing);
        assert_eq!(router().route(&said("   ")), Routing::Nothing);
    }

    #[test]
    fn with_no_phrases_configured_everything_is_a_request() {
        let empty = VoiceRouter::new([]);
        assert!(empty.is_empty());
        assert!(matches!(
            empty.route(&said("stop")),
            Routing::Request { .. }
        ));
    }

    #[test]
    fn a_phrase_written_twice_keeps_the_first() {
        let repeated = VoiceRouter::new([
            command("stop", "agent.cancel", false),
            command("stop", "media.play_pause", false),
        ]);
        assert_eq!(repeated.commands().len(), 1);

        let Routing::Command { action, .. } = repeated.route(&said("stop")) else {
            panic!("expected a command");
        };
        assert_eq!(action.selector.to_string(), "agent.cancel");
    }
}
