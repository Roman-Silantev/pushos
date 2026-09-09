//! The configurations PushOS ships with.
//!
//! A preset is ordinary configuration. There is no separate format, no
//! installer and nothing PushOS does to a preset that an operator could not do
//! by editing the file, which is the whole point: what they start from is
//! something they can read and change.
//!
//! Every preset here is checked by the tests against the providers PushOS
//! actually ships. A pad that names a verb which no longer exists, or an action
//! whose permission was never granted, would fail under a finger rather than in
//! a build, and that is exactly what a starting configuration must never do.

/// One configuration an operator can start from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Preset {
    /// What it is called on the command line.
    pub(crate) name: &'static str,
    /// One line, for the listing.
    pub(crate) summary: &'static str,
    /// The configuration itself.
    pub(crate) body: &'static str,
}

/// The preset a new installation gets when nothing else is asked for.
pub(crate) const DEFAULT: &str = "starter";

/// Every preset PushOS ships, in the order they are listed.
pub(crate) const ALL: &[Preset] = &[
    Preset {
        name: "starter",
        summary: "A guided tour. Everything PushOS can do, most of it commented out.",
        body: include_str!("../../../presets/starter.toml"),
    },
    Preset {
        name: "blank",
        summary: "One page and the transport. The least configuration that still does something.",
        body: include_str!("../../../presets/blank.toml"),
    },
    Preset {
        name: "developer",
        summary: "Editors, terminals, projects and tests, across six pages.",
        body: include_str!("../../../presets/developer.toml"),
    },
    Preset {
        name: "ai-engineer",
        summary: "Agents first: roles, workflows, push to talk and notes.",
        body: include_str!("../../../presets/ai-engineer.toml"),
    },
    Preset {
        name: "sessions",
        summary: "Eight coding sessions on eight pads, with what each last said on screen.",
        body: include_str!("../../../presets/sessions.toml"),
    },
    Preset {
        name: "automation",
        summary: "Apple Shortcuts and applications. No agents, no terminals.",
        body: include_str!("../../../presets/automation.toml"),
    },
    Preset {
        name: "music",
        summary: "Playback, volume and the notes you take while listening.",
        body: include_str!("../../../presets/music.toml"),
    },
];

