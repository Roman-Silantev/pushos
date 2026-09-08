//! The last few lines a terminal produced.
//!
//! A build can print a hundred thousand lines. PushOS keeps a fixed number of
//! them and forgets the rest, because the alternative is a process that grows
//! all day and a display that only ever shows the end anyway.

use std::collections::VecDeque;

/// How many finished lines are kept.
///
/// Enough to see what a command did, small enough that a runaway loop costs
/// nothing.
const KEPT_LINES: usize = 200;

/// How long a single line may get before the rest of it is discarded.
///
/// A line wider than this cannot be read on a 960-pixel display, and a program
/// printing a megabyte without a newline is not trying to be read.
const MAX_LINE: usize = 512;

/// A bounded view of the end of a terminal's output.
#[derive(Debug)]
pub struct OutputTail {
    finished: VecDeque<String>,
    /// The line being written, which no newline has ended yet.
    current: String,
    /// Set once the current line has been truncated, so the truncation is
    /// reported once rather than on every character.
    truncated: bool,
    /// A carriage return whose meaning is not yet decided.
    ///
    /// Followed by a newline it is half of an ordinary line ending; followed by
    /// anything else it rewrites the line. A pty emits both, often split across
    /// two reads, so the decision has to wait for the next character.
    pending_return: bool,
}

impl Default for OutputTail {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputTail {
    /// Builds an empty tail.
    pub fn new() -> Self {
        Self {
            finished: VecDeque::with_capacity(KEPT_LINES),
            current: String::new(),
            truncated: false,
            pending_return: false,
        }
    }

    /// Adds text a terminal produced.
    ///
    /// A carriage return with no newline rewrites the current line, which is
    /// how progress meters work: what is kept is the last state, not every one.
    pub fn push(&mut self, text: &str) {
        for character in text.chars() {
            // A carriage return that turned out not to end a line rewrites it.
            if std::mem::take(&mut self.pending_return) && character != '\n' {
                self.restart_line();
            }

            match character {
                '\n' => self.finish_line(),
                '\r' => self.pending_return = true,
                _ if self.current.chars().count() >= MAX_LINE => self.truncate_line(),
                _ => self.current.push(character),
            }
        }
    }

    fn finish_line(&mut self) {
        let line = std::mem::take(&mut self.current);
        self.truncated = false;
        if self.finished.len() == KEPT_LINES {
            self.finished.pop_front();
        }
        self.finished.push_back(line);
    }

    fn restart_line(&mut self) {
        self.current.clear();
        self.truncated = false;
    }

    fn truncate_line(&mut self) {
        if !self.truncated {
            self.current.push('…');
            self.truncated = true;
        }
    }

