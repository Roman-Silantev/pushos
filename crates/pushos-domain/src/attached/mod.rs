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
//!
//! Some of them PushOS can only watch. A coding agent running in an editor's
//! own panel tells PushOS what it is doing, but there is no terminal to type
//! into, and saying so is better than pretending otherwise.

mod question;
mod screen;
mod target;

pub use question::{Decision, SessionQuestion};
pub use screen::activity_of;
pub use target::{AttachedTarget, MalformedTarget};

use crate::ids::AttachedId;

/// A session running somewhere PushOS did not start it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    /// The terminal device it is on, or, for one on no device PushOS can see,
    /// an identity its host gave it.
    pub id: AttachedId,
    /// What it calls itself.
    ///
    /// Whatever the program inside has set, which for a coding agent is
    /// usually what it is working on. That makes a far better label for a pad
    /// than a device name.
    pub title: String,
    /// What it appears to be doing.
    pub activity: Activity,
    /// A name it is kept under that outlives its title and its window.
    ///
    /// The name of the tmux session it runs in. `None` for a session in a
    /// window that has no name of its own to give.
    pub name: Option<String>,
    /// What it is running in, as an operator would look for it: `Terminal`,
    /// `tmux`, `Visual Studio Code`.
    pub host: String,
    /// What PushOS can do with it.
    pub reach: Reach,
}

/// What PushOS can do with a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Reach {
    /// Read it, type into it, press keys in it and bring it to the front.
    #[default]
    Typing,
    /// See what it is doing, and nothing else.
    ///
    /// A session whose host offers no way in from outside, such as a coding
    /// agent in an editor's panel.
    Watching,
    /// Give it an instruction, put it away, and open a window onto it.
    ///
    /// A session a coding agent keeps for PushOS without a terminal of its
    /// own. There is no screen to type at, so keys mean nothing to it, but it
    /// takes whole instructions, it can be put away to free the memory its
    /// process holds, and a window can be opened onto it when the operator
    /// wants to take over.
    Dispatching,
}

impl Attached {
    /// A session PushOS can type into, with no name of its own.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        activity: Activity,
        host: impl Into<String>,
    ) -> Self {
        Self {
            id: AttachedId::new(id.into()),
            title: title.into(),
            activity,
            name: None,
            host: host.into(),
            reach: Reach::Typing,
        }
    }

    /// The same session, kept under a name.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The same session, which PushOS can only watch.
    #[must_use]
    pub const fn watched_only(mut self) -> Self {
        self.reach = Reach::Watching;
        self
    }

    /// The same session, kept for PushOS by an agent rather than a terminal.
    #[must_use]
    pub const fn dispatched(mut self) -> Self {
        self.reach = Reach::Dispatching;
        self
    }

    /// Whether it takes an instruction, whether or not it is running now.
    pub const fn takes_instructions(&self) -> bool {
        matches!(self.reach, Reach::Typing | Reach::Dispatching)
    }

    /// Whether its process can be stopped and its conversation kept.
    pub const fn can_be_put_away(&self) -> bool {
        matches!(self.reach, Reach::Dispatching)
    }

    /// Whether PushOS can type into it.
    pub const fn can_type(&self) -> bool {
        matches!(self.reach, Reach::Typing)
    }

    /// The label for a pad or a display column.
    ///
    /// Its name when it has one, because a name was chosen for exactly this.
    /// Otherwise the title with any decoration the program put on the front
    /// removed, so eight sessions do not all begin with the same glyph and get
    /// cut to it.
    pub fn label(&self) -> &str {
        if let Some(name) = &self.name {
            return name;
        }
        let trimmed = self.title_words();
        if trimmed.is_empty() {
            self.id.as_str()
        } else {
            trimmed
        }
    }

    /// What goes beside the label: where to find it, or what it is doing.
    ///
    /// A named session's label already says where it is, so its title goes
    /// here instead. One PushOS can only watch says what it is in, because
    /// that is where the operator has to go to answer it. Anything else says
    /// which terminal it is on.
    pub fn detail(&self) -> &str {
        match (self.reach, &self.name) {
            // Neither has a terminal of its own to name: where to go to reach
            // it is the useful thing to say.
            (Reach::Watching | Reach::Dispatching, _) => &self.host,
            (Reach::Typing, Some(name)) => {
                let words = self.title_words();
                if words.is_empty() || words == name {
                    &self.host
                } else {
                    words
                }
            }
            (Reach::Typing, None) => self.device(),
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

    /// The title without the decoration a program put on its front.
    fn title_words(&self) -> &str {
        self.title
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .trim()
    }
}

/// What a session appears to be doing.
///
/// Told by the session itself where it says, and otherwise read off the screen
/// and the window title. A reading is not a fact, which is why the one that
/// matters most is also the most conservative.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Activity {
    /// It is asking the operator to decide something.
    ///
    /// The only state worth interrupting someone for, and the reason any of
    /// this is on a surface: eight windows and one of them is stuck on a
    /// question.
    NeedsDecision,
    /// It is working.
    Working,
    /// Something has been typed at its prompt and not sent.
    ///
    /// Worth its own colour: a session left mid-sentence looks exactly like
    /// one waiting for a first instruction, and is not.
    Drafting,
    /// It is waiting for the operator, with nothing typed.
    Ready,
    /// Nothing recognisable is running in it.
    #[default]
    Quiet,
}

