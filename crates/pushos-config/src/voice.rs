//! Turning the `[voice]` section into something the runtime can use.
//!
//! Kept apart from the rest of the build because voice has one rule the others
//! do not: a phrase is matched against speech, so what an operator writes has
//! to be reduced the same way what they say is. "Approve," and "approve" must
//! be the same instruction, and the operator should never have to know it.

use std::collections::HashSet;
use std::path::PathBuf;

use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
use pushos_domain::voice::{SpokenCommand, VoiceEngine, normalise};

use crate::error::Problem;
use crate::model::{ConfigFile, VoiceCommandEntry};

/// Push to talk, as the runtime sees it.
#[derive(Debug)]
pub struct VoiceSettings {
    /// Which engine works out what was said.
    pub engine: VoiceEngine,
    /// The model file, for engines that need one.
    pub model: Option<PathBuf>,
    /// What anything unrecognised runs, with the words as `text`.
    ///
    /// Absent means an unrecognised phrase is reported and nothing else, which
    /// is the right default: PushOS should not invent somewhere to send words.
    pub request: Option<ActionSelector>,
    /// The phrases that mean exactly one thing, already normalised.
    pub commands: Vec<SpokenCommand>,
}

impl VoiceSettings {
    /// Whether anything can actually come of speaking.
    ///
    /// A section naming only an engine listens and then has nowhere to put what
    /// it heard, which is worth knowing before the light goes on.
    pub fn is_useful(&self) -> bool {
        self.request.is_some() || !self.commands.is_empty()
    }
}

/// Builds the voice settings, or nothing when voice is not configured.
pub(crate) fn build(file: &ConfigFile, problems: &mut Vec<Problem>) -> Option<VoiceSettings> {
    let section = &file.voice;
    if section.engine.is_none()
        && section.model.is_none()
        && section.request.is_none()
        && section.commands.is_empty()
    {
        return None;
    }

    let engine = match section.engine.as_deref() {
        None => VoiceEngine::default(),
        Some(named) => match named.parse::<VoiceEngine>() {
            Ok(engine) => engine,
            Err(source) => {
                problems.push(Problem::VoiceEngine { source });
                return None;
            }
        },
    };

    let model = section.model.as_deref().map(crate::paths::expand_home);
    if engine.needs_model() && model.is_none() {
        problems.push(Problem::VoiceModelMissing {
            engine: engine.to_string(),
        });
    }

    let request = match section.request.as_deref() {
        None => None,
        Some(written) => match written.parse::<ActionSelector>() {
            Ok(selector) => Some(selector),
            Err(source) => {
                problems.push(Problem::VoiceRequestAction { source });
                None
            }
        },
    };

    let commands = build_commands(&section.commands, problems);

    Some(VoiceSettings {
        engine,
        model,
        request,
        commands,
    })
}