    /// Every line kept, oldest first, including the one still being written.
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.finished
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.current.as_str()))
    }

    /// The most recent line worth showing.
    ///
    /// Finished lines are preferred over the one still being written, because
    /// the line a program has not finished is usually its prompt, and "the
    /// tests passed" is what an operator wants on the display rather than the
    /// shell waiting for the next thing to do.
    ///
    /// The unfinished line is used when it is all there is, which is what makes
    /// a progress meter and a shell that has only just started still show
    /// something.
    pub fn last_meaningful_line(&self) -> Option<&str> {
        self.finished
            .iter()
            .map(String::as_str)
            .rev()
            .find(|line| !line.trim().is_empty())
            .or_else(|| Some(self.current.as_str()).filter(|line| !line.trim().is_empty()))
    }

    /// The last `count` lines, oldest first, blank trailing lines removed.
    pub fn recent(&self, count: usize) -> Vec<&str> {
        let mut lines: Vec<&str> = self.lines().collect();
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        if lines.len() > count {
            lines.drain(..lines.len() - count);
        }
        lines
    }

    /// Whether anything has been produced at all.
    pub fn is_empty(&self) -> bool {
        self.finished.is_empty() && self.current.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_of(text: &str) -> OutputTail {
        let mut tail = OutputTail::new();
        tail.push(text);
        tail
    }

    #[test]
    fn a_new_tail_has_nothing_in_it() {
        let tail = OutputTail::new();
        assert!(tail.is_empty());
        assert_eq!(tail.last_meaningful_line(), None);
    }

    #[test]
    fn lines_are_kept_in_the_order_they_were_written() {
        let tail = tail_of("one\ntwo\nthree");
        assert_eq!(tail.recent(10), ["one", "two", "three"]);
    }

    #[test]
    fn a_line_arriving_in_pieces_is_one_line() {
        let mut tail = OutputTail::new();
        tail.push("com");
        tail.push("pil");
        tail.push("ing\n");
        assert_eq!(tail.recent(10), ["compiling"]);
    }

    #[test]
    fn the_line_ending_a_terminal_actually_sends_keeps_the_line() {
        // A pty ends lines with a carriage return and a newline. Treating the
        // return as a rewrite would erase every line just before keeping it.
        let tail = tail_of("compiling\r\n77 passed\r\n");
        assert_eq!(tail.recent(10), ["compiling", "77 passed"]);
    }

    #[test]
    fn a_line_ending_split_across_reads_is_still_one_ending() {
        let mut tail = OutputTail::new();
        tail.push("compiling\r");
        tail.push("\n77 passed\r\n");
        assert_eq!(tail.recent(10), ["compiling", "77 passed"]);
    }

    #[test]
    fn a_progress_meter_leaves_only_its_last_state() {
        // Every step overwrote the one before it on a real terminal, and the
        // tail has to agree with what the operator would have seen.
        let tail = tail_of("10%\r50%\r100%\r\n");
        assert_eq!(tail.recent(10), ["100%"]);
    }

    #[test]
    fn old_lines_are_forgotten_rather_than_accumulated() {
        let mut tail = OutputTail::new();
        for line in 0..KEPT_LINES * 3 {
            tail.push(&format!("line {line}\n"));
        }
        assert_eq!(
            tail.lines().count(),
            KEPT_LINES + 1,
            "plus the empty current line"
        );
        assert_eq!(tail.recent(1), [format!("line {}", KEPT_LINES * 3 - 1)]);
    }

    #[test]
    fn an_absurdly_long_line_is_cut_and_says_so() {
        let tail = tail_of(&"x".repeat(MAX_LINE * 4));
        let line = tail.recent(1)[0];
        assert_eq!(line.chars().count(), MAX_LINE + 1);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn a_cut_line_does_not_leave_the_next_one_cut() {
        let mut tail = OutputTail::new();
        tail.push(&"x".repeat(MAX_LINE * 2));
        tail.push("\nshort\n");
        assert_eq!(tail.recent(1), ["short"]);
    }

    #[test]
    fn the_display_line_is_what_a_command_said_not_the_prompt_after_it() {
        // What a shell leaves on screen is its next prompt, and the operator
        // wants to see how the command went rather than that it is ready for
        // another one.
        let tail = tail_of("sh-3.2$ cargo test\r\n77 passed\r\nsh-3.2$ ");
        assert_eq!(tail.last_meaningful_line(), Some("77 passed"));
    }

    #[test]
    fn a_terminal_that_has_only_printed_a_prompt_still_shows_it() {
        // It is all the terminal has said, so it is what there is to show.
        let tail = tail_of("sh-3.2$ ");
        assert_eq!(tail.last_meaningful_line(), Some("sh-3.2$ "));
    }

    #[test]
    fn a_progress_meter_that_has_never_ended_a_line_is_still_shown() {
        let tail = tail_of("10%\r50%\r100%");
        assert_eq!(tail.last_meaningful_line(), Some("100%"));
    }

    #[test]
    fn the_display_line_skips_trailing_blank_output() {
        // A command that finishes by printing newlines has still told the
        // operator something, and that is what the surface should show.
        let tail = tail_of("tests passed\n\n\n");
        assert_eq!(tail.last_meaningful_line(), Some("tests passed"));
    }

    #[test]
    fn asking_for_more_lines_than_exist_returns_what_there_is() {
        let tail = tail_of("only\n");
        assert_eq!(tail.recent(50), ["only"]);
    }

    #[test]
    fn asking_for_fewer_lines_returns_the_most_recent_ones() {
        let tail = tail_of("one\ntwo\nthree\nfour\n");
        assert_eq!(tail.recent(2), ["three", "four"]);
    }
}
