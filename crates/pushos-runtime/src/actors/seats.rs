//! Keeping the number of sessions actually running within what the Mac has.
//!
//! A pad holds a session; a session holds memory only while a process is
//! running for it. Measured on an M4, a Claude Code session's footprint is
//! around 440 MB and over 600 MB at its peak, so sixty-four running at once is
//! several times what the machine has, and twenty all working is already more
//! than it can spare. Footprint rather than resident size: most of a session
//! that is sitting there ends up compressed, and `ps` shows less than half of
//! what it really costs.
//!
//! So the number running is kept to a limit, and the ones chosen to be put
//! away are those that have been sitting idle longest. Nothing working is ever
//! put away, and nothing waiting on the operator: those are the two states
//! where stopping the process would lose something. What is put away keeps its
//! conversation and costs nothing, and the next instruction or window starts
//! it again where it left off.
//!
//! When the Mac itself says it is short of memory, the limit is tightened
//! rather than waiting for it to start swapping.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_config::model::SeatLimits;
use pushos_domain::attached::{Activity, Attached};
use pushos_domain::ids::AttachedId;
use pushos_domain::ports::Pressure;

/// How much lower the limit goes while the Mac says memory is short.
///
/// Halved on a warning, and down to a handful when it is critical: at that
/// point the machine is about to swap, and everything else on it is suffering
/// for sessions that are only sitting there.
const UNDER_WARNING: usize = 2;

/// The most left running while memory is critical.
const WHILE_CRITICAL: usize = 4;

/// When each session was last seen doing something.
///
/// A session nobody has used is the one to put away first, so what is kept is
/// when each stopped being busy rather than when it started.
#[derive(Debug, Default)]
pub(crate) struct Idleness {
    since: HashMap<AttachedId, Instant>,
}

impl Idleness {
    /// Notes what every session is doing now, as of `now`.
    pub(crate) fn seen(&mut self, sessions: &[Attached], now: Instant) {
        self.since
            .retain(|id, _| sessions.iter().any(|session| &session.id == id));
        for session in sessions {
            if session.activity == Activity::Ready {
                self.since.entry(session.id.clone()).or_insert(now);
            } else {
                // Busy, waiting, or put away: none of those is sitting idle.
                self.since.remove(&session.id);
            }
        }
    }

    /// How long a session has been sitting idle.
    fn how_long(&self, session: &AttachedId, now: Instant) -> Duration {
        self.since.get(session).map_or(Duration::ZERO, |since| {
            now.saturating_duration_since(*since)
        })
    }
}

/// Which sessions to put away, longest idle first.
///
/// Only sessions that can be put away and are running are ever chosen: a
/// terminal somebody is looking at is theirs, and one already put away costs
/// nothing.
pub(crate) fn to_put_away(
    sessions: &[Attached],
    idle: &Idleness,
    limits: SeatLimits,
    pressure: Pressure,
    now: Instant,
) -> Vec<AttachedId> {
    let mut resting: Vec<(&Attached, Duration)> = sessions
        .iter()
        .filter(|session| session.can_be_put_away() && session.activity == Activity::Ready)
        .map(|session| (session, idle.how_long(&session.id, now)))
        .collect();
    // Longest idle first, and by name where two are the same, so the
    // choice does not wander between looks.
    resting.sort_by(|(one, first), (other, second)| {
        second
            .cmp(first)
            .then_with(|| one.id.as_str().cmp(other.id.as_str()))
    });

    let mut going: Vec<AttachedId> = Vec::new();

    // Anything that has sat idle longer than the operator wanted, whatever
    // the limit says.
    if let Some(after) = limits.put_away_after {
        for (session, idle_for) in &resting {
            if *idle_for >= after {
                going.push(session.id.clone());
            }
        }
    }

    // And then enough of the rest to come back within the limit.
    if let Some(most) = limit_now(limits.most_live, pressure) {
        let live = sessions
            .iter()
            .filter(|session| session.can_be_put_away() && session.activity != Activity::Quiet)
            .count();
        let over = live.saturating_sub(going.len()).saturating_sub(most);
        for (session, _) in resting.iter().take(going.len() + over).skip(going.len()) {
            going.push(session.id.clone());
        }
    }

    going
}

/// The limit as it stands, given what the Mac says about its memory.
fn limit_now(asked: Option<usize>, pressure: Pressure) -> Option<usize> {
    match pressure {
        Pressure::Normal => asked,
        Pressure::Warning => {
            Some(asked.map_or(WHILE_CRITICAL * 2, |most| (most / UNDER_WARNING).max(1)))
        }
        Pressure::Critical => Some(asked.map_or(WHILE_CRITICAL, |most| most.min(WHILE_CRITICAL))),
    }
}

