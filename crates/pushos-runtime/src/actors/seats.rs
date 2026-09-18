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

/// The most sessions with a process each left running while memory is
/// critical.
const WHILE_CRITICAL: usize = 4;

/// How much lower the limit for threads in a shared server goes while memory
/// is critical.
///
/// A quarter rather than a fixed handful: sixty-four threads are about a
/// gigabyte between them, so cutting them to four would give up the whole
/// surface to recover what one session with a process of its own costs.
const THREADS_UNDER_CRITICAL: usize = 4;

/// The most threads left loaded while memory is critical, when the operator
/// set no limit of their own.
///
/// A quarter of a full surface, for the same reason: it gives back most of
/// what the threads cost and still leaves a surface worth looking at.
const THREADS_WHILE_CRITICAL: usize = 16;

/// How long a turn is held for work that has been sent but is not yet being
/// done.
///
/// A session takes a moment to start, and until it does it still reads as
/// idle; without this the next look would let the same turn out again. Past
/// this, whatever was sent has gone nowhere, and holding its turn for ever
/// would be a fleet that quietly stops taking work.
const SHOWS_UP_WITHIN: Duration = Duration::from_secs(30);

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

    // And then enough of the rest to come back within the limits. The two
    // kinds are counted apart because what they cost is not comparable: a
    // session with a process of its own is a few hundred megabytes, and a
    // thread in a shared server is a few tens.
    for costly in [true, false] {
        let asked = if costly {
            limits.most_live
        } else {
            limits.most_threads
        };
        let Some(most) = limit_now(asked, pressure, costly) else {
            continue;
        };

        let of_this_kind =
            |session: &Attached| session.can_be_put_away() && session.costs_a_process() == costly;
        let live = sessions
            .iter()
            .filter(|session| of_this_kind(session) && session.activity != Activity::Quiet)
            .count();
        let chosen: Vec<AttachedId> = going.clone();
        let waiting: Vec<&Attached> = resting
            .iter()
            .map(|(session, _)| *session)
            .filter(|session| of_this_kind(session))
            .collect();

        let already = waiting
            .iter()
            .filter(|session| chosen.contains(&session.id))
            .count();
        let over = live.saturating_sub(already).saturating_sub(most);
        going.extend(
            waiting
                .iter()
                .filter(|session| !chosen.contains(&session.id))
                .take(over)
                .map(|session| session.id.clone()),
        );
    }

    going
}

/// The limit as it stands, given what the Mac says about its memory.
///
/// What a session costs decides how hard it is cut: one with a process of its
/// own is a few hundred megabytes, so under real pressure only a handful may
/// run. Threads in a shared server are a few tens of megabytes each, and
/// cutting those to the same handful would empty the surface to recover
/// almost nothing.
fn limit_now(asked: Option<usize>, pressure: Pressure, costly: bool) -> Option<usize> {
    match pressure {
        Pressure::Normal => asked,
        Pressure::Warning => {
            let unasked = if costly {
                WHILE_CRITICAL * 2
            } else {
                THREADS_WHILE_CRITICAL * 2
            };
            Some(asked.map_or(unasked, |most| (most / UNDER_WARNING).max(1)))
        }
        // Never above what a warning already allows: the Mac saying it is
        // about to swap must not be the moment more sessions are let run.
        Pressure::Critical if costly => Some(asked.map_or(WHILE_CRITICAL, |most| {
            (most / UNDER_WARNING).clamp(1, WHILE_CRITICAL)
        })),
        Pressure::Critical => Some(asked.map_or(THREADS_WHILE_CRITICAL, |most| {
            (most / THREADS_UNDER_CRITICAL).max(1)
        })),
    }
}

/// Drops the sessions whose work has begun, or plainly never will.
///
/// What is left is the sessions that were given work and are not yet seen
/// doing it, which must not be put away underneath it. A session woken from
/// being put away reads as put away for the seconds its process takes to
/// start, and one absent from a description has not been described at all;
/// neither is a reason to take its work away, so only a session actually seen
/// working is dropped from here.
fn still_starting(
    just_started: &mut Vec<(AttachedId, Instant)>,
    sessions: &[Attached],
    now: Instant,
) {
    just_started.retain(|(id, sent)| {
        now.duration_since(*sent) < SHOWS_UP_WITHIN
            && sessions
                .iter()
                .any(|session| &session.id == id && session.activity == Activity::Ready)
    });
}

