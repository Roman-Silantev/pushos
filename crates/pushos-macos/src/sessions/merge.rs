//! Putting what each source saw together into one list of sessions.
//!
//! The same session is often seen more than once. A Claude Code session in a
//! Terminal tab is one tab and one entry in Claude Code's list; a tmux session
//! open in a Terminal window is one pane and one tab showing it. Each is shown
//! once, described by whichever source knows it best, and remembered with
//! the way to reach it.
//!
//! Nothing here asks anything of the machine, so all of it is tested directly.

use std::collections::HashMap;

use pushos_domain::attached::{Activity, Attached, activity_of};

use super::claude::ClaudeSession;
use super::hosts::Whereabouts;
use super::terminal_app::Tab;
use super::tmux::{Client, Pane};

/// How to reach a session that was seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// Through Terminal, by its tab's device.
    Tab(String),
    /// Through tmux, by the pane and the session it belongs to.
    Pane { pane: String, session: String },
    /// Not at all: PushOS can see it and has no way in.
    Nowhere { application: Option<String> },
}

/// Everything the sources saw in one look.
#[derive(Debug, Default)]
pub(super) struct Seen {
    pub(super) tabs: Vec<Tab>,
    pub(super) panes: Vec<Pane>,
    pub(super) clients: Vec<Client>,
    pub(super) claude: Vec<ClaudeSession>,
    pub(super) whereabouts: HashMap<u32, Whereabouts>,
}

/// What a session is called when it runs in tmux.
pub(super) const TMUX: &str = "tmux";

/// What a Claude Code session is called when nothing says where it runs.
const CLAUDE_CODE: &str = "Claude Code";

/// One list of sessions, each once, with the way to reach it.
///
/// In a fixed order, so a pad keeps meaning the same session from one look to
/// the next: tmux sessions by name, because those are the ones kept for pads,
/// then Terminal tabs by device, then sessions PushOS can only watch.
pub(super) fn merge(seen: &Seen) -> Vec<(Attached, Route)> {
    let mut claude_on: HashMap<&str, &ClaudeSession> = HashMap::new();
    let mut elsewhere: Vec<(&ClaudeSession, Option<&Whereabouts>)> = Vec::new();
    for session in &seen.claude {
        let found = seen.whereabouts.get(&session.pid);
        // Already on the surface as an agent PushOS runs.
        if found.is_some_and(|found| found.ours) {
            continue;
        }
        match found.and_then(|found| found.device.as_deref()) {
            Some(device) => {
                claude_on.insert(device, session);
            }
            None => elsewhere.push((session, found)),
        }
    }

    let mut merged = Vec::new();
    let mut placed: Vec<&str> = Vec::new();

    let mut panes: Vec<&Pane> = seen.panes.iter().collect();
    panes.sort_by(|left, right| left.session.cmp(&right.session));
    for pane in panes {
        let claude = claude_on.get(pane.device.as_str()).copied();
        let session = Attached::new(
            pane.device.clone(),
            titled(&pane.title, claude),
            activity(&pane.title, &pane.screen, claude),
            TMUX,
        )
        .named(pane.session.clone());
        placed.push(&pane.device);
        merged.push((
            session,
            Route::Pane {
                pane: pane.id.clone(),
                session: pane.session.clone(),
            },
        ));
    }

    // A tab running `tmux attach` is a window onto a pane already listed.
    let showing_tmux = |tab: &Tab| {
        seen.clients
            .iter()
            .any(|client| client.device == tab.device)
    };
    for tab in seen.tabs.iter().filter(|tab| !showing_tmux(tab)) {
        let claude = claude_on.get(tab.device.as_str()).copied();
        let session = Attached::new(
            tab.device.clone(),
            titled(&tab.title, claude),
            activity(&tab.title, &tab.screen, claude),
            super::terminal_app::APPLICATION,
        );
        placed.push(&tab.device);
        merged.push((session, Route::Tab(tab.device.clone())));
    }

    // Claude Code sessions on a device no tab or pane is on: an editor's
    // terminal, or a terminal application PushOS cannot script.
    let mut unplaced: Vec<(&ClaudeSession, Option<&Whereabouts>)> = claude_on
        .iter()
        .filter(|(device, _)| !placed.contains(device))
        .map(|(_, session)| (*session, seen.whereabouts.get(&session.pid)))
        .chain(elsewhere)
        .collect();
    unplaced.sort_by(|left, right| left.0.name.cmp(&right.0.name));

    for (session, found) in unplaced {
        let application = found.and_then(|found| found.application.clone());
        let id = found
            .and_then(|found| found.device.clone())
            .unwrap_or_else(|| format!("claude:{}", session.id));
        let watched = Attached::new(
            id,
            session.name.clone(),
            session.activity,
            application
                .clone()
                .unwrap_or_else(|| CLAUDE_CODE.to_owned()),
        )
        .watched_only();
        merged.push((watched, Route::Nowhere { application }));
    }

    merged
}

