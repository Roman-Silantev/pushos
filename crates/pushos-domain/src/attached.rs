//! Sessions PushOS did not start.
//!
//! An operator with eight terminals already open should not have to close them
//! to put them on a pad. PushOS cannot own those sessions: a pseudo-terminal
//! belongs to whoever created it, and no amount of asking changes that. What it
//! can do is watch them and type into them, which is most of what a pad is for.
//!
//! What it watches is therefore a different thing from a terminal PushOS runs,
//! and is kept separate rather than pretended to be the same. A session here
//! can vanish without PushOS being told, has no exit status to collect, and
//! reports what it is doing rather than what it produced.

use std::fmt;
use std::str::FromStr;

use crate::ids::AttachedId;

/// A session running in a terminal PushOS did not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    /// The terminal device it is on.
    pub id: AttachedId,
    /// What the terminal calls itself.
    ///
    /// Whatever the program inside has set, which for a coding agent is
    /// usually what it is working on. That makes a far better label for a pad
    /// than a device name.
    pub title: String,
    /// Whether something is running in it right now.
    pub busy: bool,
}

impl Attached {
    /// The label for a pad or a display column.
    ///
    /// The title with any decoration the program put on the front removed, so
    /// eight sessions do not all begin with the same glyph and get cut to it.
    pub fn label(&self) -> &str {
        let trimmed = self
            .title
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .trim();
        if trimmed.is_empty() {
            self.id.as_str()
        } else {
            trimmed
        }
    }

    /// The short form of the device, for when there is nothing else to show.
    ///
    /// `/dev/ttys003` is mostly prefix; `ttys003` is what an operator would
    /// read out.
    pub fn device(&self) -> &str {
        self.id
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_else(|| self.id.as_str())
    }
}

/// Which attached session an action acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachedTarget {
    /// The session on this terminal device.
    ///
    /// Exact, and stable for as long as that terminal is open.
    Device(String),
    /// The first session whose title contains these words.
    ///
    /// What an operator actually remembers: they know which one is the sprint
    /// work, not which one is on `ttys004`.
    Titled(String),
    /// The one the operator most recently selected.
    Selected,
}

impl fmt::Display for AttachedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(device) => write!(f, "tty:{device}"),
            Self::Titled(title) => write!(f, "title:{title}"),
            Self::Selected => f.write_str("selected"),
        }
    }
}

impl FromStr for AttachedTarget {
    type Err = MalformedTarget;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() {
            return Err(MalformedTarget {
                written: text.to_owned(),
            });
        }

        if let Some(device) = text.strip_prefix("tty:") {
            let device = device.trim();
            return if device.is_empty() {
                Err(MalformedTarget {
                    written: text.to_owned(),
                })
            } else {
                Ok(Self::Device(device.to_owned()))
            };
        }
        if let Some(title) = text.strip_prefix("title:") {
            let title = title.trim();
            return if title.is_empty() {
                Err(MalformedTarget {
                    written: text.to_owned(),
                })
            } else {
                Ok(Self::Titled(title.to_owned()))
            };
        }
        if text == "selected" {
            return Ok(Self::Selected);
        }

        // Anything unprefixed is a title, because that is what an operator
        // writing a binding will reach for.
        Ok(Self::Titled(text.to_owned()))
    }
}

impl AttachedTarget {
    /// Whether a session answers to this target.
    pub fn matches(&self, session: &Attached) -> bool {
        match self {
            Self::Device(device) => {
                session.id.as_str() == device || session.device() == device.trim_start_matches('/')
            }
            Self::Titled(words) => session.title.to_lowercase().contains(&words.to_lowercase()),
            // Answered by whoever holds the selection, not by a session.
            Self::Selected => false,
        }
    }
}

/// A target that is not written in a form PushOS understands.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{written}` is not a session; write `tty:ttys003`, `title:some words`, or `selected`")]
pub struct MalformedTarget {
    /// What was written.
    pub written: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str) -> Attached {
        Attached {
            id: AttachedId::new(id),
            title: title.to_owned(),
            busy: false,
        }
    }

    #[test]
    fn a_target_survives_being_written_down_and_read_back() {
        for written in ["tty:ttys003", "title:sprint 2 setup", "selected"] {
            let target: AttachedTarget = written.parse().expect("well formed");
            assert_eq!(target.to_string(), written);
        }
    }

    #[test]
    fn anything_unprefixed_is_taken_for_a_title() {
        // Which is what an operator writing a binding will reach for: they
        // remember the sprint work, not which device it is on.
        assert_eq!(
            "Sprint 2".parse::<AttachedTarget>().expect("well formed"),
            AttachedTarget::Titled("Sprint 2".to_owned())
        );
    }

    #[test]
    fn an_empty_target_is_refused() {
        for written in ["", "   ", "tty:", "title:  "] {
            assert!(
                written.parse::<AttachedTarget>().is_err(),
                "`{written}` should be refused"
            );
        }
    }

    #[test]
    fn a_device_matches_with_or_without_its_directory() {
        let session = session("/dev/ttys003", "Sprint 2 setup");
        for written in ["tty:/dev/ttys003", "tty:ttys003"] {
            let target: AttachedTarget = written.parse().expect("well formed");
            assert!(target.matches(&session), "`{written}` should match");
        }
        assert!(
            !"tty:ttys004"
                .parse::<AttachedTarget>()
                .expect("well formed")
                .matches(&session)
        );
    }

    #[test]
    fn a_title_matches_on_part_of_itself_whatever_the_case() {
        // Titles are long and change as the work does. Matching the whole
        // thing exactly would mean a binding that stopped working the moment
        // the session moved on.
        let session = session("/dev/ttys003", "Review project tasks before integrations");
        for written in ["review project", "INTEGRATIONS", "title:tasks"] {
            let target: AttachedTarget = written.parse().expect("well formed");
            assert!(target.matches(&session), "`{written}` should match");
        }
        assert!(
            !"sprint"
                .parse::<AttachedTarget>()
                .expect("ok")
                .matches(&session)
        );
    }

    #[test]
    fn the_selection_is_answered_by_whoever_holds_it() {
        // A session cannot know whether it is the selected one, so it never
        // claims to be.
        let session = session("/dev/ttys003", "Anything");
        assert!(!AttachedTarget::Selected.matches(&session));
    }

    #[test]
    fn a_label_drops_the_decoration_a_program_put_on_the_front() {
        // Eight sessions all beginning with the same glyph would all be cut to
        // it on a display eight columns wide.
        assert_eq!(
            session("/dev/ttys003", "✳ Sprint 2 setup").label(),
            "Sprint 2 setup"
        );
        assert_eq!(
            session("/dev/ttys003", "◐ Push OS system").label(),
            "Push OS system"
        );
    }

    #[test]
    fn a_session_with_no_title_is_named_by_its_device() {
        let session = session("/dev/ttys003", "   ");
        assert_eq!(session.label(), "/dev/ttys003");
        assert_eq!(session.device(), "ttys003");
    }
}