/// Keeps the number of sessions running within the limit, over and over.
pub(crate) struct SeatsTask {
    sessions: Arc<dyn pushos_domain::ports::AttachedSessions>,
    pressure: Arc<dyn pushos_domain::ports::MemoryPressure>,
    watching: tokio::sync::watch::Receiver<Vec<Attached>>,
    /// Read again at every look, so an operator who changes the limit sees it
    /// take effect with the next one rather than at the next restart.
    limits: Box<dyn Fn() -> SeatLimits + Send + Sync>,
    every: Duration,
}

/// How often the number running is looked at.
///
/// Putting one away and starting it again costs a couple of seconds, so this
/// is not a decision to take every three seconds; a minute of a session
/// sitting idle past the limit costs only memory.
pub(crate) const HOW_OFTEN: Duration = Duration::from_secs(30);

impl std::fmt::Debug for SeatsTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeatsTask")
            .field("every", &self.every)
            .finish_non_exhaustive()
    }
}

impl SeatsTask {
    /// Watches the sessions a provider can see, and puts away what must go.
    pub(crate) fn new(
        sessions: Arc<dyn pushos_domain::ports::AttachedSessions>,
        pressure: Arc<dyn pushos_domain::ports::MemoryPressure>,
        watching: tokio::sync::watch::Receiver<Vec<Attached>>,
        limits: impl Fn() -> SeatLimits + Send + Sync + 'static,
    ) -> Self {
        Self {
            sessions,
            pressure,
            watching,
            limits: Box::new(limits),
            every: HOW_OFTEN,
        }
    }

    /// Looks more or less often than the default.
    #[cfg(test)]
    const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Runs until PushOS stops.
    pub(crate) async fn run(mut self, shutdown: crate::shutdown::Shutdown) {
        let mut idle = Idleness::default();
        let mut every = tokio::time::interval(self.every);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = every.tick() => {}
            }

            let sessions = self.watching.borrow_and_update().clone();
            let now = Instant::now();
            idle.seen(&sessions, now);
            if sessions.iter().all(|session| !session.can_be_put_away()) {
                continue;
            }

