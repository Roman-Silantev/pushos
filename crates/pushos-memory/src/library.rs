//! Notes as Markdown files in directories the operator chose.
//!
//! The files are the truth and the index is derived. That is what keeps a
//! PushOS installation something an operator can walk away from: their notes
//! are still there, still readable, and still editable by whatever else they
//! use.
//!
//! Searching asks the index first and reads the files when it has nothing to
//! say. Without a database that is every search, which is slower and still
//! correct; a library that refused to search without an index would have made
//! the index compulsory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use pushos_domain::ids::{NoteId, WorkspaceId};
use pushos_domain::memory::{Draft, Excerpt, Note, Search};
use pushos_domain::ports::{MemoryError, MemoryStore, NoteIndex, Source};
use tracing::{debug, info, warn};

use crate::frontmatter::{self, Header};
use crate::naming;

/// How deep into a source's directories notes are looked for.
///
/// Deep enough for the year-and-month folders people keep, shallow enough that
/// pointing PushOS at a home directory by mistake does not walk a disk.
const MOST_DEPTH: usize = 6;

/// How many files one source may contribute.
///
/// A guard rather than a limit anyone should meet. A directory with more notes
/// than this in it is a directory that was not meant to be a note source.
const MOST_NOTES: usize = 10_000;

/// Notes on disk.
#[derive(Debug)]
pub struct MarkdownLibrary {
    sources: Vec<Source>,
    index: Arc<dyn NoteIndex>,
}

impl MarkdownLibrary {
    /// Builds a library over the configured sources.
    pub fn new(sources: Vec<Source>, index: Arc<dyn NoteIndex>) -> Self {
        Self { sources, index }
    }

    /// Where a new note goes.
    fn writable(&self) -> Result<&Source, MemoryError> {
        self.sources
            .iter()
            .find(|source| source.writable)
            .ok_or_else(|| MemoryError::NowhereToWrite {
                context: if self.sources.is_empty() {
                    "no note sources are configured; add one under `[[memory.sources]]`".to_owned()
                } else {
                    "every note source is read only; one of them needs `writable = true`".to_owned()
                },
            })
    }

    /// The source an identity belongs to, and where it lives.
    fn locate(&self, note: &NoteId) -> Option<(&Source, PathBuf)> {
        self.sources.iter().find_map(|source| {
            naming::locate(&source.id, &source.root, note).map(|at| (source, at))
        })
    }

    /// Reads every note in every source, in the order they are searched.
    fn everything(&self) -> Vec<Note> {
        let mut found = Vec::new();
        for source in &self.sources {
            for path in files_under(&source.root) {
                match read_file(source, &path) {
                    Ok(note) => found.push(note),
                    // One unreadable file is not a reason to lose the rest.
                    Err(error) => warn!(%error, path = %path.display(), "a note could not be read"),
                }
            }
        }
        found
    }

    /// Searches by reading, for when there is no index to ask.
    fn read_through(&self, search: &Search) -> Vec<Excerpt> {
        let words: Vec<String> = search
            .text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect();

        let mut found: Vec<Note> = self
            .everything()
            .into_iter()
            .filter(|note| matches(note, search, &words))
            .collect();

        // Newest first, which is the only ordering available without scores and
        // the right one for an open search anyway.
        found.sort_by_key(|note| std::cmp::Reverse(note.written_at));
        found.truncate(search.taking());
        found.iter().map(excerpt_of).collect()
    }
}

