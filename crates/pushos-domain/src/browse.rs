//! Reaching what is not on a pad.
//!
//! Sixty-four pads is a lot and still not everything: a Mac with two dozen
//! projects, a role for each kind of work and a page for each way of working
//! has more in it than the surface holds. Ableton solves this with a browser
//! and one knob, and it is the right answer — a list you turn through and step
//! into beats binding a pad to everything you might ever want.
//!
//! This module is the cursor and nothing else. What is in the tree is built
//! from live state by whatever owns that state, which keeps this pure and
//! means the browser can never show something that has gone.

use serde::{Deserialize, Serialize};

/// One row in the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Twig {
    /// What it is called.
    pub label: String,
    /// What it is doing, or where it is, shown beside the name.
    pub detail: Option<String>,
    /// Whether stepping into it leads somewhere rather than doing something.
    pub has_children: bool,
}

impl Twig {
    /// A row that leads further in.
    pub fn branch(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            has_children: true,
        }
    }

    /// A row that does something when chosen.
    pub fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            has_children: false,
        }
    }

    /// Adds what to show beside the name.
    #[must_use]
    pub fn saying(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// What a control asks the browser to do.
///
/// The control decides what; whatever owns the tree decides what that means,
/// the same way a page control names a move and the surface performs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowseMove {
    /// Show the browser, at the top.
    Open,
    /// Put it away, leaving whatever was underneath.
    Close,
    /// Move the cursor by this many rows.
    Step(i8),
    /// Go into the row the cursor is on, or choose it if it is a leaf.
    Enter,
    /// Go back out to the level above.
    Leave,
}

/// Where the operator is in the tree.
///
/// A path of chosen rows and a cursor at the current level. Nothing about the
/// tree itself, so a row that disappears while the browser is open cannot
/// leave this pointing at something that is not there — the cursor is clamped
/// against the rows that exist each time it moves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Browsing {
    path: Vec<usize>,
    at: usize,
}

impl Browsing {
    /// A browser opened at the top of the tree.
    pub fn opened() -> Self {
        Self::default()
    }

    /// The rows chosen to get here, outermost first.
    pub fn path(&self) -> &[usize] {
        &self.path
    }

    /// Which row the cursor is on, given how many there are.
    ///
    /// Clamped rather than stored blindly: the tree is rebuilt from live state
    /// every time it is drawn, and a session that ended while the browser was
    /// open makes the level shorter underneath the cursor.
    pub fn at(&self, rows: usize) -> usize {
        if rows == 0 {
            return 0;
        }
        self.at.min(rows - 1)
    }

    /// How deep in the tree the cursor is.
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Moves the cursor, stopping at either end.
    ///
    /// Not wrapping, for the same reason a fleet does not: a list has an order
    /// and coming back round to the top loses the operator's place in it.
    pub fn step(&mut self, by: i8, rows: usize) {
        if rows == 0 {
            self.at = 0;
            return;
        }
        let at = i64::from(u32::try_from(self.at(rows)).unwrap_or(u32::MAX)) + i64::from(by);
        let last = i64::from(u32::try_from(rows - 1).unwrap_or(u32::MAX));
        self.at = usize::try_from(at.clamp(0, last)).unwrap_or(0);
    }

    /// Goes into the row the cursor is on.
    pub fn descend(&mut self, rows: usize) {
        self.path.push(self.at(rows));
        self.at = 0;
    }

    /// Goes back out a level.
    ///
    /// Returns whether there was anywhere to go: leaving the top is how the
    /// browser is closed, and that is the caller's decision rather than this
    /// one's.
    pub fn ascend(&mut self) -> bool {
        match self.path.pop() {
            // Back on the row that was stepped into, not at the top of the
            // level: an operator who goes in and straight back out should find
            // their finger where they left it.
            Some(was) => {
                self.at = was;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_browser_starts_at_the_top() {
        let browsing = Browsing::opened();
        assert_eq!(browsing.at(5), 0);
        assert_eq!(browsing.depth(), 0);
        assert!(browsing.path().is_empty());
    }

    #[test]
    fn the_cursor_stops_at_either_end_rather_than_wrapping() {
        let mut browsing = Browsing::opened();
        browsing.step(-1, 3);
        assert_eq!(browsing.at(3), 0, "it should not fall off the top");

        for _ in 0..10 {
            browsing.step(1, 3);
        }
        assert_eq!(browsing.at(3), 2, "nor off the bottom");
    }

    #[test]
    fn a_level_that_got_shorter_does_not_leave_the_cursor_past_its_end() {
        // The tree is built from live state, so a session ending while the
        // browser is open makes the level shorter under the operator's finger.
        let mut browsing = Browsing::opened();
        browsing.step(7, 8);
        assert_eq!(browsing.at(8), 7);
        assert_eq!(browsing.at(3), 2, "clamped to what is there now");
        assert_eq!(browsing.at(0), 0, "and an empty level is not a crash");
    }

    #[test]
    fn going_in_and_back_out_leaves_the_finger_where_it_was() {
        let mut browsing = Browsing::opened();
        browsing.step(2, 5);
        browsing.descend(5);
        assert_eq!(browsing.depth(), 1);
        assert_eq!(browsing.path(), [2]);
        assert_eq!(browsing.at(9), 0, "a new level starts at its top");

        browsing.step(3, 9);
        assert!(browsing.ascend());
        assert_eq!(browsing.depth(), 0);
        assert_eq!(
            browsing.at(5),
            2,
            "back on the row that was stepped into, not at the top"
        );
    }

    #[test]
    fn leaving_the_top_says_there_was_nowhere_to_go() {
        let mut browsing = Browsing::opened();
        assert!(!browsing.ascend(), "the caller decides what that means");
        assert_eq!(browsing.depth(), 0);
    }

    #[test]
    fn stepping_an_empty_level_does_nothing_rather_than_panicking() {
        let mut browsing = Browsing::opened();
        browsing.step(3, 0);
        assert_eq!(browsing.at(0), 0);
    }

    #[test]
    fn a_row_says_whether_it_leads_further_in() {
        assert!(Twig::branch("Projects").has_children);
        assert!(!Twig::leaf("pushos").has_children);
        assert_eq!(
            Twig::leaf("Claude Code")
                .saying("working")
                .detail
                .as_deref(),
            Some("working")
        );
    }
}
