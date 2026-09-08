//! Turning what a program wrote into something PushOS can show.
//!
//! A pseudo-terminal carries bytes, not text. They arrive in whatever chunks
//! the kernel felt like, so a character can be split across two reads and an
//! escape sequence across three. Everything here is therefore incremental: it
//! holds what it cannot yet interpret and returns only what it can.
//!
//! Terminal control sequences are removed rather than interpreted. PushOS shows
//! a few lines on a 160-pixel-high display; it is not a terminal emulator, and
//! a half-drawn progress bar rendered literally is worse than nothing.

/// Where a control sequence has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Escape {
    /// Ordinary text.
    #[default]
    None,
    /// `ESC` seen; the next byte says what kind of sequence this is.
    Introducer,
    /// `ESC [`: parameters until a byte in `0x40..=0x7E` ends it.
    ControlSequence,
    /// `ESC` followed by an intermediate byte, waiting for the final one.
    Intermediate,
    /// `ESC ]`, `ESC P` and friends: a string that runs until a terminator.
    String,
    /// Inside a string, an `ESC` was seen; a `\` ends the string.
    StringTerminator,
}

/// Decodes terminal output into printable text.
///
/// One per terminal, fed every chunk in order.
#[derive(Debug, Default)]
pub(crate) struct OutputFilter {
    /// Bytes of a character that has not finished arriving.
    ///
    /// Never more than three: the longest incomplete UTF-8 sequence.
    partial: Vec<u8>,
    escape: Escape,
}

impl OutputFilter {
    /// Builds a filter ready for the first chunk.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Decodes one chunk, returning the text that can be shown.
    ///
    /// A character split across chunks appears once, in the chunk that
    /// completes it. Bytes that are not valid UTF-8 become the replacement
    /// character rather than being dropped, so corruption is visible instead of
    /// silent.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
        let decoded = self.decode(bytes);
        self.strip(&decoded)
    }

    /// Reassembles characters, holding back an unfinished one.
    fn decode(&mut self, bytes: &[u8]) -> String {
        self.partial.extend_from_slice(bytes);
        let mut text = String::with_capacity(self.partial.len());

        loop {
            match std::str::from_utf8(&self.partial) {
                Ok(whole) => {
                    text.push_str(whole);
                    self.partial.clear();
                    return text;
                }
                Err(problem) => {
                    let valid = problem.valid_up_to();
                    // The prefix is valid by definition, so nothing is lost.
                    text.push_str(&String::from_utf8_lossy(&self.partial[..valid]));

                    // No length means the bytes are merely unfinished, so they
                    // are kept for the chunk that completes them.
                    let Some(len) = problem.error_len() else {
                        self.partial.drain(..valid);
                        return text;
                    };

                    // Genuinely invalid: report it and carry on.
                    text.push(char::REPLACEMENT_CHARACTER);
                    self.partial.drain(..valid + len);
                }
            }
        }
    }

    /// Removes control sequences, keeping the text and the line structure.
    ///
    /// Each state gets its own reader, because what a byte means depends
    /// entirely on which part of a sequence it lands in.
    fn strip(&mut self, text: &str) -> String {
        let mut plain = String::with_capacity(text.len());

        for character in text.chars() {
            self.escape = match self.escape {
                Escape::None => {
                    if character == '\u{1b}' {
                        Escape::Introducer
                    } else {
                        if keeps(character) {
                            plain.push(character);
                        }
                        Escape::None
                    }
                }
                Escape::Introducer => after_introducer(character),
                Escape::Intermediate => after_intermediate(character),
                Escape::ControlSequence => in_control_sequence(character),
                Escape::String => in_string(character),
                Escape::StringTerminator => after_string_escape(character),
            };
        }

        plain
    }
}

/// What follows an `ESC`.
const fn after_introducer(character: char) -> Escape {
    match character {
        '[' => Escape::ControlSequence,
        // Operating system, device control, privacy and application program
        // commands all run until a string terminator.
        ']' | 'P' | 'X' | '^' | '_' => Escape::String,
        // An intermediate byte means the final one has not arrived yet.
        // Character set selection, which shells emit at startup, is three
        // characters long for exactly this reason.
        _ if is_intermediate(character) => Escape::Intermediate,
        // Anything else is the final byte, and the sequence is done.
        _ => Escape::None,
    }
}

/// What follows an intermediate byte.
const fn after_intermediate(character: char) -> Escape {
    if is_intermediate(character) {
        Escape::Intermediate
    } else {
        Escape::None
    }
}