impl Activity {
    /// Whether nothing is happening and nobody is needed.
    ///
    /// A session at an empty prompt, or a terminal with nothing running. Its pad
    /// stays dark: on a surface left on all day a light should mean something is
    /// going on or someone is wanted, and eight pads glowing because eight
    /// sessions are simply there would be eight lights worn for nothing and a
    /// question lost among them. The screen still says each one is ready.
    pub const fn is_resting(self) -> bool {
        matches!(self, Self::Ready | Self::Quiet)
    }

    /// The light this state shows on the screen, and on a pad unless it rests.
    pub const fn status_color(self) -> crate::color::StatusColor {
        use crate::color::StatusColor;
        match self {
            // Blinking amber, the same as anything else waiting on a person.
            Self::NeedsDecision => StatusColor::Waiting,
            Self::Working => StatusColor::Working,
            Self::Drafting => StatusColor::Workflow,
            Self::Ready => StatusColor::Complete,
            Self::Quiet => StatusColor::Idle,
        }
    }

    /// How it reads on the display.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NeedsDecision => "asking",
            Self::Working => "working",
            Self::Drafting => "unsent",
            Self::Ready => "ready",
            Self::Quiet => "quiet",
        }
    }

    /// Whether the operator is being waited for.
    pub const fn wants_a_person(self) -> bool {
        matches!(self, Self::NeedsDecision)
    }
}

/// How many sessions the surface shows at once.
///
/// Eight, because there are eight pads in a row, eight buttons under the
/// screen and eight columns on it. Anything else would need an operator to
/// count across.
pub const BANK: usize = 8;

/// Whether a name is one a session can be kept and called by.
///
/// Letters, digits, hyphens and underscores, up to 64 of them. Narrow on
/// purpose: a name is written in configuration, said on a display and handed
/// to the program that keeps the session, and in tmux a colon or a dot in a
/// name would be read as part of an address.
pub fn is_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str) -> Attached {
        Attached::new(id, title, Activity::default(), "Terminal")
    }

    #[test]
    fn only_the_state_that_wants_a_person_says_it_does() {
        for quiet in [
            Activity::Working,
            Activity::Drafting,
            Activity::Ready,
            Activity::Quiet,
        ] {
            assert!(!quiet.wants_a_person(), "`{quiet:?}`");
        }
    }

    #[test]
    fn every_state_looks_different_from_every_other() {
        // The whole point: eight pads the same colour tell an operator nothing.
        let all = [
            Activity::NeedsDecision,
            Activity::Working,
            Activity::Drafting,
            Activity::Ready,
            Activity::Quiet,
        ];
        for (index, one) in all.iter().enumerate() {
            for other in &all[index + 1..] {
                assert_ne!(
                    one.status_color(),
                    other.status_color(),
                    "`{one:?}` and `{other:?}` look the same"
                );
                assert_ne!(one.describe(), other.describe());
            }
        }
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
        assert_eq!(session.detail(), "ttys003");
    }

    #[test]
    fn a_named_session_is_labelled_by_its_name_and_says_what_it_is_doing_beside_it() {
        // The name was chosen for the pad; the title changes with the work.
        let session = session("/dev/ttys007", "✳ Fix the login flow").named("client-1");
        assert_eq!(session.label(), "client-1");
        assert_eq!(session.detail(), "Fix the login flow");

        let untitled = self::session("/dev/ttys008", "").named("client-2");
        assert_eq!(untitled.detail(), "Terminal", "somewhere to look, at least");
    }

    #[test]
    fn a_session_pushos_can_only_watch_says_where_to_go_to_answer_it() {
        let panel = Attached::new(
            "claude:0d2c",
            "nemo-claw-0a",
            Activity::NeedsDecision,
            "Visual Studio Code",
        )
        .watched_only();
        assert!(!panel.can_type());
        assert_eq!(panel.label(), "nemo-claw-0a");
        assert_eq!(panel.detail(), "Visual Studio Code");
    }

    #[test]
    fn only_a_session_with_nothing_happening_rests() {
        assert!(Activity::Ready.is_resting());
        assert!(Activity::Quiet.is_resting());
        for busy in [
            Activity::Working,
            Activity::NeedsDecision,
            Activity::Drafting,
        ] {
            assert!(!busy.is_resting(), "{busy:?} is worth a light");
        }
    }

    #[test]
    fn a_session_name_is_something_a_pad_can_say_and_tmux_can_keep() {
        for good in ["client-1", "api_worker", "A1"] {
            assert!(is_session_name(good), "`{good}`");
        }
        for bad in ["", "client 1", "client:1", "a.b", "../x", &"x".repeat(65)] {
            assert!(!is_session_name(bad), "`{bad}`");
        }
    }
}