/// Keeps the number of sessions running within the limit, over and over.
pub(crate) struct SeatsTask {
    sessions: Arc<dyn pushos_domain::ports::AttachedSessions>,
    /// Whose queue of work is let out as sessions finish.
    provider: Option<Arc<pushos_actions::providers::session::SessionProvider>>,
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

/// How often it looks while work is waiting for a turn.
///
/// A session that finishes frees a turn, and whatever is waiting should go
/// then rather than up to half a minute later; an operator watching a pad
/// would call that half a minute broken.
const WHILE_WORK_WAITS: Duration = Duration::from_secs(2);

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
            provider: None,
            pressure,
            watching,
            limits: Box::new(limits),
            every: HOW_OFTEN,
        }
    }

    /// Lets work that is waiting for a turn go as sessions finish.
    #[must_use]
    pub(crate) fn letting_work_through(
        mut self,
        provider: Arc<pushos_actions::providers::session::SessionProvider>,
    ) -> Self {
        // Said now rather than at the first look: until it is, the fleet has
        // no limit, and the first thing an operator does is press several
        // pads at once.
        provider.allow_working((self.limits)().working_at_once);
        self.provider = Some(provider);
        self
    }

    /// Looks more or less often than the default.
    #[cfg(test)]
    const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Starts what has been waiting, as far as the limit allows.
    ///
    /// Counted from what the sessions are doing rather than from what PushOS
    /// started: a session may be working because the operator typed in its own
    /// window, and that is still the model's attention being used.
    async fn let_work_through(&self, sessions: &[Attached]) -> Vec<AttachedId> {
        let Some(provider) = &self.provider else {
            return Vec::new();
        };
        let limits = (self.limits)();
        provider.allow_working(limits.working_at_once);
        if provider.waiting_turns() == 0 {
            return Vec::new();
        }

        let Some(most) = limits.working_at_once else {
            return provider.start_waiting_work(usize::MAX).await;
        };
        let working: Vec<AttachedId> = sessions
            .iter()
            .filter(|session| session.can_be_put_away() && session.activity == Activity::Working)
            .map(|session| session.id.clone())
            .collect();
        // Asked of the provider rather than counted here, because a pad the
        // operator pressed a second ago took a turn that this task never saw
        // and no description shows yet. One account of what is under way, kept
        // in one place.
        let starting = provider.work_starting(&working);
        let free = most.saturating_sub(working.len()).saturating_sub(starting);
        if free == 0 {
            return Vec::new();
        }
        let started = provider.start_waiting_work(free).await;
        if !started.is_empty() {
            tracing::debug!(
                started = started.len(),
                working = working.len(),
                starting,
                "let work through that was waiting a turn"
            );
        }
        started
    }

    /// Runs until PushOS stops.
    pub(crate) async fn run(mut self, shutdown: crate::shutdown::Shutdown) {
        let mut idle = Idleness::default();
        // Sessions given work that has not shown up as work yet, and when they
        // were given it. They still read as idle until the agent gets going,
        // and both decisions below need to know better: the turn they took is
        // not free again, and a session about to work is not one to put away.
        let mut just_started: Vec<(AttachedId, Instant)> = Vec::new();
        let mut last_look: Option<Instant> = None;

        loop {
            let waiting = self
                .provider
                .as_ref()
                .is_some_and(|provider| provider.waiting_turns() > 0);
            let soon = if waiting {
                WHILE_WORK_WAITS.min(self.every)
            } else {
                self.every
            };

            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(soon) => {}
            }

            let sessions = self.watching.borrow_and_update().clone();
            let now = Instant::now();
            still_starting(&mut just_started, &sessions, now);
            for id in self.let_work_through(&sessions).await {
                if !just_started.iter().any(|(started, _)| *started == id) {
                    just_started.push((id, now));
                }
            }

            // At every look, quick or not: it is only bookkeeping over what
            // is already in hand, and a session that worked and finished
            // between two slow looks would otherwise read as having sat idle
            // through the whole of it, and be put away first for it.
            idle.seen(&sessions, now);

            // The rest of the look is not for every tick. Asking the Mac about
            // its memory costs a subprocess, and putting a session away is not
            // a decision to take every two seconds.
            if last_look.is_some_and(|last: Instant| last.elapsed() < self.every) {
                continue;
            }
            last_look = Some(now);
            if sessions.iter().all(|session| !session.can_be_put_away()) {
                continue;
            }

            let pressure = self.pressure.now().await;
            let going = to_put_away(&sessions, &idle, (self.limits)(), pressure, now);
            for id in going
                .into_iter()
                .filter(|id| !just_started.iter().any(|(started, _)| started == id))
            {
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
        Attached::new(format!("kept:{id}"), id, activity, "Claude Code")
            .dispatched(pushos_domain::ports::Keeper::ClaudeCode)
    }

    fn in_a_terminal(id: &str, activity: Activity) -> Attached {
        Attached::new(format!("/dev/{id}"), id, activity, "Terminal")
    }

    fn limits(most_live: Option<usize>) -> SeatLimits {
        SeatLimits {
            most_live,
            working_at_once: None,
            // The tests below are about sessions with a process each; a
            // separate test covers threads in a shared server.
            most_threads: None,
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
                working_at_once: None,
                most_threads: None,
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
    fn threads_in_a_shared_server_are_counted_apart_from_sessions_with_a_process() {
        // Sixty-four threads is about a gigabyte; twenty sessions with a
        // process each is nine. One limit for both would put away threads
        // that cost almost nothing.
        let threads: Vec<Attached> = (0..30)
            .map(|index| {
                Attached::new(
                    format!("thread:{index}"),
                    index.to_string(),
                    Activity::Ready,
                    "Codex",
                )
                .dispatched(pushos_domain::ports::Keeper::CodexThreads)
            })
            .collect();

        let limits = SeatLimits {
            most_live: Some(2),
            working_at_once: None,
            most_threads: Some(64),
            put_away_after: None,
        };
        let going = to_put_away(
            &threads,
            &Idleness::default(),
            limits,
            Pressure::Normal,
            Instant::now(),
        );
        assert!(
            going.is_empty(),
            "thirty threads are well inside what a server holds: {going:?}"
        );

        // And the limit that is theirs does apply.
        let tighter = SeatLimits {
            most_threads: Some(8),
            ..limits
        };
        let going = to_put_away(
            &threads,
            &Idleness::default(),
            tighter,
            Pressure::Normal,
            Instant::now(),
        );
        assert_eq!(going.len(), 22, "down to eight");
    }

    #[test]
    fn a_mac_short_of_memory_keeps_fewer_running() {
        assert_eq!(limit_now(Some(20), Pressure::Normal, true), Some(20));
        assert_eq!(limit_now(Some(20), Pressure::Warning, true), Some(10));
        assert_eq!(limit_now(Some(20), Pressure::Critical, true), Some(4));
        assert_eq!(
            limit_now(Some(2), Pressure::Warning, true),
            Some(1),
            "never rounded down to none at all"
        );
        assert_eq!(
            limit_now(None, Pressure::Normal, true),
            None,
            "an operator who asked for no limit has none"
        );
        assert_eq!(
            limit_now(None, Pressure::Critical, true),
            Some(WHILE_CRITICAL),
            "until the Mac itself says it cannot"
        );
    }

    #[test]
    fn threads_are_not_cut_to_the_number_a_session_with_a_process_would_be() {
        // Sixty-four threads are about a gigabyte between them. Cutting them
        // to four would empty the surface to recover what one Claude Code
        // session costs on its own.
        assert_eq!(limit_now(Some(64), Pressure::Critical, false), Some(16));
        assert_eq!(limit_now(Some(64), Pressure::Warning, false), Some(32));
        assert_eq!(
            limit_now(Some(2), Pressure::Critical, false),
            Some(1),
            "never rounded down to none at all"
        );
        assert_eq!(
            limit_now(None, Pressure::Critical, false),
            Some(THREADS_WHILE_CRITICAL),
            "and an operator who set no limit is held to a surface, not a handful"
        );
        assert_eq!(
            limit_now(None, Pressure::Warning, false),
            Some(THREADS_WHILE_CRITICAL * 2)
        );
        assert!(
            limit_now(None, Pressure::Critical, false) > limit_now(None, Pressure::Critical, true),
            "a thread costs a fraction of a session, so more of them fit"
        );
    }

    #[test]
    fn a_turn_is_held_until_the_work_it_was_given_is_under_way() {
        let now = Instant::now();
        let one = session("one", Activity::Ready);
        let mut held = vec![(one.id.clone(), now)];

        still_starting(&mut held, std::slice::from_ref(&one), now);
        assert_eq!(held.len(), 1, "it still reads as idle: the turn is taken");

        // Another session being described says nothing about this one, and
        // freeing the turn here would let the same turn out twice.
        still_starting(&mut held, &[one, session("two", Activity::Working)], now);
        assert_eq!(held.len(), 1);

        still_starting(&mut held, &[session("one", Activity::Working)], now);
        assert!(held.is_empty(), "it is counted among the working now");
    }

    #[test]
    fn a_turn_is_not_held_for_ever_by_work_that_never_started() {
        let now = Instant::now();
        let one = session("one", Activity::Ready);
        let mut held = vec![(
            one.id.clone(),
            now.checked_sub(SHOWS_UP_WITHIN).expect("a while"),
        )];

        still_starting(&mut held, &[one], now);

        assert!(
            held.is_empty(),
            "a fleet that quietly stopped taking work would be far worse"
        );
    }

    fn typing(target: &str, text: &str) -> pushos_domain::action::ActionContext {
        use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
        let mut params = Params::new();
        params.set("target", ParamValue::Text(target.into()));
        params.set("text", ParamValue::Text(text.into()));
        pushos_domain::action::ActionContext::new(
            ActionDefinition::new(
                "session.send".parse::<ActionSelector>().expect("valid"),
                params,
            ),
            pushos_domain::ids::CorrelationId::generate(),
            pushos_domain::context::SurfaceContext::empty(),
        )
    }

    #[tokio::test]
    async fn a_session_woken_from_being_put_away_keeps_the_turn_it_was_given() {
        use pushos_domain::ports::ActionProvider as _;
        use pushos_testkit::{FakeAttached, SessionCall};

        let busy = session("busy", Activity::Working);
        let away = session("away", Activity::Quiet);
        let free = session("free", Activity::Ready);
        let fake = Arc::new(FakeAttached::holding([
            busy.clone(),
            away.clone(),
            free.clone(),
        ]));
        let provider = Arc::new(pushos_actions::providers::session::SessionProvider::new(
            Arc::clone(&fake) as Arc<_>,
        ));
        provider.allow_working(Some(1));

        // Both pads pressed while the one turn is taken: both wait.
        for id in ["kept:away", "kept:free"] {
            provider
                .execute(typing(&format!("id:{id}"), "review the invoices"))
                .await
                .expect("accepted");
        }
        assert_eq!(provider.waiting_turns(), 2);

        // The busy one finishes.
        let described = [session("busy", Activity::Ready), away.clone(), free.clone()];
        let (_found, watching) = tokio::sync::watch::channel(described.to_vec());
        let task = SeatsTask::new(
            Arc::clone(&fake) as Arc<_>,
            Arc::new(pushos_domain::ports::RoomToSpare),
            watching,
            || SeatLimits {
                most_live: None,
                working_at_once: Some(1),
                most_threads: None,
                put_away_after: None,
            },
        )
        .letting_work_through(Arc::clone(&provider));

        let now = Instant::now();
        let mut just_started: Vec<(AttachedId, Instant)> = Vec::new();
        just_started.extend(
            task.let_work_through(&described)
                .await
                .into_iter()
                .map(|id| (id, now)),
        );
        assert_eq!(just_started.len(), 1, "one turn, one piece of work");

        // Two seconds later. The session that was woken is still starting its
        // process, so it is still described as put away.
        let soon = now + Duration::from_secs(2);
        still_starting(&mut just_started, &described, soon);
        just_started.extend(
            task.let_work_through(&described)
                .await
                .into_iter()
                .map(|id| (id, soon)),
        );

        let went: Vec<String> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                SessionCall::Sent(id, _) => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(
            went.len(),
            1,
            "the operator allowed one at once, and the woken session has not \
             started yet: {went:?}"
        );
    }

    #[tokio::test]
    async fn work_sent_straight_from_a_pad_is_counted_before_more_is_let_through() {
        use pushos_domain::ports::ActionProvider as _;
        use pushos_testkit::{FakeAttached, SessionCall};

        let described = [
            session("one", Activity::Ready),
            session("two", Activity::Ready),
            session("three", Activity::Ready),
        ];
        let fake = Arc::new(FakeAttached::holding(described.to_vec()));
        let provider = Arc::new(pushos_actions::providers::session::SessionProvider::new(
            Arc::clone(&fake) as Arc<_>,
        ));
        provider.allow_working(Some(2));

        // Three pads pressed in the same second. The provider itself holds the
        // third back: two have been sent and nothing is described as working
        // yet, so the fleet is full.
        for id in ["kept:one", "kept:two", "kept:three"] {
            provider
                .execute(typing(&format!("id:{id}"), "review the invoices"))
                .await
                .expect("accepted");
        }
        assert_eq!(provider.waiting_turns(), 1, "the third waits its turn");

        let (_found, watching) = tokio::sync::watch::channel(described.to_vec());
        let task = SeatsTask::new(
            Arc::clone(&fake) as Arc<_>,
            Arc::new(pushos_domain::ports::RoomToSpare),
            watching,
            || SeatLimits {
                most_live: None,
                working_at_once: Some(2),
                most_threads: None,
                put_away_after: None,
            },
        )
        .letting_work_through(Arc::clone(&provider));

        // The next look, two seconds later. The two that were sent still read
        // as idle, exactly as they did when the third was held back.
        let started = task.let_work_through(&described).await;

        let went: Vec<String> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                SessionCall::Sent(id, _) => Some(id),
                _ => None,
            })
            .collect();
        assert!(
            started.is_empty() && went.len() == 2,
            "two at once is what the operator allowed: {went:?}"
        );
    }

    #[test]
    fn a_tighter_pressure_never_raises_the_limit() {
        for most in 1..=64 {
            let normal = limit_now(Some(most), Pressure::Normal, true);
            let warning = limit_now(Some(most), Pressure::Warning, true);
            let critical = limit_now(Some(most), Pressure::Critical, true);
            assert!(
                critical <= warning && warning <= normal,
                "most_live {most}: normal {normal:?}, warning {warning:?}, \
                 critical {critical:?}"
            );
        }
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