/// What follows `ESC [`.
const fn in_control_sequence(character: char) -> Escape {
    if ends_sequence(character) {
        Escape::None
    } else {
        Escape::ControlSequence
    }
}

/// What follows the start of a control string.
const fn in_string(character: char) -> Escape {
    match character {
        // A bell ends a string, which is how terminals set a window title.
        '\u{7}' => Escape::None,
        '\u{1b}' => Escape::StringTerminator,
        _ => Escape::String,
    }
}

/// What follows an `ESC` inside a control string.
const fn after_string_escape(character: char) -> Escape {
    match character {
        '\\' => Escape::None,
        // A stray escape starts the search for a terminator again.
        '\u{1b}' => Escape::StringTerminator,
        _ => Escape::String,
    }
}

/// Whether a character survives into the shown text.
///
/// Newline and carriage return are kept because they are what a tail uses to
/// tell lines apart. Tab is kept because it is how output lines up. Every other
/// control character is noise on a display this small.
fn keeps(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\t') || !character.is_control()
}

/// Whether a byte ends a control sequence.
const fn ends_sequence(character: char) -> bool {
    matches!(character, '\u{40}'..='\u{7e}')
}

/// Whether a byte only leads up to the one that ends a sequence.
const fn is_intermediate(character: char) -> bool {
    matches!(character, '\u{20}'..='\u{2f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered(chunks: &[&[u8]]) -> String {
        let mut filter = OutputFilter::new();
        chunks.iter().map(|chunk| filter.push(chunk)).collect()
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(filtered(&[b"running tests\n"]), "running tests\n");
    }

    #[test]
    fn a_character_split_across_reads_appears_once_and_whole() {
        // The kernel does not respect character boundaries.
        let split = "café".as_bytes();
        let (head, tail) = split.split_at(4);
        assert_eq!(filtered(&[head, tail]), "café");
    }

    #[test]
    fn an_unfinished_character_is_held_back_rather_than_mangled() {
        let mut filter = OutputFilter::new();
        assert_eq!(filter.push(&[0xE2, 0x82]), "", "nothing is complete yet");
        assert_eq!(filter.push(&[0xAC]), "€");
    }

    #[test]
    fn invalid_bytes_are_shown_rather_than_silently_dropped() {
        // Corruption the operator cannot see is corruption they cannot report.
        assert_eq!(filtered(&[b"a\xffb"]), "a\u{fffd}b");
    }

    #[test]
    fn colour_sequences_are_removed_but_their_text_survives() {
        assert_eq!(filtered(&[b"\x1b[32mok\x1b[0m\n"]), "ok\n");
    }

    #[test]
    fn a_sequence_split_across_reads_is_still_removed() {
        assert_eq!(filtered(&[b"\x1b[3", b"2mok"]), "ok");
    }

    #[test]
    fn a_window_title_is_removed_however_it_is_terminated() {
        assert_eq!(filtered(&[b"\x1b]0;a title\x07done"]), "done");
        assert_eq!(filtered(&[b"\x1b]0;a title\x1b\\done"]), "done");
    }

    #[test]
    fn a_sequence_with_an_intermediate_byte_is_removed_whole() {
        // Character set selection, which shells emit on startup. Stopping one
        // character early would leave a stray `B` at the front of the output.
        assert_eq!(filtered(&[b"\x1b(Bready"]), "ready");
        assert_eq!(filtered(&[b"\x1b(", b"Bready"]), "ready");
    }

    #[test]
    fn a_two_character_sequence_takes_exactly_two_characters() {
        // Save cursor, which has no intermediate byte.
        assert_eq!(filtered(&[b"\x1b7ready"]), "ready");
    }

    #[test]
    fn line_structure_is_kept_and_other_control_characters_are_not() {
        assert_eq!(
            filtered(&[b"one\ttwo\r\nthree\x00\x07"]),
            "one\ttwo\r\nthree"
        );
    }

    #[test]
    fn a_cursor_move_does_not_swallow_the_text_after_it() {
        assert_eq!(filtered(&[b"\x1b[2K\x1b[1G50%"]), "50%");
    }

    #[test]
    fn the_held_back_bytes_never_grow_without_bound() {
        let mut filter = OutputFilter::new();
        for _ in 0..1000 {
            filter.push(&[0xE2, 0x82, 0xAC]);
        }
        assert!(filter.partial.len() < 4, "at most one unfinished character");
    }
}
