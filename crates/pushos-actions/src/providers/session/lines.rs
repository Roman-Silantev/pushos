//! Picking out what a session said from what is on its screen.

/// The last few lines that have anything on them.
///
/// A terminal's screen is mostly blank and mostly rules, and a coding agent
/// draws a good deal of chrome besides. What an operator wants is the last few
/// things that were actually said, so the rules, the empty rows and the status
/// line at the foot are dropped and the words are kept.
pub(super) fn last_lines(screen: &str, most: usize) -> Vec<String> {
    let mut kept: Vec<String> = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.chars().any(char::is_alphanumeric))
        .filter(|line| !is_chrome(line))
        .map(|line| line.chars().take(WIDEST).collect())
        .collect();

    if kept.len() > most {
        kept.drain(..kept.len() - most);
    }
    kept
}

/// How many characters of a line the panel can hold.
///
/// Wider than it looks: the panel is 960 pixels and this type is small, so a
/// line of prose fits whole and only a wrapped paste is cut.
const WIDEST: usize = 150;

/// Whether a line is the terminal's own furniture rather than something said.
fn is_chrome(line: &str) -> bool {
    let trimmed = line.trim();
    // A row of rule characters, which every agent draws between sections.
    let ruled = trimmed
        .chars()
        .all(|c| matches!(c, '─' | '━' | '═' | '-' | '_' | '·'));

    ruled
        || trimmed.contains("shift+tab to cycle")
        || trimmed.starts_with("/clear to save")
        || trimmed.contains("new task? /clear")
        // Codex's footer.
        || trimmed.contains("? for shortcuts")
        || trimmed.ends_with("context left")
}
