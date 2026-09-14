//! Naming which session an action acts on.

use std::fmt;
use std::str::FromStr;

use super::Attached;

/// Which attached session an action acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachedTarget {
    /// The session on this terminal device.
    ///
    /// Exact, and stable for as long as that terminal is open.
    Device(String),
    /// The session kept under this name.
    ///
    /// The one form that outlives the terminal showing it: a tmux session keeps
    /// its name when its window is closed, and when PushOS restarts. That makes
    /// it the form a pad should be bound with.
    Named(String),
    /// The session with exactly this identity, for one that is on no terminal
    /// device PushOS can see, such as one in Visual Studio Code's own panel.
    Exact(String),
    /// The first session whose title contains these words.
    ///
    /// What an operator actually remembers: they know which one is the sprint
    /// work, not which one is on `ttys004`.
    Titled(String),
    /// The one the operator most recently selected.
    Selected,
    /// The session in this position of what the surface is currently showing.
    ///
    /// One-based, and counted within the bank of eight in view, so a pad means
    /// "the third of these" rather than one particular window. That is what
    /// lets eight pads serve any number of sessions: the bank moves and the
    /// pads keep their meaning.
    Slot(u8),
}

impl AttachedTarget {
    /// The most exact way to name a session that is open now.
    ///
    /// By its name when it has one, because that is what still means the same
    /// session tomorrow, then by its device, then by its identity.
    pub fn exactly(session: &Attached) -> Self {
        if let Some(name) = &session.name {
            return Self::Named(name.clone());
        }
        if session.id.as_str().starts_with(DEVICE_DIRECTORY) {
            return Self::Device(session.id.to_string());
        }
        Self::Exact(session.id.to_string())
    }

    /// Whether a session answers to this target.
    pub fn matches(&self, session: &Attached) -> bool {
        match self {
            Self::Device(device) => {
                session.id.as_str() == device || session.device() == device.trim_start_matches('/')
            }
            Self::Named(name) => session
                .name
                .as_deref()
                .is_some_and(|held| held.eq_ignore_ascii_case(name)),
            Self::Exact(id) => session.id.as_str() == id,
            Self::Titled(words) => session.title.to_lowercase().contains(&words.to_lowercase()),
            // Both answered by whoever holds the surface rather than by a
            // session: one is a position among others, and the other is a
            // choice. Neither is something a session can know about itself.
            Self::Slot(_) | Self::Selected => false,
        }
    }
}

/// Where terminal devices live, which is how a device is told from any other
/// identity.
const DEVICE_DIRECTORY: &str = "/dev/";

/// The highest slot a pad may name.
///
/// The same eight as [`BANK`](super::BANK), written in the type a slot is counted in. The
/// two are kept in step by a test rather than by arithmetic, because a bank is
/// a handful of controls on a physical surface and not a number that grows.
const BANK_MAX: u8 = 8;

impl fmt::Display for AttachedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(device) => write!(f, "tty:{device}"),
            Self::Named(name) => write!(f, "name:{name}"),
            Self::Exact(id) => write!(f, "id:{id}"),
            Self::Titled(title) => write!(f, "title:{title}"),
            Self::Slot(at) => write!(f, "slot:{at}"),
            Self::Selected => f.write_str("selected"),
        }
    }
}

impl FromStr for AttachedTarget {
    type Err = MalformedTarget;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let malformed = || MalformedTarget {
            written: text.to_owned(),
        };
        if text.is_empty() {
            return Err(malformed());
        }

        let words = |rest: &str| {
            let rest = rest.trim();
            if rest.is_empty() {
                Err(malformed())
            } else {
                Ok(rest.to_owned())
            }
        };

