//! Reading what a session is doing off its screen.
//!
//! The fallback, for every session nothing better describes: a coding agent
//! that publishes no state of its own, a shell, or a Claude Code session that
//! has not registered yet because it is still asking whether to trust its
//! folder. Everything here is a reading of somebody else's interface, so it is
//! deliberately reluctant: it reports that a person is wanted only for
//! something that is plainly a question, and falls back to saying nothing
//! recognisable is happening rather than guessing.
//!
//! The wording it looks for comes from the programs themselves: Claude Code as
//! captured from a running session, and Codex from the screens its own tests
//! render.

use super::Activity;

/// The spinner frames a coding agent puts in its window title while it works.
const SPINNING: [char; 5] = ['◐', '◑', '◒', '◓', '✻'];

/// The prompts coding agents draw when they are waiting for an instruction.
///
/// Claude Code's, then Codex's. A selected choice in a list is drawn with the
/// same character, which is why questions are looked for first.
const PROMPTS: [char; 2] = ['❯', '›'];

/// How far back up the screen to look.
///
/// A terminal screen is tall and mostly history. What a session is doing now is
/// at the bottom of it.
const DEEP: usize = 12;

/// Works out what a session appears to be doing.
///
/// Reads the window title first, because a spinner there is unambiguous, then
/// the last of what is on screen.
pub fn activity_of(title: &str, screen: &str) -> Activity {
    if title.starts_with(SPINNING) {
        return Activity::Working;
    }

    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let bottom = || lines.iter().rev().take(DEEP);

    if bottom().any(|line| is_a_question(line)) {
        return Activity::NeedsDecision;
    }
    if bottom().any(|line| is_working(line)) {
        return Activity::Working;
    }

    match lines.iter().rev().find_map(|line| after_the_prompt(line)) {
        // Codex draws a suggestion in an empty prompt, dimmed, and dimming is
        // not something a screen's text carries. The footer is: it offers
        // shortcuts only while nothing has been typed.
        Some(typed) if !typed.is_empty() && !bottom().any(|line| offers_shortcuts(line)) => {
            Activity::Drafting
        }
        Some(_) => Activity::Ready,
        None => Activity::Quiet,
    }
}

/// Whether a line is part of a question being asked.
fn is_a_question(line: &str) -> bool {
    let trimmed = line.trim_start_matches(PROMPTS).trim();
    // A numbered choice: `1. Yes`, `2. No, and tell me why`. One of these on
    // its own is how both agents ask for approval, and prose almost never
    // begins this way.
    let numbered = trimmed.split_once('.').is_some_and(|(head, rest)| {
        head.len() <= 2
            && !head.is_empty()
            && head.chars().all(|c| c.is_ascii_digit())
            && rest.starts_with(' ')
    });

    let lower = line.to_lowercase();
    numbered
        || lower.contains("(y/n)")
        // The foot of a dialog whose choices are not numbered, such as Claude
        // Code asking whether to trust a folder, or Codex asking to continue.
        || lower.contains("enter to confirm")
        || lower.contains("enter to continue")
}

/// Whether a line says something is still going.
fn is_working(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("esc to interrupt") || lower.contains("ctrl+c to stop")
}

/// Whether a line is the footer an agent shows only while its prompt is empty.
fn offers_shortcuts(line: &str) -> bool {
    line.contains("? for shortcuts")
}

/// What has been typed at the prompt, when a line is the prompt.
fn after_the_prompt(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(PROMPTS)?;
    Some(rest.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen as Claude Code draws it, with whatever is going on at the
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
    fn a_dialog_whose_choices_are_not_numbered_is_still_a_question() {
        // Claude Code asking whether to trust a folder, as captured. The
        // selected choice sits at the prompt character, and reading it as
        // something typed would call a question an unsent message.
        let trust = " Quick safety check: Is this a project you created or one you trust?\n\
                     \n Security guide\n\
                     ❯ No, exit\n   Yes, I trust this folder\n\
                     Enter to confirm · Esc to cancel\n";
        assert_eq!(activity_of("", trust), Activity::NeedsDecision);
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

    /// Codex's prompt as its own tests render it, with the footer beneath.
    fn codex(prompt: &str, footer: &str) -> String {
        format!("\n{prompt}\n\n\n\n  {footer}                    100% context left\n")
    }

    #[test]
    fn codex_waiting_with_its_suggestion_showing_is_ready() {
        // The suggestion is dimmed on screen and indistinguishable from typing
        // in text, so the footer decides.
        assert_eq!(
            activity_of("", &codex("› Ask Codex to do anything", "? for shortcuts")),
            Activity::Ready
        );
    }

    #[test]
    fn codex_with_something_typed_is_drafting() {
        // Codex hides the shortcuts hint while there is text in the prompt.
        assert_eq!(activity_of("", &codex("› h", "")), Activity::Drafting);
    }

    #[test]
    fn codex_working_is_working() {
        let working = "• Working (12s • esc to interrupt)\n\n› Ask Codex to do anything\n";
        assert_eq!(activity_of("", working), Activity::Working);
    }

    #[test]
    fn codex_asking_for_approval_is_asking() {
        // Its approval dialog, as its own tests render it. The selected choice
        // is drawn with Codex's prompt character.
        let approval = "  Do you want to approve network access to \"example.com\"?\n\n\
                        › 1. Yes, just this once (y)\n\
                          2. Yes, and allow this host for this conversation (a)\n\
                          4. No, and tell Codex what to do differently (esc)\n\n\
                          Press enter to confirm or esc to cancel\n";
        assert_eq!(activity_of("", approval), Activity::NeedsDecision);
    }

    #[test]
    fn codex_asking_whether_to_trust_a_folder_is_asking() {
        // Captured from a running Codex.
        let trust = "  Do you trust the contents of this directory?\n\
                     › 1. Yes, continue\n  2. No, quit\n  Press enter to continue\n";
        assert_eq!(activity_of("", trust), Activity::NeedsDecision);
    }
}