/// Every commented-out setting and binding in a preset, uncommented.
///
/// A line counts as an example when what it says, with the comment marker
/// removed, is a table header or an assignment. Prose never is, and a rule
/// rather than a list of key names means a new setting cannot be documented
/// without also being checked.
///
/// Used by the tests, so that what a preset documents is held to the same
/// standard as what it enables. A commented example that no longer works is a
/// trap: the operator uncomments it and PushOS refuses to start.
#[cfg(test)]
pub(crate) fn with_every_example_enabled(body: &str) -> String {
    body.lines()
        .map(|line| {
            let Some(uncommented) = line.trim_start().strip_prefix("# ") else {
                return line;
            };
            if looks_like_toml(uncommented) {
                uncommented
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a line is a table header or an assignment.
#[cfg(test)]
fn looks_like_toml(line: &str) -> bool {
    if line.starts_with('[') {
        return true;
    }

    let Some((key, _)) = line.split_once(" = ") else {
        return false;
    };
    !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_alphanumeric() || "_-.".contains(character))
}

/// The preset with this name.
pub(crate) fn find(name: &str) -> Option<&'static Preset> {
    ALL.iter().find(|preset| preset.name == name)
}

/// Every name, for an error message that tells the operator what to try.
pub(crate) fn names() -> String {
    ALL.iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use pushos_config::{ConfigFile, RuntimeConfig};

    use super::*;

    /// Reads a preset, failing with the problems rather than a unit type.
    fn build(preset: &Preset) -> (ConfigFile, RuntimeConfig) {
        read(preset.name, preset.body)
    }

    /// Reads a preset with every commented example enabled.
    ///
    /// What a preset documents is held to the same standard as what it ships:
    /// a commented example that no longer works is a trap, because the operator
    /// uncomments it and PushOS refuses to start.
    fn build_documented(preset: &Preset) -> (ConfigFile, RuntimeConfig) {
        read(preset.name, &with_every_example_enabled(preset.body))
    }

    fn read(name: &str, body: &str) -> (ConfigFile, RuntimeConfig) {
        let parsed: ConfigFile = toml::from_str(body)
            .unwrap_or_else(|error| panic!("`{name}` is not valid TOML: {error}"));

        let config = RuntimeConfig::build(&parsed).unwrap_or_else(|error| {
            let problems: Vec<String> = error.problems().iter().map(ToString::to_string).collect();
            panic!("`{name}` is not a valid configuration: {problems:?}")
        });

        (parsed, config)
    }

    #[test]
    fn every_shipped_preset_is_valid() {
        // Which also proves there are no ambiguous bindings, no page that does
        // not exist and no control PushOS cannot address: the builder refuses
        // all three, and refuses them all at once.
        for preset in ALL {
            build(preset);
            build_documented(preset);
        }
    }

    #[test]
    fn every_shipped_preset_can_actually_be_written() {
        // The catalogue names a file. A preset listed but missing would only be
        // found out by an operator asking for it.
        for preset in ALL {
            assert!(
                !preset.body.trim().is_empty(),
                "`{}` has nothing in it",
                preset.name
            );
            assert!(
                find(preset.name).is_some(),
                "`{}` cannot be looked up by its own name",
                preset.name
            );
        }
        assert!(
            find(DEFAULT).is_some(),
            "`pushos init` has nothing to write"
        );
    }

    #[test]
    fn preset_names_are_unique_and_usable_as_arguments() {
        let mut seen = BTreeSet::new();
        for preset in ALL {
            assert!(
                seen.insert(preset.name),
                "`{}` is listed more than once",
                preset.name
            );
            assert!(
                preset
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "`{}` is awkward to type after --preset",
                preset.name
            );
            assert!(
                !preset.summary.is_empty(),
                "`{}` has nothing to say for itself in the listing",
                preset.name
            );
        }
    }

    #[test]
    fn every_action_a_preset_binds_exists_and_declares_that_verb() {
        // Checked against the providers PushOS actually ships rather than a
        // list written here, so a preset cannot name something that was renamed
        // or removed. A verb that does not exist fails only when the operator
        // finally presses the control.
        for preset in ALL {
            let (parsed, config) = build_documented(preset);
            let shipped = crate::host::shipped_namespaces(&config);

            for action in actions_in(&parsed) {
                let (provider, verb) = action.split_once('.').unwrap_or_else(|| {
                    panic!(
                        "`{}`: `{action}` is not written as provider.verb",
                        preset.name
                    )
                });

                let verbs = shipped.get(provider).unwrap_or_else(|| {
                    panic!(
                        "`{}`: `{action}` names a provider PushOS does not ship",
                        preset.name
                    )
                });
                assert!(
                    verbs.iter().any(|shipped| shipped == verb),
                    "`{}`: `{action}` names a verb `{provider}` does not have",
                    preset.name
                );
            }
        }
    }

    #[test]
    fn every_binding_a_preset_ships_has_the_permission_it_needs() {
        // The guarantee that makes a preset worth shipping: install it and
        // every pad on it works. A binding whose permission was never granted
        // fails under a finger, with a message about a capability the operator
        // never chose to withhold.
        for preset in ALL {
            let (parsed, config) = build(preset);
            let required = crate::host::shipped_permissions(&config);

            // Every place an action can be named, not only bindings. A step
            // of a sequence is dispatched exactly like a press and is refused
            // exactly like one, so a preset that granted only what its pads
            // need would have a pad that gets halfway and stops.
            for action in actions_in(&parsed) {
                let Some(needs) = required.get(&action) else {
                    continue;
                };
                for permission in needs {
                    assert!(
                        config.permissions.allows(*permission),
                        "`{}`: `{action}` needs `{permission}`, which the preset does not grant",
                        preset.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_preset_grants_nothing_it_does_not_use() {
        // The other half of the same promise. A preset that granted more than
        // it needed would teach the operator that the list means nothing.
        for preset in ALL {
            let (parsed, config) = build_documented(preset);
            let required = crate::host::shipped_permissions(&config);

            let used: BTreeSet<_> = actions_in(&parsed)
                .into_iter()
                .filter_map(|action| required.get(&action).cloned())
                .flatten()
                .collect();

            for granted in config.permissions.iter() {
                assert!(
                    used.contains(&granted),
                    "`{}` grants `{granted}` and never uses it",
                    preset.name
                );
            }
        }
    }

    #[test]
    fn every_preset_can_be_left_and_returned_to() {
        // A surface with no way home is one an operator gets stranded on. Every
        // preset has to bind something that moves between pages.
        for preset in ALL {
            let (parsed, _) = build(preset);
            let moves = actions_in(&parsed)
                .into_iter()
                .any(|action| action.starts_with("page."));
            assert!(
                moves,
                "`{}` binds nothing that moves between pages",
                preset.name
            );
        }
    }

    #[test]
    fn every_preset_starts_somewhere_it_declared() {
        for preset in ALL {
            let (_, config) = build(preset);
            assert!(
                !config.pages.is_empty(),
                "`{}` has no pages at all",
                preset.name
            );
            if let Some(home) = &config.home_page {
                assert!(
                    config.pages.iter().any(|page| page.id == *home),
                    "`{}` starts on a page it does not declare",
                    preset.name
                );
            }
        }
    }

    #[test]
    fn every_preset_says_what_it_is_at_the_top() {
        // An operator reads the file before they run it. One that opened with a
        // binding would tell them nothing about what they were about to get.
        for preset in ALL {
            let opening = preset.body.lines().next().unwrap_or_default();
            assert!(
                opening.starts_with("# PushOS")
                    || opening.starts_with("# The PushOS")
                    || opening.starts_with('#'),
                "`{}` opens with `{opening}` rather than a description",
                preset.name
            );
        }
    }

    /// Every action a preset names, wherever it names one.
    ///
    /// Bindings, spoken phrases, workflow steps and the two places an action is
    /// named as a destination. A trap in any of them fails just as late.
    fn actions_in(parsed: &ConfigFile) -> Vec<String> {
        parsed
            .bindings
            .iter()
            .map(|binding| binding.action.clone())
            .chain(parsed.voice.request.clone())
            .chain(
                parsed
                    .voice
                    .commands
                    .iter()
                    .map(|command| command.action.clone()),
            )
            .chain(parsed.memory.brief.clone())
            .chain(
                parsed
                    .workflows
                    .iter()
                    .flat_map(|workflow| workflow.nodes.iter())
                    .filter_map(|node| node.action.clone()),
            )
            .chain(
                parsed
                    .sequences
                    .iter()
                    .flat_map(|run| run.steps.iter())
                    .map(|step| step.action.clone()),
            )
            .collect()
    }
}
