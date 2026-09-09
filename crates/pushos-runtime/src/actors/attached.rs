//! Keeping the display up to date with sessions PushOS did not start.
//!
//! Nothing tells PushOS when a terminal window opens or closes, so this asks.
//! Asking costs a subprocess, which is why it is not asked often and why the
//! answer is what the display holds until the next one.

use std::sync::Arc;
use std::time::Duration;

use pushos_domain::attached::Attached;
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::AttachedSessions;
use pushos_ui::{SessionLine, Tone};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::shutdown::Shutdown;

/// How often PushOS asks what is open.
///
/// Often enough that a window opened while looking at the Push appears without
/// the operator wondering, seldom enough that PushOS is not running a
/// subprocess every frame.
pub(crate) const HOW_OFTEN: Duration = Duration::from_secs(3);

/// Asks what is open, over and over, and tells whoever is listening.
pub(crate) struct AttachedTask {
    sessions: Arc<dyn AttachedSessions>,
    found: watch::Sender<Vec<Attached>>,
    every: Duration,
}

impl std::fmt::Debug for AttachedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachedTask")
            .field("every", &self.every)
            .finish_non_exhaustive()
    }
}

impl AttachedTask {
    /// Builds a task over whatever can see other terminals.
    pub(crate) fn new(
        sessions: Arc<dyn AttachedSessions>,
    ) -> (Self, watch::Receiver<Vec<Attached>>) {
        let (found, watching) = watch::channel(Vec::new());
        (
            Self {
                sessions,
                found,
                every: HOW_OFTEN,
            },
            watching,
        )
    }

    /// Asks more or less often than the default.
    #[must_use]
    pub(crate) const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Asks until shutdown, telling whoever is listening what changed.
    pub(crate) async fn run(self, shutdown: Shutdown, mut changed: impl FnMut()) {
        let mut refused = false;

        loop {
            match self.sessions.discover().await {
                Ok(open) => {
                    refused = false;
                    // Sent only when something actually differs, so a display
                    // that has not changed is not redrawn three times a second.
                    if *self.found.borrow() != open {
                        debug!(sessions = open.len(), "the open terminals changed");
                        self.found.send_replace(open);
                        changed();
                    }
                }
                Err(error) => {
                    // Said once rather than every three seconds: a refused
                    // permission stays refused until the operator fixes it.
                    if !refused {
                        warn!(%error, "cannot see the terminals already open");
                        refused = true;
                    }
                }
            }

            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(self.every) => {}
            }
        }
    }
}

/// What the display should say about sessions PushOS did not start.
pub(crate) fn lines_for(sessions: &[Attached], selected: Option<&AttachedId>) -> Vec<SessionLine> {
    sessions
        .iter()
        .map(|session| {
            let mut line = SessionLine::new(
                session.label().to_owned(),
                if session.busy { "working" } else { "waiting" },
                // Never Attention: PushOS cannot tell whether one of these
                // wants something, and a surface that cried for attention
                // about eight windows at once would be worse than one that
                // said nothing.
                if session.busy {
                    Tone::Active
                } else {
                    Tone::Normal
                },
            )
            .with_detail(session.device().to_owned());

            if selected.is_some_and(|id| *id == session.id) {
                line = line.selected();
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(device: &str, title: &str, busy: bool) -> Attached {
        Attached {
            id: AttachedId::new(device),
            title: title.to_owned(),
            busy,
        }
    }

    #[test]
    fn a_session_is_named_by_what_it_is_working_on() {
        // Which is the whole reason these are worth showing: eight windows
        // called `ttys000` through `ttys007` would tell an operator nothing.
        let lines = lines_for(&[session("/dev/ttys003", "✳ Sprint 2 setup", true)], None);
        assert_eq!(lines[0].name, "Sprint 2 setup");
        assert_eq!(lines[0].detail.as_deref(), Some("ttys003"));
        assert_eq!(lines[0].state, "working");
    }

    #[test]
    fn the_selected_session_says_so() {
        let sessions = [
            session("/dev/ttys003", "Sprint", true),
            session("/dev/ttys004", "Review", false),
        ];
        let lines = lines_for(&sessions, Some(&AttachedId::new("/dev/ttys004")));

        assert!(!lines[0].selected);
        assert!(lines[1].selected);
    }

    #[test]
    fn nothing_here_ever_asks_for_attention() {
        // PushOS cannot tell whether one of these wants something. A surface
        // crying out about eight windows at once is worse than a quiet one.
        let sessions = [
            session("/dev/ttys003", "Sprint", true),
            session("/dev/ttys004", "Review", false),
        ];
        for line in lines_for(&sessions, None) {
            assert_ne!(line.tone, Tone::Attention);
        }
    }
}
