//! The small header at the top of a note.
//!
//! Deliberately not a YAML parser. PushOS writes three keys and reads whatever
//! it recognises, ignoring the rest, because these files belong to the operator
//! and may well have been written by something else with opinions of its own. A
//! header PushOS cannot understand is a header to leave alone, not a reason to
//! refuse the note.

use pushos_domain::ids::WorkspaceId;

/// The line that opens and closes a header.
const FENCE: &str = "---";

/// What PushOS reads out of a header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Header {
    /// What the note is called.
    pub(crate) title: Option<String>,
    /// The project it belongs to.
    pub(crate) workspace: Option<WorkspaceId>,
    /// How it was filed.
    pub(crate) tags: Vec<String>,
}

/// Splits a file into its header and everything under it.
///
/// A file with no header is all body, which is the common case for notes
/// written by hand.
pub(crate) fn split(text: &str) -> (Header, &str) {
    let Some(rest) = opening(text) else {
        return (Header::default(), text);
    };
    let Some(end) = closing(rest) else {
        // An opening fence with no closing one is a horizontal rule someone
        // started their note with, not a header PushOS should eat.
        return (Header::default(), text);
    };

    (read(&rest[..end.0]), rest[end.1..].trim_start_matches('\n'))
}

/// Whatever follows the opening fence, when there is one.
fn opening(text: &str) -> Option<&str> {
    let trimmed = text.trim_start_matches(['\u{feff}', '\n', '\r']);
    let rest = trimmed.strip_prefix(FENCE)?;
    match rest.strip_prefix('\n') {
        Some(rest) => Some(rest),
        None => rest.strip_prefix("\r\n"),
    }
}

/// Where the closing fence starts and ends.
fn closing(rest: &str) -> Option<(usize, usize)> {
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == FENCE {
            return Some((at, at + line.len()));
        }
        at += line.len();
    }
    None
}

/// Reads the keys PushOS knows, ignoring everything else.
fn read(header: &str) -> Header {
    let mut found = Header::default();

    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        if value.is_empty() {
            continue;
        }

        match key.trim().to_lowercase().as_str() {
            "title" => found.title = Some(value.to_owned()),
            "workspace" | "project" => found.workspace = Some(WorkspaceId::new(value)),
            "tags" => found.tags = tags(value),
            _ => {}
        }
    }

    found
}

/// Tags, written either as a list or as words.
fn tags(value: &str) -> Vec<String> {
    value
        .trim_matches(['[', ']'])
        .split([',', ' '])
        .map(|tag| tag.trim().trim_matches(['"', '\'', '#']))
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Writes a header for a note PushOS is creating.
///
/// Only the keys that have something to say. A header full of empty fields is
/// noise in a file a person has to read.
pub(crate) fn write(header: &Header) -> String {
    let mut lines = vec![FENCE.to_owned()];
    if let Some(title) = &header.title {
        lines.push(format!("title: {title}"));
    }
    if let Some(workspace) = &header.workspace {
        lines.push(format!("workspace: {workspace}"));
    }
    if !header.tags.is_empty() {
        lines.push(format!("tags: [{}]", header.tags.join(", ")));
    }
    lines.push(FENCE.to_owned());
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_with_no_header_is_all_body() {
        let (header, body) = split("Just a thought.\n");
        assert_eq!(header, Header::default());
        assert_eq!(body, "Just a thought.\n");
    }

    #[test]
    fn the_keys_pushos_writes_are_the_keys_it_reads() {
        let header = Header {
            title: Some("Deploy".to_owned()),
            workspace: Some(WorkspaceId::new("sydclaw")),
            tags: vec!["decision".to_owned(), "infra".to_owned()],
        };
        let file = format!("{}The body.\n", write(&header));

        let (read, body) = split(&file);
        assert_eq!(read, header);
        assert_eq!(body, "The body.\n");
    }

    #[test]
    fn keys_pushos_does_not_know_are_left_alone_rather_than_refused() {
        // These files may have been written by something else entirely.
        let (header, body) = split("---\ntitle: Deploy\naliases: [d]\ncssclass: wide\n---\nBody\n");
        assert_eq!(header.title.as_deref(), Some("Deploy"));
        assert_eq!(body, "Body\n");
    }

    #[test]
    fn a_note_beginning_with_a_horizontal_rule_keeps_it() {
        // An opening fence with nothing closing it is somebody's punctuation.
        let text = "---\nJust a thought under a line.\n";
        let (header, body) = split(text);
        assert_eq!(header, Header::default());
        assert_eq!(body, text);
    }

    #[test]
    fn tags_are_read_however_they_were_written() {
        for written in [
            "tags: [a, b]",
            "tags: a b",
            "tags: \"a\", \"b\"",
            "tags: #a #b",
        ] {
            let (header, _) = split(&format!("---\n{written}\n---\nbody\n"));
            assert_eq!(header.tags, ["a", "b"], "`{written}`");
        }
    }

    #[test]
    fn a_project_can_be_written_either_way_round() {
        for key in ["workspace", "project"] {
            let (header, _) = split(&format!("---\n{key}: sydclaw\n---\nbody\n"));
            assert_eq!(header.workspace, Some(WorkspaceId::new("sydclaw")));
        }
    }

    #[test]
    fn an_empty_value_is_the_same_as_not_saying_it() {
        let (header, _) = split("---\ntitle:\nworkspace:   \n---\nbody\n");
        assert_eq!(header, Header::default());
    }

    #[test]
    fn windows_line_endings_do_not_hide_the_header() {
        let (header, body) = split("---\r\ntitle: Deploy\r\n---\r\nBody\r\n");
        assert_eq!(header.title.as_deref(), Some("Deploy"));
        assert_eq!(body, "Body\r\n");
    }
}
