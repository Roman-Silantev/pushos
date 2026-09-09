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
    ///
    /// True for every window with a program in it, which is every window worth
    /// showing, so it separates nothing on its own.
    pub busy: bool,
    /// What it appears to be doing.
    pub activity: Activity,
}

/// What a session appears to be doing.
///
/// Read off the screen and the window title, because there is nothing else to
/// read: PushOS did not start these and is told nothing about them. That makes
/// every answer here a reading rather than a fact, which is why the one that
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
    /// The light this state shows.
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

/// The spinner frames a coding agent puts in its window title while it works.
const SPINNING: [char; 5] = ['◐', '◑', '◒', '◓', '✻'];

/// The prompt a coding agent draws when it is waiting for an instruction.
const PROMPT: char = '❯';

/// Works out what a session appears to be doing.
///
/// Reads the window title first, because a spinner there is unambiguous, then
/// the last of what is on screen. Everything here is a reading of somebody
/// else's interface, so it is deliberately reluctant: it reports that a person
/// is wanted only for something that is plainly a question, and falls back to
/// saying nothing recognisable is happening rather than guessing.
pub fn activity_of(title: &str, screen: &str) -> Activity {
    if title.starts_with(SPINNING) {
        return Activity::Working;
    }

    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();

    if lines
        .iter()
        .rev()
        .take(DEEP)
        .any(|line| is_a_question(line))
    {
        return Activity::NeedsDecision;
    }
    if lines.iter().rev().take(DEEP).any(|line| is_working(line)) {
        return Activity::Working;
    }

    match lines.iter().rev().find_map(|line| after_the_prompt(line)) {
        Some(typed) if !typed.is_empty() => Activity::Drafting,
        Some(_) => Activity::Ready,
        None => Activity::Quiet,
    }
}

/// How far back up the screen to look.
///
/// A terminal screen is tall and mostly history. What a session is doing now is
/// at the bottom of it.
const DEEP: usize = 12;

/// Whether a line is one of a set of choices being offered.
fn is_a_question(line: &str) -> bool {
    let trimmed = line.trim_start_matches(PROMPT).trim();
    // A numbered choice: `1. Yes`, `2. No, and tell me why`. One of these on
    // its own is how every agent asks, and prose almost never begins this way.
    let numbered = trimmed.split_once('.').is_some_and(|(head, rest)| {
        head.len() <= 2
            && !head.is_empty()
            && head.chars().all(|c| c.is_ascii_digit())
            && rest.starts_with(' ')
    });

    numbered || line.to_lowercase().contains("(y/n)")
}

/// Whether a line says something is still going.
fn is_working(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("esc to interrupt") || lower.contains("ctrl+c to stop")
}

/// What has been typed at the prompt, when a line is the prompt.
fn after_the_prompt(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(PROMPT)?;
    Some(rest.trim().to_owned())
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
    /// The session in this position of what the surface is currently showing.
    ///
    /// One-based, and counted within the bank of eight in view, so a pad means
    /// "the third of these" rather than one particular window. That is what
    /// lets eight pads serve any number of sessions: the bank moves and the
    /// pads keep their meaning.
    Slot(u8),
}

/// How many sessions the surface shows at once.
///
/// Eight, because there are eight pads in a row, eight buttons under the
/// screen and eight columns on it. Anything else would need an operator to
/// count across.
pub const BANK: usize = 8;

impl fmt::Display for AttachedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(device) => write!(f, "tty:{device}"),
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
        if let Some(at) = text.strip_prefix("slot:") {
            return at
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|at| (1..=BANK_MAX).contains(at))
                .map(Self::Slot)
                .ok_or(MalformedTarget {
                    written: text.to_owned(),
                });
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
            // Both answered by whoever holds the surface rather than by a
            // session: one is a position among others, and the other is a
            // choice. Neither is something a session can know about itself.
            Self::Slot(_) | Self::Selected => false,
        }
    }
}

/// The highest slot a pad may name.
///
/// The same eight as [`BANK`], written in the type a slot is counted in. The
/// two are kept in step by a test rather than by arithmetic, because a bank is
/// a handful of controls on a physical surface and not a number that grows.
const BANK_MAX: u8 = 8;

/// A target that is not written in a form PushOS understands.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{written}` is not a session; write `tty:ttys003`, `title:some words`, `slot:3` or `selected`"
)]
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
            activity: Activity::default(),
        }
    }

    /// A screen as a coding agent draws it, with whatever is going on at the
    /// bottom.
    fn screen(bottom: &str) -> String {
        format!(
            "  an earlier answer that ran to several lines\n\n\
             {}\n\
             {bottom}\n\
             {}\n  auto mode on (shift+tab to cycle)\n",
            "─".repeat(40),
            "─".repeat(40)
        )
    }

    #[test]
    fn a_spinner_in_the_title_means_it_is_working() {
        for spinning in ["◐ Push OS system", "◑ Sprint 2", "✻ Anything"] {
            assert_eq!(
                activity_of(spinning, &screen("❯ ")),
                Activity::Working,
                "`{spinning}`"
            );
        }
    }

    #[test]
    fn an_empty_prompt_means_it_is_waiting_for_you() {
        assert_eq!(activity_of("✳ Sprint 2", &screen("❯ ")), Activity::Ready);
    }

    #[test]
    fn something_typed_and_not_sent_is_not_the_same_as_waiting() {
        // A session left mid-sentence looks exactly like one waiting for a
        // first instruction, and is not.
        assert_eq!(
            activity_of("✳ Sprint 2", &screen("❯ yes lets clean the vm")),
            Activity::Drafting
        );
    }

    #[test]
    fn a_numbered_choice_means_it_is_asking_you_something() {
        // The one state worth interrupting someone for.
        let asking = "Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and do not ask again\n  3. No";
        assert_eq!(activity_of("✳ Sprint 2", asking), Activity::NeedsDecision);
        assert!(Activity::NeedsDecision.wants_a_person());
    }

    #[test]
    fn a_yes_or_no_question_counts_too() {
        assert_eq!(
            activity_of("✳ Sprint 2", &screen("Overwrite the file? (y/n)")),
            Activity::NeedsDecision
        );
    }

    #[test]
    fn prose_that_merely_looks_like_a_list_is_not_a_question() {
        // Being wrong here would light a pad amber and send the operator to a
        // window that wanted nothing.
        for prose in [
            "I found 3. of them in the logs",
            "the version is 1.2. something",
            "e.g. this is not a choice",
        ] {
            assert_ne!(
                activity_of("✳ Sprint 2", &screen(prose)),
                Activity::NeedsDecision,
                "`{prose}`"
            );
        }
    }

    #[test]
    fn a_line_saying_it_can_be_interrupted_means_it_is_working() {
        assert_eq!(
            activity_of("✳ Sprint 2", &screen("Thinking... (esc to interrupt)")),
            Activity::Working
        );
    }

    #[test]
    fn a_window_with_nothing_recognisable_in_it_says_so() {
        // A plain shell is not a coding session, and colouring it as one would
        // be a guess dressed up as information.
        assert_eq!(
            activity_of("bash", "user@host ~ %\nls\nfile.txt\n"),
            Activity::Quiet
        );
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
    fn a_target_survives_being_written_down_and_read_back() {
        for written in ["tty:ttys003", "title:sprint 2 setup", "slot:3", "selected"] {
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