/// Builds the phrases, reporting each one that cannot be used.
fn build_commands(
    entries: &[VoiceCommandEntry],
    problems: &mut Vec<Problem>,
) -> Vec<SpokenCommand> {
    let mut seen: HashSet<String> = HashSet::with_capacity(entries.len());
    let mut commands = Vec::with_capacity(entries.len());

    for entry in entries {
        let phrase = normalise(&entry.phrase);
        if phrase.is_empty() {
            problems.push(Problem::VoicePhraseEmpty {
                written: entry.phrase.clone(),
            });
            continue;
        }
        if !seen.insert(phrase.clone()) {
            // Two spellings of the same phrase reduce to one thing, and which
            // of them wins would be invisible in the file.
            problems.push(Problem::DuplicateVoicePhrase {
                phrase: phrase.clone(),
            });
            continue;
        }

        let selector = match entry.action.parse::<ActionSelector>() {
            Ok(selector) => selector,
            Err(source) => {
                problems.push(Problem::VoiceAction {
                    phrase: phrase.clone(),
                    source,
                });
                continue;
            }
        };

        let mut params: Params = entry
            .params
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if let Some(target) = &entry.target {
            params.set("target", ParamValue::Text(target.as_str().into()));
        }

        commands.push(SpokenCommand {
            phrase,
            action: ActionDefinition::new(selector, params),
            confirm: entry.confirm,
        });
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VoiceSection;

    fn command(phrase: &str, action: &str) -> VoiceCommandEntry {
        VoiceCommandEntry {
            phrase: phrase.to_owned(),
            action: action.to_owned(),
            target: None,
            params: std::collections::BTreeMap::new(),
            confirm: false,
        }
    }

    fn built(section: VoiceSection) -> (Option<VoiceSettings>, Vec<Problem>) {
        let file = ConfigFile {
            voice: section,
            ..ConfigFile::default()
        };
        let mut problems = Vec::new();
        let settings = build(&file, &mut problems);
        (settings, problems)
    }

    #[test]
    fn no_voice_section_means_voice_is_off() {
        let (settings, problems) = built(VoiceSection::default());
        assert!(settings.is_none(), "voice is opt-in");
        assert!(problems.is_empty());
    }

    #[test]
    fn the_engine_defaults_to_the_one_already_installed() {
        let (settings, problems) = built(VoiceSection {
            commands: vec![command("stop", "agent.stop")],
            ..VoiceSection::default()
        });
        let settings = settings.expect("a command turns voice on");
        assert!(problems.is_empty());
        assert_eq!(settings.engine, VoiceEngine::Apple);
        assert!(settings.model.is_none());
    }

    #[test]
    fn a_phrase_is_reduced_the_same_way_speech_is() {
        // Otherwise a phrase written with a capital or a comma would never
        // match anything the operator said.
        let (settings, problems) = built(VoiceSection {
            commands: vec![command("  Stop, Now! ", "agent.stop")],
            ..VoiceSection::default()
        });
        assert!(problems.is_empty());
        let settings = settings.expect("configured");
        assert_eq!(settings.commands[0].phrase, "stop now");
    }

    #[test]
    fn two_spellings_of_one_phrase_are_reported_rather_than_silently_ranked() {
        let (_, problems) = built(VoiceSection {
            commands: vec![
                command("Stop.", "agent.stop"),
                command("stop", "media.stop"),
            ],
            ..VoiceSection::default()
        });
        assert!(
            problems
                .iter()
                .any(|problem| problem.to_string().contains("stop")),
            "the operator cannot see which one wins, so PushOS must not pick"
        );
    }

    #[test]
    fn a_phrase_with_no_words_in_it_is_rejected() {
        let (_, problems) = built(VoiceSection {
            commands: vec![command("...", "agent.stop")],
            ..VoiceSection::default()
        });
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_malformed_action_names_the_phrase_that_had_it() {
        let (_, problems) = built(VoiceSection {
            commands: vec![command("stop", "stop")],
            ..VoiceSection::default()
        });
        let message = problems.first().expect("one problem").to_string();
        assert!(message.contains("stop"));
    }

    #[test]
    fn whisper_without_a_model_is_reported_before_anything_starts() {
        let (_, problems) = built(VoiceSection {
            engine: Some("whisper".to_owned()),
            commands: vec![command("stop", "agent.stop")],
            ..VoiceSection::default()
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].to_string().contains("model"));
    }

    #[test]
    fn an_engine_nobody_has_stops_the_section_rather_than_falling_back() {
        // Falling back would listen with an engine the operator did not ask
        // for, which is worse than saying so.
        let (settings, problems) = built(VoiceSection {
            engine: Some("vosk".to_owned()),
            ..VoiceSection::default()
        });
        assert!(settings.is_none());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_target_becomes_a_parameter_like_it_does_for_a_binding() {
        let (settings, _) = built(VoiceSection {
            commands: vec![VoiceCommandEntry {
                target: Some("deploy".to_owned()),
                confirm: true,
                ..command("ship it", "workflow.start")
            }],
            ..VoiceSection::default()
        });
        let settings = settings.expect("configured");
        assert!(settings.commands[0].confirm);
        assert_eq!(
            settings.commands[0].action.params.text("target"),
            Some("deploy")
        );
    }

    #[test]
    fn listening_with_nowhere_to_put_the_words_is_visible() {
        let (settings, _) = built(VoiceSection {
            engine: Some("apple".to_owned()),
            ..VoiceSection::default()
        });
        let settings = settings.expect("the engine was named");
        assert!(!settings.is_useful());
    }
}
