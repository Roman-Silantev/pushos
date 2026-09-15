//! Where a process is running.
//!
//! Claude Code says which process each of its sessions is, and nothing about
//! where that process is. The process table says the rest: the terminal device
//! it is attached to, which matches it to a Terminal tab or a tmux pane, and
//! the application it was started from, which is where the operator has to go
//! when PushOS cannot reach it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pushos_domain::ports::{ProcessRunner, ProcessSpec};

/// The process table, addressed absolutely so `PATH` cannot redirect it.
const PS: &str = "/bin/ps";

/// How many parents to walk up before giving up. A real chain from a session
/// to its application is a handful long.
const MOST_ANCESTORS: usize = 64;

/// Where one process is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Whereabouts {
    /// The terminal device it is attached to, if it has one.
    pub(super) device: Option<String>,
    /// The application it was started from, such as `Visual Studio Code`.
    pub(super) application: Option<String>,
    /// Whether PushOS started it, directly or through something it started.
    pub(super) ours: bool,
}

/// Looks processes up in the process table.
#[derive(Debug)]
pub(super) struct Hosts {
    processes: Arc<dyn ProcessRunner>,
}

impl Hosts {
    pub(super) fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self { processes }
    }

    /// Where each of these processes is. One that cannot be found is left out.
    pub(super) async fn locate(&self, pids: &[u32]) -> HashMap<u32, Whereabouts> {
        if pids.is_empty() {
            return HashMap::new();
        }
        let spec = ProcessSpec::new(PS, ["-axo".to_owned(), "pid=,ppid=,tty=,comm=".to_owned()])
            .within(Duration::from_secs(5))
            .capturing(4 * 1024 * 1024);

        let Ok(outcome) = self.processes.run(&spec).await else {
            return HashMap::new();
        };
        let table = parse(&outcome.stdout_tail);
        let me = std::process::id();
        pids.iter()
            .filter_map(|&pid| whereabouts(&table, pid, me).map(|found| (pid, found)))
            .collect()
    }
}

/// The terminal device of the nearest process above `pid` that has one.
///
/// A command an agent runs for a hook runs with no terminal of its own, but
/// the agent that started it has one, and that device is how the session is
/// known.
pub async fn terminal_above(processes: &dyn ProcessRunner, pid: u32) -> Option<String> {
    let spec = ProcessSpec::new(PS, ["-axo".to_owned(), "pid=,ppid=,tty=,comm=".to_owned()])
        .within(Duration::from_secs(5))
        .capturing(4 * 1024 * 1024);
    let outcome = processes.run(&spec).await.ok()?;
    nearest_device(&parse(&outcome.stdout_tail), pid)
}

/// The first device found walking up from `pid`, itself included.
fn nearest_device(table: &HashMap<u32, Row>, pid: u32) -> Option<String> {
    let mut at = pid;
    for _ in 0..MOST_ANCESTORS {
        let row = table.get(&at)?;
        if let Some(device) = &row.device {
            return Some(device.clone());
        }
        if row.parent == at || row.parent == 0 {
            return None;
        }
        at = row.parent;
    }
    None
}

/// One row of the process table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    parent: u32,
    device: Option<String>,
    command: String,
}

/// Reads `ps -axo pid=,ppid=,tty=,comm=`.
fn parse(text: &str) -> HashMap<u32, Row> {
    text.lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut field = || {
                let (value, after) = rest.split_once(char::is_whitespace)?;
                rest = after.trim_start();
                Some(value)
            };
            let pid = field()?.parse().ok()?;
            let parent = field()?.parse().ok()?;
            let tty = field()?;
            let device = (tty != "??").then(|| format!("/dev/{tty}"));
            Some((
                pid,
                Row {
                    parent,
                    device,
                    // The command is the rest of the line, spaces and all.
                    command: rest.trim_end().to_owned(),
                },
            ))
        })
        .collect()
}

/// Where one process is, from the table.
fn whereabouts(table: &HashMap<u32, Row>, pid: u32, me: u32) -> Option<Whereabouts> {
    let row = table.get(&pid)?;
    let mut found = Whereabouts {
        device: row.device.clone(),
        application: None,
        ours: false,
    };

    let mut at = row.parent;
    for _ in 0..MOST_ANCESTORS {
        if at == me {
            found.ours = true;
        }
        let Some(ancestor) = table.get(&at) else {
            break;
        };
        if found.application.is_none() {
            found.application = application(&ancestor.command);
        }
        if at <= 1 || ancestor.parent == at {
            break;
        }
        at = ancestor.parent;
    }
    Some(found)
}

