//! Turning a note into a file name, and a file back into a note's identity.
//!
//! A note's identity is where it lives: source, then path within it. Derived
//! rather than generated, so re-reading a directory gives every note the name
//! it had before, and a note can be linked to and reopened.

use std::path::{Component, Path, PathBuf};

use pushos_domain::ids::{NoteId, SourceId};

/// The extension a note file has.
pub(crate) const EXTENSION: &str = "md";

/// How many characters of a title become part of a file name.
const SLUG_LIMIT: usize = 48;

/// The identity of a note at `path` inside `root`.
///
/// `None` when the path is not actually inside the root, which is the case
/// worth refusing rather than resolving.
pub(crate) fn identify(source: &SourceId, root: &Path, path: &Path) -> Option<NoteId> {
    let relative = path.strip_prefix(root).ok()?;
    let mut written = String::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            // `..` and absolute roots have no place in an identity; a note is
            // somewhere inside its source or it is not that source's note.
            return None;
        };
        if !written.is_empty() {
            written.push('/');
        }
        written.push_str(&part.to_string_lossy());
    }

    (!written.is_empty()).then(|| NoteId::new(format!("{source}/{written}")))
}

/// Where a note lives, given its identity.
///
/// `None` when the identity does not belong to this source, or when it tries to
/// climb out of it. An identity arrives from configuration, a search result or
/// an action's parameters, so it is not to be trusted with a file path.
pub(crate) fn locate(source: &SourceId, root: &Path, note: &NoteId) -> Option<PathBuf> {
    let prefix = format!("{source}/");
    let relative = note.as_str().strip_prefix(&prefix)?;
    if relative.is_empty() {
        return None;
    }

    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        path.push(part);
    }

    // Belt and braces: a component that looked ordinary but resolved elsewhere,
    // such as one containing a separator the split did not catch.
    path.starts_with(root).then_some(path)
}

/// A file name for a note with this title, written now.
///
/// Dated first so a directory listing is in the order things happened, which is
/// how a pile of captured thoughts is usually read.
pub(crate) fn file_name(title: &str, today: &str, taken: impl Fn(&str) -> bool) -> String {
    let slug = slug(title);
    let stem = if slug.is_empty() {
        today.to_owned()
    } else {
        format!("{today}-{slug}")
    };

    let first = format!("{stem}.{EXTENSION}");
    if !taken(&first) {
        return first;
    }

    // Two thoughts captured on one day about one thing is ordinary, not an
    // error, so the second gets a number rather than overwriting the first.
    for attempt in 2..100_u32 {
        let candidate = format!("{stem}-{attempt}.{EXTENSION}");
        if !taken(&candidate) {
            return candidate;
        }
    }

    format!(
        "{stem}-{}.{EXTENSION}",
        pushos_domain::ids::ExecutionId::generate()
    )
}

/// The part of a file name that comes from the title.
fn slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut hyphenated = true;

    for character in title.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
            hyphenated = false;
        } else if !hyphenated {
            slug.push('-');
            hyphenated = true;
        }
        if slug.len() >= SLUG_LIMIT {
            break;
        }
    }

    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceId {
        SourceId::new("notes")
    }

    #[test]
    fn a_note_is_identified_by_where_it_lives() {
        let id = identify(
            &source(),
            Path::new("/tmp/notes"),
            Path::new("/tmp/notes/2026/deploy.md"),
        );
        assert_eq!(id, Some(NoteId::new("notes/2026/deploy.md")));
    }

    #[test]
    fn a_file_outside_the_source_has_no_identity_in_it() {
        assert_eq!(
            identify(&source(), Path::new("/tmp/notes"), Path::new("/etc/passwd")),
            None
        );
    }

    #[test]
    fn an_identity_resolves_back_to_the_file_it_came_from() {
        let root = Path::new("/tmp/notes");
        let id = identify(&source(), root, Path::new("/tmp/notes/a/b.md")).expect("inside");
        assert_eq!(
            locate(&source(), root, &id),
            Some(PathBuf::from("/tmp/notes/a/b.md"))
        );
    }

    #[test]
    fn an_identity_cannot_climb_out_of_its_source() {
        // Identities arrive from search results and action parameters, so this
        // is the difference between reading a note and reading anything.
        let root = Path::new("/tmp/notes");
        for escape in [
            "notes/../../etc/passwd",
            "notes/..",
            "notes/./../secret",
            "notes/",
        ] {
            assert_eq!(
                locate(&source(), root, &NoteId::new(escape)),
                None,
                "`{escape}` must not resolve"
            );
        }
    }

    #[test]
    fn an_identity_belonging_to_another_source_does_not_resolve() {
        assert_eq!(
            locate(
                &source(),
                Path::new("/tmp/notes"),
                &NoteId::new("archive/a.md")
            ),
            None
        );
    }

    #[test]
    fn a_file_name_reads_as_the_thing_it_is_about() {
        assert_eq!(
            file_name("The deploy needs the new token", "2026-09-09", |_| false),
            "2026-09-09-the-deploy-needs-the-new-token.md"
        );
    }

    #[test]
    fn punctuation_and_case_do_not_reach_the_file_system() {
        assert_eq!(
            file_name("Deploy: it's *broken*!", "2026-09-09", |_| false),
            "2026-09-09-deploy-it-s-broken.md"
        );
    }

    #[test]
    fn a_title_with_no_letters_in_it_still_produces_a_name() {
        assert_eq!(file_name("!!!", "2026-09-09", |_| false), "2026-09-09.md");
    }

    #[test]
    fn a_second_note_on_one_day_does_not_overwrite_the_first() {
        let existing = |name: &str| name == "2026-09-09-deploy.md";
        assert_eq!(
            file_name("Deploy", "2026-09-09", existing),
            "2026-09-09-deploy-2.md"
        );
    }

    #[test]
    fn a_long_title_is_cut_rather_than_producing_an_unusable_name() {
        let name = file_name(&"word ".repeat(40), "2026-09-09", |_| false);
        assert!(name.len() < 80, "{name}");
        assert!(name.ends_with(&format!(".{EXTENSION}")));
    }
}
