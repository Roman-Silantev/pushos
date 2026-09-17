//! Keeping the display up to date with sessions PushOS did not start.
//!
//! Nothing tells PushOS when a terminal window opens or closes, so this asks.
//! Asking costs a subprocess, which is why it is not asked often and why the
//! answer is what the display holds until the next one.

use std::sync::Arc;
use std::time::Duration;

use pushos_domain::attached::{Activity, Attached};
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

/// Says whether a session has work waiting for a free turn.
type WaitingWork = Box<dyn Fn(&AttachedId) -> bool + Send + Sync>;

/// Asks what is open, over and over, and tells whoever is listening.
pub(crate) struct AttachedTask {
    sessions: Arc<dyn AttachedSessions>,
    /// Which sessions have work waiting for a free turn.
    ///
    /// PushOS's own doing rather than anything a session says, and shown on
    /// the pad all the same: an operator looking at sixty-four of them needs
    /// to see that a pad they pressed is going to work, not that it did
    /// nothing.
    waiting: Option<WaitingWork>,
    found: watch::Sender<Vec<Attached>>,
    every: Duration,
    /// What the surface is doing, so looking can slow down when nobody could
    /// see the answer.
    view: Option<watch::Receiver<crate::actors::SurfaceView>>,
}

/// How much less often to look at the terminals when nobody could be looking at
/// the surface: none is attached, or it has gone dark.
///
/// Every look is an `osascript` process and some work for Terminal, around
/// thirty milliseconds of CPU measured on this machine. Every three seconds
/// that is a steady one percent of a core for as long as PushOS runs, most of
/// it spent on a surface nobody is reading. A question still gets noticed,
/// and still wakes a dark surface, within this many looks.
const RESTING_FACTOR: u32 = 5;

/// How much less often again when there is nothing to keep up with either.
///
/// Nobody is watching the surface *and* no pad holds a session an agent keeps,
/// so the only reason to look is to notice a terminal somebody opened. That
/// can wait a minute: a question from a session reaches the Push through its
/// hook the moment it is asked, and anything that asks PushOS directly — a
/// window, the command line — is answered by looking there and then.
const IDLE_FACTOR: u32 = 20;

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
                view: None,
                waiting: None,
            },
            watching,
        )
    }

    /// Says on the pad which sessions have work waiting for a free turn.
    fn mark_waiting(&self, open: &mut [Attached]) {
        let Some(waiting) = &self.waiting else {
            return;
        };
        for session in open {
            // Only over a session that is doing nothing: one that is working,
            // or asking the operator something, has more to say than this.
            if session.activity == Activity::Ready && waiting(&session.id) {
                session.activity = Activity::Queued;
            }
        }
    }

    /// Shows which sessions have work waiting for a turn.
    #[must_use]
    pub(crate) fn showing_waiting_work(
        mut self,
        waiting: impl Fn(&AttachedId) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.waiting = Some(Box::new(waiting));
        self
    }

    /// Asks more or less often than the default.
    #[must_use]
    pub(crate) const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Looks less often while nobody could be looking at the surface.
    #[must_use]
    pub(crate) fn minding(mut self, view: watch::Receiver<crate::actors::SurfaceView>) -> Self {
        self.view = Some(view);
        self
    }

    /// Whether anyone could be reading what a look would find.
    fn watched(&self) -> bool {
        self.view
            .as_ref()
            .is_none_or(|view| is_watched(&view.borrow()))
    }

    /// Asks until shutdown, telling whoever is listening what changed.
    pub(crate) async fn run(mut self, shutdown: Shutdown, mut changed: impl FnMut()) {
        let mut refused = false;

        loop {
            match self.sessions.discover().await {
                Ok(mut open) => {
                    refused = false;
                    self.mark_waiting(&mut open);
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

            let watched = self.watched();
            let following = self.found.borrow().iter().any(Attached::can_be_put_away);
            let wait = match (watched, following) {
                (true, _) => self.every,
                (false, true) => self.every * RESTING_FACTOR,
                (false, false) => self.every * IDLE_FACTOR,
            };

            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
                // Someone came back. Look now rather than up to a slow interval
                // late, so the surface they just woke is not showing old news.
                () = returns(self.view.as_mut(), watched) => {}
            }
        }
    }
}

