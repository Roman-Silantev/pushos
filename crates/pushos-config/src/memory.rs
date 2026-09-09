//! Turning the `[memory]` section into something the runtime can use.
//!
//! Kept apart from the rest of the build because a note source is a directory
//! on the operator's disk, and the checks that matter are about that: a name
//! that can be part of a path, and a place PushOS is actually allowed to write.

use std::collections::HashSet;

use pushos_domain::action::ActionSelector;
use pushos_domain::ids::SourceId;
use pushos_domain::ports::Source;

use crate::error::Problem;
use crate::model::{ConfigFile, MemorySourceEntry};

/// Notes, as the runtime sees them.
#[derive(Debug)]
pub struct MemorySettings {
    /// Where notes are kept, in the order they are searched.
    pub sources: Vec<Source>,
    /// What a briefing is handed to, with the notes as `text`.
    pub brief: Option<ActionSelector>,
}

impl MemorySettings {
    /// Whether PushOS may write a note down.
    ///
    /// A configuration that can only read is a real choice rather than a
    /// mistake, and the capture pad says so when pressed.
    pub fn can_capture(&self) -> bool {
        self.sources.iter().any(|source| source.writable)
    }
}

/// Builds the memory settings, or nothing when memory is not configured.
pub(crate) fn build(file: &ConfigFile, problems: &mut Vec<Problem>) -> Option<MemorySettings> {
    let section = &file.memory;
    if section.brief.is_none() && section.sources.is_empty() {
        return None;
    }

    if section.sources.is_empty() {
        problems.push(Problem::MemoryWithoutSources);
        return None;
    }

    let brief = match section.brief.as_deref() {
        None => None,
        Some(written) => match written.parse::<ActionSelector>() {
            Ok(selector) => Some(selector),
            Err(source) => {
                problems.push(Problem::MemoryBriefAction { source });
                None
            }
        },
    };

    Some(MemorySettings {
        sources: build_sources(&section.sources, problems),
        brief,
    })
}

/// Builds the sources, reporting each one that cannot be used.
fn build_sources(entries: &[MemorySourceEntry], problems: &mut Vec<Problem>) -> Vec<Source> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(entries.len());
    let mut sources = Vec::with_capacity(entries.len());

    for entry in entries {
        if !is_usable(&entry.id) {
            // The name becomes the first part of every note's identity, and
            // that identity is resolved back to a file path.
            problems.push(Problem::UnusableNoteSource {
                id: entry.id.clone(),
            });
            continue;
        }
        if !seen.insert(entry.id.as_str()) {
            problems.push(Problem::DuplicateNoteSource {
                id: entry.id.clone(),
            });
            continue;
        }

        sources.push(Source {
            id: SourceId::new(&entry.id),
            root: crate::paths::expand_home(&entry.path),
            writable: entry.writable,
        });
    }

    sources
}

/// Whether a name can safely be part of a note's identity.
fn is_usable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MemorySection;

    fn source(id: &str, path: &str, writable: bool) -> MemorySourceEntry {
        MemorySourceEntry {
            id: id.to_owned(),
            path: path.to_owned(),
            writable,
        }
    }

    fn built(section: MemorySection) -> (Option<MemorySettings>, Vec<Problem>) {
        let file = ConfigFile {
            memory: section,
            ..ConfigFile::default()
        };
        let mut problems = Vec::new();
        let settings = build(&file, &mut problems);
        (settings, problems)
    }

    #[test]
    fn no_memory_section_means_memory_is_off() {
        let (settings, problems) = built(MemorySection::default());
        assert!(settings.is_none(), "memory is opt-in");
        assert!(problems.is_empty());
    }

    #[test]
    fn a_source_is_read_only_unless_it_says_otherwise() {
        // A directory the operator pointed PushOS at is theirs, and reading it
        // is not permission to write in it.
        let (settings, problems) = built(MemorySection {
            sources: vec![source("notes", "/tmp/notes", false)],
            ..MemorySection::default()
        });
        assert!(problems.is_empty());
        let settings = settings.expect("a source turns memory on");
        assert!(!settings.can_capture());
    }

    #[test]
    fn a_leading_tilde_is_expanded_like_everywhere_else() {
        let (settings, _) = built(MemorySection {
            sources: vec![source("notes", "~/Notes", true)],
            ..MemorySection::default()
        });
        let settings = settings.expect("configured");
        assert!(!settings.sources[0].root.starts_with("~"));
        assert!(settings.can_capture());
    }

    #[test]
    fn memory_with_nowhere_to_read_is_reported_rather_than_started() {
        let (settings, problems) = built(MemorySection {
            brief: Some("agent.prompt".to_owned()),
            sources: Vec::new(),
        });
        assert!(settings.is_none());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn two_sources_with_one_name_are_reported() {
        // A name is the first part of every note's identity there, so two of
        // them would make an identity ambiguous.
        let (_, problems) = built(MemorySection {
            sources: vec![
                source("notes", "/tmp/a", true),
                source("notes", "/tmp/b", false),
            ],
            ..MemorySection::default()
        });
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_name_that_could_escape_a_directory_is_refused() {
        // The name is resolved back into a file path, so this is the difference
        // between reading a note and reading anything.
        for bad in ["..", "a/b", "", "with space", "notes/../etc"] {
            let (_, problems) = built(MemorySection {
                sources: vec![source(bad, "/tmp/notes", true)],
                ..MemorySection::default()
            });
            assert_eq!(problems.len(), 1, "`{bad}` should be refused");
        }
    }

    #[test]
    fn a_malformed_briefing_action_is_reported_by_name() {
        let (_, problems) = built(MemorySection {
            brief: Some("prompt".to_owned()),
            sources: vec![source("notes", "/tmp/notes", true)],
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].to_string().contains("brief"));
    }

    #[test]
    fn sources_keep_the_order_they_were_written_in() {
        // The first writable one is where new notes go, so the order is a
        // decision the operator made rather than an accident.
        let (settings, _) = built(MemorySection {
            sources: vec![
                source("archive", "/tmp/archive", false),
                source("notes", "/tmp/notes", true),
            ],
            ..MemorySection::default()
        });
        let settings = settings.expect("configured");
        assert_eq!(settings.sources[0].id.as_str(), "archive");
        assert_eq!(settings.sources[1].id.as_str(), "notes");
    }
}