#[async_trait]
impl MemoryStore for MarkdownLibrary {
    async fn write(&self, draft: &Draft) -> Result<Note, MemoryError> {
        if draft.is_empty() {
            return Err(MemoryError::Empty);
        }

        let source = self.writable()?;
        std::fs::create_dir_all(&source.root)
            .map_err(|error| MemoryError::unreadable("making the note directory", error))?;

        let title = draft.heading();
        let name = naming::file_name(&title, &today(), |candidate| {
            source.root.join(candidate).exists()
        });
        let path = source.root.join(&name);

        let header = Header {
            title: Some(title.clone()),
            workspace: draft.workspace.clone(),
            tags: draft.tags.clone(),
        };
        let body = draft.body.trim();
        std::fs::write(&path, format!("{}{body}\n", frontmatter::write(&header)))
            .map_err(|error| MemoryError::unreadable("writing a note", error))?;

        let note = Note {
            id: naming::identify(&source.id, &source.root, &path).ok_or_else(|| {
                MemoryError::NowhereToWrite {
                    context: format!("`{}` is not inside its own source", path.display()),
                }
            })?,
            title,
            body: body.to_owned(),
            source: source.id.clone(),
            path: Some(path),
            workspace: draft.workspace.clone(),
            tags: draft.tags.clone(),
            written_at: SystemTime::now(),
        };

        // Indexed before returning, so a note captured on one pad is findable
        // from the next one without anything in between.
        self.index.record(&note).await?;
        info!(note = %note.id, "wrote a note");
        Ok(note)
    }

    async fn find(&self, search: &Search) -> Result<Vec<Excerpt>, MemoryError> {
        if self.index.keeps() {
            return self.index.search(search).await;
        }

        // No index to ask. Reading is slower and always available, and the
        // operator asked a question either way.
        debug!("no index is kept; reading the notes instead");
        Ok(self.read_through(search))
    }

    async fn read(&self, note: &NoteId) -> Result<Option<Note>, MemoryError> {
        let Some((source, path)) = self.locate(note) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        read_file(source, &path).map(Some)
    }

    async fn refresh(&self) -> Result<usize, MemoryError> {
        let mut counted = 0;
        for source in &self.sources {
            // Emptied first: a note deleted from the directory has to leave the
            // index with it, and there is no other moment PushOS learns that.
            self.index.empty(&source.id).await?;
        }

        for note in self.everything() {
            self.index.record(&note).await?;
            counted += 1;
        }

        info!(
            notes = counted,
            sources = self.sources.len(),
            "notes indexed"
        );
        Ok(counted)
    }

    fn sources(&self) -> Vec<Source> {
        self.sources.clone()
    }
}

/// Whether a note answers a search.
fn matches(note: &Note, search: &Search, words: &[String]) -> bool {
    if let Some(workspace) = &search.workspace
        && note.workspace.as_ref() != Some(workspace)
    {
        return false;
    }
    if let Some(tag) = &search.tag
        && !note.tags.iter().any(|held| held == tag)
    {
        return false;
    }
    if words.is_empty() {
        return true;
    }

    let haystack = format!(
        "{} {} {}",
        note.title.to_lowercase(),
        note.body.to_lowercase(),
        note.tags.join(" ").to_lowercase()
    );
    // Every word, the same as the index requires, so the two agree about what
    // counts as a hit even though one is far faster than the other.
    words.iter().all(|word| haystack.contains(word))
}

/// Turns a note into something showable.
fn excerpt_of(note: &Note) -> Excerpt {
    Excerpt {
        id: note.id.clone(),
        title: note.title.clone(),
        snippet: note.summary().chars().take(SNIPPET_LIMIT).collect(),
        source: note.source.clone(),
    }
}

/// How long a read-through snippet may be.
const SNIPPET_LIMIT: usize = 80;

/// Reads one file as a note.
fn read_file(source: &Source, path: &Path) -> Result<Note, MemoryError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| MemoryError::unreadable(format!("reading `{}`", path.display()), error))?;
    let (header, body) = frontmatter::split(&text);

    let id = naming::identify(&source.id, &source.root, path).ok_or_else(|| {
        MemoryError::NowhereToWrite {
            context: format!(
                "`{}` is not inside `{}`",
                path.display(),
                source.root.display()
            ),
        }
    })?;

    Ok(Note {
        title: header.title.unwrap_or_else(|| title_of(body, path)),
        body: body.trim().to_owned(),
        source: source.id.clone(),
        workspace: header.workspace.clone().or_else(|| workspace_of(body)),
        tags: header.tags,
        written_at: modified(path),
        path: Some(path.to_path_buf()),
        id,
    })
}