/// What the display should say about sessions PushOS did not start.
/// Whether a surface is somewhere a person could be reading it.
fn is_watched(view: &crate::actors::SurfaceView) -> bool {
    view.snapshot.surface != pushos_ui::SurfacePresence::Absent && view.rest.shows_screen()
}

/// Waits until a surface nobody was watching becomes one somebody is.
///
/// Forever when it was already watched, or when there is nothing to follow.
async fn returns(view: Option<&mut watch::Receiver<crate::actors::SurfaceView>>, watched: bool) {
    let Some(view) = view else {
        return std::future::pending().await;
    };
    if watched {
        return std::future::pending().await;
    }
    while view.changed().await.is_ok() {
        if is_watched(&view.borrow_and_update()) {
            return;
        }
    }
    std::future::pending::<()>().await;
}

pub(crate) fn lines_for(sessions: &[Attached], selected: Option<&AttachedId>) -> Vec<SessionLine> {
    sessions
        .iter()
        .map(|session| {
            let mut line = SessionLine::new(
                session.label().to_owned(),
                session.activity.describe(),
                tone_for(session.activity),
            )
            .with_detail(session.detail().to_owned());

            if selected.is_some_and(|id| *id == session.id) {
                line = line.selected();
            }
            line
        })
        .collect()
}