/// The application a program belongs to, from its path.
///
/// The outermost bundle, because an editor runs its terminals from a helper
/// bundle inside its own: `/Applications/Visual Studio Code.app/Contents/
/// Frameworks/Code Helper.app/…` belongs to Visual Studio Code.
fn application(command: &str) -> Option<String> {
    let (before, _) = command.split_once(".app/")?;
    let name = before.rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    /// A process table like this Mac's, trimmed to the rows that matter.
    const TABLE: &str = "\
        1     0 ??       /sbin/launchd
      842     1 ??       /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal
      893   842 ttys002  login
      894   893 ttys002  -zsh
     1622   894 ttys002  claude
      900     1 ??       /Applications/Visual Studio Code.app/Contents/MacOS/Code
      901   900 ??       /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)
      902   901 ttys010  /bin/zsh
     1700   902 ttys010  claude
     1701   901 ??       claude
      950     1 ??       tmux
      951   950 ttys006  -zsh
     1800   951 ttys006  claude
     4000     1 ??       pushos
     4001  4000 ??       node
     4002  4001 ??       claude
    ";

    fn located(pid: u32) -> Whereabouts {
        whereabouts(&parse(TABLE), pid, 4000).expect("in the table")
    }

    #[test]
    fn a_session_in_terminal_is_on_its_tab_device_and_in_terminal() {
        let found = located(1622);
        assert_eq!(found.device.as_deref(), Some("/dev/ttys002"));
        assert_eq!(found.application.as_deref(), Some("Terminal"));
        assert!(!found.ours);
    }

    #[test]
    fn a_session_in_an_editor_belongs_to_the_editor_not_its_helper() {
        assert_eq!(
            located(1700).application.as_deref(),
            Some("Visual Studio Code")
        );
        let panel = located(1701);
        assert_eq!(panel.device, None, "the editor's panel has no terminal");
        assert_eq!(panel.application.as_deref(), Some("Visual Studio Code"));
    }

    #[test]
    fn a_session_in_tmux_has_its_pane_device_and_no_application() {
        let found = located(1800);
        assert_eq!(found.device.as_deref(), Some("/dev/ttys006"));
        assert_eq!(found.application, None);
    }

    #[test]
    fn a_session_pushos_started_itself_is_known_as_ours() {
        // An agent PushOS runs is already on the surface as an agent, and must
        // not appear a second time as somebody else's session.
        assert!(located(4002).ours);
    }

    #[test]
    fn a_process_that_is_not_in_the_table_is_left_out() {
        assert_eq!(whereabouts(&parse(TABLE), 99_999, 4000), None);
    }

    #[test]
    fn a_parent_that_is_its_own_parent_does_not_loop() {
        let table = parse("  10  11 ??  a\n  11  11 ??  b\n");
        assert!(whereabouts(&table, 10, 4000).is_some());
    }

    #[test]
    fn a_hook_with_no_terminal_is_placed_on_the_terminal_of_the_agent_above_it() {
        // The agent in a Terminal tab runs a shell, which runs the hook, and
        // neither of those has a device.
        let table = parse(
            "  894   893 ttys002  -zsh\n 1622   894 ttys002  claude\n 1650  1622 ??  /bin/sh\n 1651  1650 ??  pushos\n",
        );
        assert_eq!(
            nearest_device(&table, 1651).as_deref(),
            Some("/dev/ttys002")
        );
    }

    #[test]
    fn a_hook_under_an_editors_panel_has_no_terminal() {
        let table =
            parse("  901     1 ??  Code Helper\n 1701   901 ??  claude\n 1750  1701 ??  pushos\n");
        assert_eq!(nearest_device(&table, 1750), None);
    }

    #[tokio::test]
    async fn nothing_is_asked_when_there_is_nothing_to_find() {
        let processes = FakeProcesses::new();
        let hosts = Hosts::new(Arc::new(processes.clone()));
        assert!(hosts.locate(&[]).await.is_empty());
        assert!(processes.spawned().is_empty());
    }
}