/// What to call a note whose header did not say.
///
/// Its first heading, or its first line, or its file name. A note nobody titled
/// still has to be recognisable in a list of five.
fn title_of(body: &str, path: &Path) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let heading = first.trim_start_matches('#').trim();

    if heading.is_empty() {
        path.file_stem()
            .map_or_else(|| "Note".to_owned(), |stem| stem.to_string_lossy().into())
    } else {
        heading.chars().take(TITLE_LIMIT).collect()
    }
}

/// How much of a first line becomes a title.
const TITLE_LIMIT: usize = 60;

/// The project a note belongs to, when only the body says so.
///
/// Always `None`: a project is something the header states, and guessing it
/// from prose would file notes under projects they only mention.
const fn workspace_of(_body: &str) -> Option<WorkspaceId> {
    None
}

/// When a file last changed, falling back to now.
fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|data| data.modified())
        .unwrap_or_else(|_| SystemTime::now())
}

/// Every note file under a directory.
///
/// Iterative rather than recursive, and bounded in both depth and count: a
/// source is a directory the operator named, and naming the wrong one should
/// cost a moment rather than a walk of the whole disk.
fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0_usize)];

    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();

            // Dot directories are somebody else's business: `.git`, `.obsidian`
            // and the like hold thousands of files and no notes.
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                if depth < MOST_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|it| it == naming::EXTENSION) {
                found.push(path);
                if found.len() >= MOST_NOTES {
                    warn!(
                        root = %root.display(),
                        "stopping at {MOST_NOTES} notes; is this the directory you meant?"
                    );
                    return found;
                }
            }
        }
    }

    // Read in whatever order the file system gave them, so sort for a stable
    // result: a listing that reshuffles between runs is one nobody can trust.
    found.sort();
    found
}

/// Today, as a file name wants it.
fn today() -> String {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Turns days since the epoch into a calendar date.
///
/// Howard Hinnant's algorithm, which is exact and needs no dependency. A date
/// crate for one format string would be a dependency for a file name.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * shifted_month + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    })
    .unwrap_or(1);

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_algorithm_agrees_with_dates_anyone_can_check() {
        for (days, expected) in [
            (0_i64, (1970_i64, 1_u32, 1_u32)),
            (19_723, (2024, 1, 1)),
            // A leap day and the day after it, which is where a home-made
            // calendar usually goes wrong.
            (19_782, (2024, 2, 29)),
            (19_783, (2024, 3, 1)),
            // 2000 was a leap year and 1900 was not, which is the other one.
            (11_016, (2000, 2, 29)),
            (20_705, (2026, 9, 9)),
        ] {
            assert_eq!(civil_from_days(days), expected, "day {days}");
        }
    }

    #[test]
    fn every_day_of_a_decade_is_a_real_date_in_order() {
        // A file name that sorted wrongly, or named a thirty-second of
        // February, would be found out slowly and by a person.
        let mut previous = civil_from_days(18_262);
        for days in 18_263..21_915_i64 {
            let (year, month, day) = civil_from_days(days);
            assert!((1..=12).contains(&month), "day {days} gave month {month}");
            assert!((1..=31).contains(&day), "day {days} gave day {day}");
            assert!(
                (year, month, day) > previous,
                "day {days} went backwards from {previous:?}"
            );
            previous = (year, month, day);
        }
    }

    #[test]
    fn a_note_is_titled_by_its_first_heading_when_nothing_else_says() {
        let path = Path::new("/tmp/notes/whatever.md");
        assert_eq!(title_of("# Deploy\n\nbody", path), "Deploy");
        assert_eq!(title_of("Just a line\n", path), "Just a line");
        assert_eq!(title_of("   \n\n", path), "whatever");
    }
}