/// How a session's state should read on the display.
///
/// Attention is reserved for the one state that genuinely wants a person. A
/// surface crying out about eight windows at once would be worse than a quiet
/// one, and the operator would stop believing it.
const fn tone_for(activity: Activity) -> Tone {
    match activity {
        Activity::NeedsDecision => Tone::Attention,
        Activity::Working => Tone::Active,
        Activity::Drafting | Activity::Queued | Activity::Ready => Tone::Normal,
        Activity::Quiet => Tone::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(device: &str, title: &str, busy: bool) -> Attached {
        let activity = if busy {
            Activity::Working
        } else {
            Activity::Ready
        };
        Attached::new(device, title, activity, "Terminal")
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
    fn only_the_one_that_wants_a_person_asks_for_attention() {
        // A surface crying out about eight windows at once would be worse than
        // a quiet one, and the operator would stop believing it.
        let mut asking = session("/dev/ttys005", "Stuck", false);
        asking.activity = Activity::NeedsDecision;

        let sessions = [
            session("/dev/ttys003", "Sprint", true),
            session("/dev/ttys004", "Review", false),
            asking,
        ];
        let lines = lines_for(&sessions, None);

        assert_eq!(lines[0].tone, Tone::Active, "working");
        assert_eq!(lines[1].tone, Tone::Normal, "ready");
        assert_eq!(lines[2].tone, Tone::Attention, "asking");
    }

    fn looks(fake: &pushos_testkit::FakeAttached) -> usize {
        fake.calls()
            .iter()
            .filter(|call| matches!(call, pushos_testkit::SessionCall::Discovered))
            .count()
    }

    fn surface(rest: pushos_domain::rest::Rest) -> crate::actors::SurfaceView {
        let mut view = crate::actors::SurfaceView::waiting();
        view.snapshot = Arc::new(pushos_ui::UiSnapshot {
            surface: pushos_ui::SurfacePresence::Hardware,
            ..pushos_ui::UiSnapshot::disconnected()
        });
        view.rest = rest;
        view
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_to_watch_and_nothing_to_keep_up_with_means_looking_seldom() {
        // A Mac with the Push unplugged and no agent on a pad: every look is
        // an `osascript` and, when a session has changed, a whole Claude Code
        // started to ask what it is doing.
        let fake = pushos_testkit::FakeAttached::with_sessions([("/dev/ttys003", "a terminal")]);
        let (_view, following) = watch::channel(crate::actors::SurfaceView::waiting());
        let (task, _found) = AttachedTask::new(Arc::new(fake.clone()));
        let task = task.every(Duration::from_secs(3)).minding(following);

        let shutdown = Shutdown::new();
        let running = shutdown.clone();
        tokio::spawn(async move { task.run(running, || {}).await });

        // A minute with the Push unplugged and nothing on a pad.
        tokio::time::sleep(Duration::from_secs(60)).await;
        let seldom = looks(&fake);
        shutdown.stop().await;

        assert!(
            seldom <= 2,
            "a minute with nothing to follow is a look or two, not four: {seldom}"
        );
    }

    #[tokio::test]
    async fn a_pad_whose_work_is_waiting_says_so_rather_than_looking_idle() {
        // Otherwise an operator presses it again, and again, because a pad
        // that did what it was told looks exactly like one that did nothing.
        let fake = pushos_testkit::FakeAttached::holding([
            Attached::new("thread:one", "one", Activity::Ready, "Codex")
                .dispatched(pushos_domain::ports::Keeper::CodexThreads),
            Attached::new("thread:two", "two", Activity::Working, "Codex")
                .dispatched(pushos_domain::ports::Keeper::CodexThreads),
        ]);
        let (task, found) = AttachedTask::new(Arc::new(fake));
        let task = task
            .every(Duration::from_millis(10))
            .showing_waiting_work(|session| session.as_str() == "thread:one");

        let shutdown = Shutdown::new();
        let running = tokio::spawn(task.run(shutdown.clone(), || {}));
        tokio::time::sleep(Duration::from_millis(60)).await;
        shutdown.stop().await;
        let _ = running.await;

        let seen = found.borrow().clone();
        assert_eq!(seen[0].activity, Activity::Queued, "its work is held");
        assert_eq!(
            seen[1].activity,
            Activity::Working,
            "one already working is left as it is"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn nobody_able_to_see_the_surface_means_looking_less_often() {
        use pushos_domain::rest::Rest;

        // A session an agent keeps for a pad: PushOS has to keep up with what
        // it is doing even while nobody is looking at the surface.
        let fake = pushos_testkit::FakeAttached::holding([Attached::new(
            "kept:55c88714",
            "builder",
            Activity::Ready,
            "Claude Code",
        )
        .dispatched(pushos_domain::ports::Keeper::ClaudeCode)]);
        let (view, following) = watch::channel(crate::actors::SurfaceView::waiting());
        let (task, _found) = AttachedTask::new(Arc::new(fake.clone()));
        let task = task.every(Duration::from_secs(3)).minding(following);

        let shutdown = Shutdown::new();
        let running = shutdown.clone();
        tokio::spawn(async move { task.run(running, || {}).await });

        // No surface attached for a minute: a look every fifteen seconds.
        tokio::time::sleep(Duration::from_secs(60)).await;
        let unwatched = looks(&fake);
        assert!(
            (4..=5).contains(&unwatched),
            "a minute unwatched is four or five looks, not twenty: {unwatched}"
        );

        // A surface appears and someone is using it: a look at once, then every
        // three seconds.
        view.send_replace(surface(Rest::Awake));
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            looks(&fake),
            unwatched + 1,
            "looked the moment someone came back"
        );

        tokio::time::sleep(Duration::from_secs(30)).await;
        let watched = looks(&fake) - unwatched - 1;
        assert!(
            (9..=10).contains(&watched),
            "every three seconds: {watched}"
        );

        // It goes dark: slow again.
        view.send_replace(surface(Rest::Asleep));
        let before = looks(&fake);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(looks(&fake) - before <= 5);

        shutdown.stop().await;
    }
}
