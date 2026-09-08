//! What a binding means when it names a terminal.
//!
//! A control is bound before the terminal it acts on exists. "The test run"
//! must therefore mean the test run whether it is the one from this morning or
//! one started thirty seconds ago, which is what naming a terminal buys.

use std::fmt;

use crate::ids::SessionId;

/// Which terminal an action acts on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TerminalTarget {
    /// The terminal with this name, whichever one currently holds it.
    ///
    /// The form a binding should almost always use: it survives the terminal
    /// being closed and opened again.
    Named(String),
    /// One specific terminal, by identifier.
    ///
    /// Exact and fragile, and only correct for a layout built around the
    /// terminals that happen to be running now.
    Session(SessionId),
    /// Whichever terminal the operator most recently selected.
    Selected,
}

impl fmt::Display for TerminalTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => write!(f, "name:{name}"),
            Self::Session(session) => write!(f, "session:{session}"),
            Self::Selected => f.write_str("selected"),
        }
    }
}

impl std::str::FromStr for TerminalTarget {
    type Err = MalformedTerminalTarget;

    /// Reads the form a binding writes:
    ///
    /// ```text
    /// name:tests
    /// session:abc123
    /// selected
    /// ```
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() || text == "selected" {
            return Ok(Self::Selected);
        }

        let (key, value) = text
            .split_once(':')
            .ok_or_else(|| MalformedTerminalTarget(text.to_owned()))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(MalformedTerminalTarget(text.to_owned()));
        }

        match key.trim() {
            "name" | "terminal" => Ok(Self::Named(value.to_owned())),
            "session" => Ok(Self::Session(SessionId::new(value))),
            _ => Err(MalformedTerminalTarget(text.to_owned())),
        }
    }
}

/// A terminal target that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` does not name a terminal; expected `name:tests`, `session:<id>` or `selected`")]
pub struct MalformedTerminalTarget(pub String);

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    fn parse(text: &str) -> Result<TerminalTarget, MalformedTerminalTarget> {
        TerminalTarget::from_str(text)
    }

    #[test]
    fn a_named_terminal_survives_being_written_out_and_read_back() {
        // Configuration is round-tripped by Studio, so this has to hold
        // exactly or an edit elsewhere in the file would rewrite a binding.
        for text in ["name:tests", "session:abc123", "selected"] {
            let target = parse(text).expect("the form is valid");
            assert_eq!(target.to_string(), text);
        }
    }

    #[test]
    fn an_empty_target_means_whatever_is_selected() {
        assert_eq!(parse(""), Ok(TerminalTarget::Selected));
        assert_eq!(parse("  "), Ok(TerminalTarget::Selected));
    }

    #[test]
    fn terminal_is_accepted_as_a_synonym_for_name() {
        assert_eq!(
            parse("terminal:tests"),
            Ok(TerminalTarget::Named("tests".to_owned()))
        );
    }

    #[test]
    fn surrounding_space_is_ignored() {
        assert_eq!(
            parse("  name: tests  "),
            Ok(TerminalTarget::Named("tests".to_owned()))
        );
    }

    #[test]
    fn a_target_with_nothing_after_the_colon_is_refused() {
        // Silently treating this as "selected" would bind a control to
        // whatever happened to be chosen, which is not what was written.
        assert!(parse("name:").is_err());
        assert!(parse("session:  ").is_err());
    }

    #[test]
    fn an_unknown_kind_is_refused_rather_than_guessed_at() {
        assert!(parse("role:builder").is_err());
        assert!(parse("tests").is_err());
    }

    #[test]
    fn the_failure_says_what_a_terminal_target_looks_like() {
        let error = parse("tests").expect_err("bare names are not targets");
        let message = error.to_string();
        assert!(message.contains("name:tests"), "{message}");
        assert!(message.contains("selected"), "{message}");
    }
}