/// What a session is doing: what Claude Code says, where it says anything.
///
/// Its word is taken over the screen's, which can mistake the numbered list at
/// the end of an answer for a question. The one thing only the screen knows is
/// that something has been typed and not sent.
fn activity(title: &str, screen: &str, claude: Option<&ClaudeSession>) -> Activity {
    let read = activity_of(title, screen);
    match claude.map(|claude| claude.activity) {
        Some(Activity::Ready) if read == Activity::Drafting => Activity::Drafting,
        Some(reported) => reported,
        None => read,
    }
}

/// A title for a session, falling back to Claude Code's name for it.
fn titled(title: &str, claude: Option<&ClaudeSession>) -> String {
    match claude {
        Some(claude) if title.trim().is_empty() => claude.name.clone(),
        _ => title.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::attached::Reach;

    use super::*;

    fn tab(device: &str, title: &str, screen: &str) -> Tab {
        Tab {
            device: device.to_owned(),
            title: title.to_owned(),
            screen: screen.to_owned(),
        }
    }

    fn pane(id: &str, device: &str, session: &str, screen: &str) -> Pane {
        Pane {
            id: id.to_owned(),
            device: device.to_owned(),
            session: session.to_owned(),
            title: String::new(),
            screen: screen.to_owned(),
        }
    }

    fn claude(pid: u32, name: &str, activity: Activity) -> ClaudeSession {
        ClaudeSession {
            pid,
            id: format!("id-{pid}"),
            name: name.to_owned(),
            activity,
        }
    }

    fn at(device: Option<&str>, application: Option<&str>) -> Whereabouts {
        Whereabouts {
            device: device.map(str::to_owned),
            application: application.map(str::to_owned),
            ours: false,
        }
    }

    #[test]
    fn claude_code_is_believed_over_the_screen() {
        // The end of an answer is often a numbered list, which the screen
        // alone reads as a question.
        let seen = Seen {
            tabs: vec![tab(
                "/dev/ttys002",
                "✳ Plan",
                "Next:\n1. Write tests\n2. Ship\n❯ ",
            )],
            claude: vec![claude(1622, "work-b5", Activity::Ready)],
            whereabouts: HashMap::from([(1622, at(Some("/dev/ttys002"), Some("Terminal")))]),
            ..Seen::default()
        };
        let merged = merge(&seen);
        assert_eq!(merged.len(), 1, "one tab, one session");
        assert_eq!(merged[0].0.activity, Activity::Ready);
    }

    #[test]
    fn only_the_screen_can_tell_that_something_was_typed_and_not_sent() {
        let seen = Seen {
            panes: vec![pane("%0", "/dev/ttys006", "client-1", "❯ fix the login")],
            claude: vec![claude(1800, "client-b5", Activity::Ready)],
            whereabouts: HashMap::from([(1800, at(Some("/dev/ttys006"), None))]),
            ..Seen::default()
        };
        assert_eq!(merge(&seen)[0].0.activity, Activity::Drafting);
    }

    #[test]
    fn a_session_nothing_describes_is_read_off_its_screen() {
        // Codex publishes no state of its own.
        let seen = Seen {
            panes: vec![pane(
                "%1",
                "/dev/ttys007",
                "client-2",
                "• Working (3s • esc to interrupt)",
            )],
            ..Seen::default()
        };
        assert_eq!(merge(&seen)[0].0.activity, Activity::Working);
    }

    #[test]
    fn a_tmux_session_is_kept_under_its_name_and_reached_through_tmux() {
        let seen = Seen {
            panes: vec![pane("%0", "/dev/ttys006", "client-1", "❯ ")],
            ..Seen::default()
        };
        let (session, route) = &merge(&seen)[0];
        assert_eq!(session.name.as_deref(), Some("client-1"));
        assert_eq!(session.host, TMUX);
        assert_eq!(
            *route,
            Route::Pane {
                pane: "%0".to_owned(),
                session: "client-1".to_owned()
            }
        );
    }

    #[test]
    fn a_window_showing_a_tmux_session_is_not_a_second_session() {
        let seen = Seen {
            tabs: vec![
                tab("/dev/ttys002", "tmux", "❯ "),
                tab("/dev/ttys003", "✳ Other work", "❯ "),
            ],
            panes: vec![pane("%0", "/dev/ttys006", "client-1", "❯ ")],
            clients: vec![Client {
                device: "/dev/ttys002".to_owned(),
                session: "client-1".to_owned(),
            }],
            ..Seen::default()
        };
        let ids: Vec<String> = merge(&seen)
            .iter()
            .map(|(session, _)| session.id.to_string())
            .collect();
        assert_eq!(ids, ["/dev/ttys006", "/dev/ttys003"]);
    }

    #[test]
    fn a_session_in_an_editor_is_shown_and_known_to_be_out_of_reach() {
        let seen = Seen {
            claude: vec![
                claude(1700, "editor-terminal", Activity::NeedsDecision),
                claude(1701, "editor-panel", Activity::Working),
            ],
            whereabouts: HashMap::from([
                (1700, at(Some("/dev/ttys010"), Some("Visual Studio Code"))),
                (1701, at(None, Some("Visual Studio Code"))),
            ]),
            ..Seen::default()
        };
        let merged = merge(&seen);
        assert_eq!(merged.len(), 2);

        // By name, so the panel comes before the terminal.
        let (terminal, _) = &merged[1];
        assert_eq!(terminal.id.as_str(), "/dev/ttys010");
        assert_eq!(terminal.label(), "editor-terminal");
        assert_eq!(terminal.detail(), "Visual Studio Code");
        assert_eq!(terminal.reach, Reach::Watching);
        assert!(
            terminal.activity.wants_a_person(),
            "and it still asks for one"
        );

        let (panel, route) = &merged[0];
        assert_eq!(panel.id.as_str(), "claude:id-1701");
        assert_eq!(
            *route,
            Route::Nowhere {
                application: Some("Visual Studio Code".to_owned())
            }
        );
    }

    #[test]
    fn a_session_pushos_runs_itself_is_not_listed_again() {
        let mut ours = at(None, None);
        ours.ours = true;
        let seen = Seen {
            claude: vec![claude(4002, "agent", Activity::Working)],
            whereabouts: HashMap::from([(4002, ours)]),
            ..Seen::default()
        };
        assert!(merge(&seen).is_empty());
    }

    #[test]
    fn the_order_is_tmux_by_name_then_terminal_then_the_rest() {
        let seen = Seen {
            tabs: vec![tab("/dev/ttys002", "tab", "")],
            panes: vec![
                pane("%1", "/dev/ttys007", "zeta", ""),
                pane("%0", "/dev/ttys006", "alpha", ""),
            ],
            claude: vec![claude(1701, "panel", Activity::Ready)],
            whereabouts: HashMap::from([(1701, at(None, Some("Cursor")))]),
            ..Seen::default()
        };
        let labels: Vec<String> = merge(&seen)
            .iter()
            .map(|(session, _)| session.label().to_owned())
            .collect();
        assert_eq!(labels, ["alpha", "zeta", "tab", "panel"]);
    }

    #[test]
    fn an_untitled_tab_running_claude_code_takes_its_name() {
        let seen = Seen {
            tabs: vec![tab("/dev/ttys002", "", "❯ ")],
            claude: vec![claude(1622, "nemo-claw-0a", Activity::Ready)],
            whereabouts: HashMap::from([(1622, at(Some("/dev/ttys002"), Some("Terminal")))]),
            ..Seen::default()
        };
        assert_eq!(merge(&seen)[0].0.label(), "nemo-claw-0a");
    }
}