            let pressure = self.pressure.now().await;
            for id in to_put_away(&sessions, &idle, (self.limits)(), pressure, now) {
                match self.sessions.put_away(&id).await {
                    Ok(true) => tracing::info!(
                        session = %id,
                        ?pressure,
                        "put a session away to leave the Mac its memory"
                    ),
                    Ok(false) => {}
                    Err(error) => {
                        tracing::debug!(%error, session = %id, "could not put a session away");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, activity: Activity) -> Attached {
        Attached::new(format!("kept:{id}"), id, activity, "Claude Code").dispatched()
    }

    fn in_a_terminal(id: &str, activity: Activity) -> Attached {
        Attached::new(format!("/dev/{id}"), id, activity, "Terminal")
    }

    fn limits(most_live: Option<usize>) -> SeatLimits {
        SeatLimits {
            most_live,
            put_away_after: None,
        }
    }

    #[test]
    fn nothing_goes_while_the_limit_is_not_reached() {
        let sessions = [
            session("one", Activity::Ready),
            session("two", Activity::Working),
        ];
        let going = to_put_away(
            &sessions,
            &Idleness::default(),
            limits(Some(4)),
            Pressure::Normal,
            Instant::now(),
        );
        assert!(going.is_empty());
    }

    #[test]
    fn over_the_limit_the_one_idle_longest_goes_first() {
        let now = Instant::now();
        let sessions = [
            session("recent", Activity::Ready),
            session("oldest", Activity::Ready),
            session("working", Activity::Working),
        ];
        let mut idle = Idleness::default();
        idle.since.insert(
            sessions[0].id.clone(),
            now.checked_sub(Duration::from_secs(60)).expect("a while"),
        );
        idle.since.insert(
            sessions[1].id.clone(),
            now.checked_sub(Duration::from_secs(600)).expect("longer"),
        );

        let going = to_put_away(&sessions, &idle, limits(Some(2)), Pressure::Normal, now);

        assert_eq!(
            going,
            [sessions[1].id.clone()],
            "one over, and it is the oldest"
        );
    }

    #[test]
    fn nothing_working_or_waiting_on_the_operator_is_ever_put_away() {
        let sessions = [
            session("working", Activity::Working),
            session("asking", Activity::NeedsDecision),
        ];
        let going = to_put_away(
            &sessions,
            &Idleness::default(),
            limits(Some(1)),
            Pressure::Critical,
            Instant::now(),
        );
        assert!(
            going.is_empty(),
            "even over the limit, and even when memory is short"
        );
    }

    #[test]
    fn a_session_in_somebody_s_terminal_is_not_pushos_s_to_stop() {
        let sessions = [
            in_a_terminal("ttys001", Activity::Ready),
            in_a_terminal("ttys002", Activity::Ready),
        ];
        let going = to_put_away(
            &sessions,
            &Idleness::default(),
            limits(Some(0)),
            Pressure::Normal,
            Instant::now(),
        );
        assert!(going.is_empty());
    }

    #[test]
    fn one_already_put_away_is_not_counted_and_not_put_away_again() {
        let sessions = [
            session("away", Activity::Quiet),
            session("live", Activity::Ready),
        ];
        let going = to_put_away(
            &sessions,
            &Idleness::default(),
            limits(Some(1)),
            Pressure::Normal,
            Instant::now(),
        );
        assert!(going.is_empty(), "one is running, and one is allowed");
    }

    #[test]
    fn sitting_idle_for_longer_than_asked_is_enough_on_its_own() {
        let now = Instant::now();
        let sessions = [session("forgotten", Activity::Ready)];
        let mut idle = Idleness::default();
        idle.since.insert(
            sessions[0].id.clone(),
            now.checked_sub(Duration::from_secs(3_600))
                .expect("an hour"),
        );

        let going = to_put_away(
            &sessions,
            &idle,
            SeatLimits {
                most_live: Some(20),
                put_away_after: Some(Duration::from_mins(20)),
            },
            Pressure::Normal,
            now,
        );

        assert_eq!(
            going,
            [sessions[0].id.clone()],
            "nobody has used it all day"
        );
    }

    #[test]
    fn a_mac_short_of_memory_keeps_fewer_running() {
        assert_eq!(limit_now(Some(20), Pressure::Normal), Some(20));
        assert_eq!(limit_now(Some(20), Pressure::Warning), Some(10));
        assert_eq!(limit_now(Some(20), Pressure::Critical), Some(4));
        assert_eq!(
            limit_now(Some(2), Pressure::Warning),
            Some(1),
            "never rounded down to none at all"
        );
        assert_eq!(
            limit_now(None, Pressure::Normal),
            None,
            "an operator who asked for no limit has none"
        );
        assert_eq!(
            limit_now(None, Pressure::Critical),
            Some(WHILE_CRITICAL),
            "until the Mac itself says it cannot"
        );
    }

    #[test]
    fn what_counts_as_idle_is_forgotten_when_a_session_goes() {
        let now = Instant::now();
        let mut idle = Idleness::default();
        let sessions = [session("one", Activity::Ready)];
        idle.seen(&sessions, now);
        assert_eq!(idle.since.len(), 1);

        idle.seen(&[], now);
        assert!(
            idle.since.is_empty(),
            "it cannot grow with sessions that went"
        );
    }

    #[tokio::test]
    async fn the_task_puts_away_what_the_limit_says_and_leaves_the_rest() {
        use pushos_testkit::FakeAttached;

        let sessions = vec![
            session("one", Activity::Ready),
            session("two", Activity::Ready),
            session("busy", Activity::Working),
        ];
        let fake = Arc::new(FakeAttached::holding(sessions.clone()));
        let (found, watching) = tokio::sync::watch::channel(sessions);
        let shutdown = crate::shutdown::Shutdown::new();

        let task = SeatsTask::new(
            Arc::clone(&fake) as Arc<_>,
            Arc::new(pushos_domain::ports::RoomToSpare),
            watching,
            || limits(Some(1)),
        )
        .every(Duration::from_millis(10));
        let running = tokio::spawn(task.run(shutdown.clone()));

        // Long enough for a few looks; one is enough to decide.
        tokio::time::sleep(Duration::from_millis(80)).await;
        shutdown.stop().await;
        running.await.expect("the task ends when PushOS does");
        drop(found);

        let put_away: Vec<String> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                pushos_testkit::SessionCall::PutAway(id) => Some(id),
                _ => None,
            })
            .collect();
        assert!(
            put_away.contains(&"kept:one".to_owned()) || put_away.contains(&"kept:two".to_owned()),
            "one of the two idle sessions went: {put_away:?}"
        );
        assert!(
            !put_away.contains(&"kept:busy".to_owned()),
            "the one working stayed: {put_away:?}"
        );
    }

    #[test]
    fn a_session_that_starts_working_is_no_longer_idle() {
        let now = Instant::now();
        let mut idle = Idleness::default();
        let ready = [session("one", Activity::Ready)];
        idle.seen(&ready, now);

        let working = [session("one", Activity::Working)];
        idle.seen(&working, now);
        assert!(idle.since.is_empty());

        // And when it finishes, it starts sitting idle from then, not from
        // before it was asked to do anything.
        let later = now + Duration::from_secs(300);
        idle.seen(&ready, later);
        assert_eq!(idle.how_long(&ready[0].id, later), Duration::ZERO);
    }
}