        if let Some(device) = text.strip_prefix("tty:") {
            return words(device).map(Self::Device);
        }
        if let Some(name) = text.strip_prefix("name:") {
            return words(name).map(Self::Named);
        }
        if let Some(id) = text.strip_prefix("id:") {
            return words(id).map(Self::Exact);
        }
        if let Some(title) = text.strip_prefix("title:") {
            return words(title).map(Self::Titled);
        }
        if let Some(at) = text.strip_prefix("slot:") {
            return at
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|at| (1..=BANK_MAX).contains(at))
                .map(Self::Slot)
                .ok_or_else(malformed);
        }
        if text == "selected" {
            return Ok(Self::Selected);
        }

        // Anything unprefixed is a title, because that is what an operator
        // writing a binding will reach for.
        Ok(Self::Titled(text.to_owned()))
    }
}

/// A target that is not written in a form PushOS understands.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{written}` is not a session; write `name:client-1`, `tty:ttys003`, `title:some words`, \
     `slot:3` or `selected`"
)]
pub struct MalformedTarget {
    /// What was written.
    pub written: String,
}

#[cfg(test)]
mod tests {
    use super::super::{Activity, Attached, BANK};
    use super::*;

    fn session(id: &str, title: &str) -> Attached {
        Attached::new(id, title, Activity::default(), "Terminal")
    }

    #[test]
    fn a_target_survives_being_written_down_and_read_back() {
        for written in [
            "tty:ttys003",
            "name:client-1",
            "id:claude:0d2c",
            "title:sprint 2 setup",
            "slot:3",
            "selected",
        ] {
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
    fn a_slot_may_name_every_control_in_the_bank_and_no_more() {
        assert_eq!(usize::from(BANK_MAX), BANK, "the two must stay in step");
    }

    #[test]
    fn a_slot_outside_the_bank_is_refused() {
        // There are eight pads in a row. A binding naming a ninth would be one
        // that never fired, and never saying so is worse than refusing it.
        for written in ["slot:0", "slot:9", "slot:100", "slot:x", "slot:"] {
            assert!(
                written.parse::<AttachedTarget>().is_err(),
                "`{written}` should be refused"
            );
        }
        for at in 1..=8 {
            assert_eq!(
                format!("slot:{at}")
                    .parse::<AttachedTarget>()
                    .expect("valid"),
                AttachedTarget::Slot(at)
            );
        }
    }

    #[test]
    fn a_slot_is_answered_by_the_surface_rather_than_by_a_session() {
        // A session cannot know where it sits among the others, any more than
        // it can know whether it is the selected one.
        let session = session("/dev/ttys003", "Anything");
        assert!(!AttachedTarget::Slot(1).matches(&session));
        assert!(!AttachedTarget::Selected.matches(&session));
    }

    #[test]
    fn an_empty_target_is_refused() {
        for written in ["", "   ", "tty:", "title:  ", "name:", "id: "] {
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
    fn a_name_matches_the_whole_name_and_nothing_else() {
        // Unlike a title, a name is chosen, so `client-1` must not also answer
        // for `client-10`.
        let session = session("/dev/ttys003", "Sprint 2").named("client-10");
        assert!(AttachedTarget::Named("CLIENT-10".to_owned()).matches(&session));
        assert!(!AttachedTarget::Named("client-1".to_owned()).matches(&session));
        assert!(
            !AttachedTarget::Named("client-1".to_owned())
                .matches(&self::session("/dev/ttys004", "client-1")),
            "a title is not a name"
        );
    }

    #[test]
    fn a_session_is_named_exactly_by_the_most_lasting_form_it_has() {
        // The bug this replaces: Studio was offered `device:…`, which nothing
        // understood, so a control bound from Studio never found its session.
        let named = session("/dev/ttys003", "Sprint").named("client-1");
        let tab = session("/dev/ttys004", "Sprint");
        let panel = session("claude:0d2c", "nemo-claw").watched_only();

        for (session, written) in [
            (&named, "name:client-1"),
            (&tab, "tty:/dev/ttys004"),
            (&panel, "id:claude:0d2c"),
        ] {
            let target = AttachedTarget::exactly(session);
            assert_eq!(target.to_string(), written);
            let read_back: AttachedTarget = written.parse().expect("well formed");
            assert!(
                read_back.matches(session),
                "`{written}` must find the session it was written for"
            );
        }
    }
}
