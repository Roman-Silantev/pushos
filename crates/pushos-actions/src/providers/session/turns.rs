//! How many sessions may be working at once, and what waits its turn.
//!
//! Memory is not what stops sixty-four pads working at once; the model does.
//! Both Claude Code and Codex answer more than a handful of streams at a time
//! with refusals, and a refusal costs the whole turn rather than delaying it,
//! so a surface that starts everything at once finishes less work than one
//! that paces itself.
//!
//! So a pad that is given work while the fleet is busy does not fail and does
//! not start anyway: its work waits here, and goes the moment a session
//! finishes. That is the difference between sixty-four pads that collapse and
//! sixty-four pads that get through their work.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pushos_domain::ids::AttachedId;

/// The most work that may wait at once.
///
/// One per pad and some to spare. Past this, a pad says so rather than adding
/// to a queue nobody could read.
const MOST_WAITING: usize = 128;

/// How many times work is offered again after the agent refused it.
///
/// A refusal is usually the model saying there is too much at once, which
/// passes. After this many it is something else, and trying for ever would be
/// a pad that never reports a problem.
const TRIES: u8 = 4;

/// How long work counts as under way before it has been seen being done.
///
/// Pressing several pads at once is faster than any description of what the
/// sessions are doing, so without this every press in the same few seconds
/// would see an idle fleet and go straight past the limit. Past this,
/// whatever was sent has gone nowhere, and counting it for ever would be a
/// fleet that quietly stops taking work.
const STARTS_WITHIN: Duration = Duration::from_secs(30);

/// How long to leave it before offering refused work again.
///
/// Doubling each time: a model that is rate limiting says so for a while, and
/// asking again immediately would spend the next refusal for nothing.
const FIRST_WAIT: Duration = Duration::from_secs(5);

/// Work that has not started yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Waiting {
    /// Which session it is for.
    pub(super) session: AttachedId,
    /// What to tell it.
    pub(super) text: String,
    /// How many times the agent has refused to start it.
    refused: u8,
    /// Not offered again before this, after a refusal.
    next_try: Option<Instant>,
    /// What this piece of work is written down under.
    ///
    /// A refusal arrives after the fact, and by then the operator may have
    /// replaced this work or cancelled it. This is how one is told from the
    /// other, so what comes back is only ever what is still wanted.
    token: u64,
}

impl Waiting {
    /// Whether it may be offered now.
    fn ready(&self, now: Instant) -> bool {
        self.next_try.is_none_or(|next| now >= next)
    }

    /// The same work, refused once more and left for longer.
    ///
    /// `None` once it has been refused too often to be worth offering again.
    fn refused_again(mut self, now: Instant) -> Option<Self> {
        self.refused += 1;
        if self.refused >= TRIES {
            return None;
        }
        self.next_try = Some(now + FIRST_WAIT * 2_u32.pow(u32::from(self.refused) - 1));
        Some(self)
    }
}

/// What became of work the agent refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// It waits, and goes again when it has waited long enough.
    WaitsAgain,
    /// Refused too often: something is wrong that waiting will not fix.
    GivenUpOn,
    /// The operator replaced or cancelled it while the agent was deciding.
    NoLongerWanted,
}

/// The queue, and what has been handed out of it.
#[derive(Debug, Default)]
struct Queue {
    /// Work waiting for a turn, oldest first.
    waiting: VecDeque<Waiting>,
    /// Work handed to a session and not yet finished with, by session.
    ///
    /// What makes putting a refusal back safe: work the operator has since
    /// replaced or cancelled is no longer here, so it comes back to nothing.
    given: HashMap<AttachedId, u64>,
    /// When each session was last sent work, until it is seen doing it.
    sent: HashMap<AttachedId, Instant>,
    /// What the next piece of work is written down under.
    next: u64,
}

impl Queue {
    /// A number no other piece of work has.
    fn token(&mut self) -> u64 {
        self.next = self.next.wrapping_add(1);
        self.next
    }
}

/// How many may work at once, and what is waiting for a turn.
#[derive(Debug, Default)]
pub(super) struct Turns {
    /// The most sessions that may be working at once. Nought is no limit.
    allowed: AtomicUsize,
    queue: Mutex<Queue>,
}

impl Turns {
    /// A book with no limit, until the runtime says what the limit is.
    pub(super) fn new() -> Self {
        Self {
            allowed: AtomicUsize::new(0),
            queue: Mutex::new(Queue::default()),
        }
    }

    /// Sets how many sessions may be working at once.
    pub(super) fn allow(&self, most: usize) {
        self.allowed.store(most, Ordering::Relaxed);
    }

    /// How many may work at once, if there is a limit at all.
    pub(super) fn limit(&self) -> Option<usize> {
        let allowed = self.allowed.load(Ordering::Relaxed);
        (allowed > 0).then_some(allowed)
    }

    /// Whether work given now would have to wait, with `working` under way.
    pub(super) fn busy(&self, working: usize) -> bool {
        self.limit().is_some_and(|most| working >= most)
    }

    /// Notes that a session has just been sent work.
    pub(super) fn sent(&self, session: &AttachedId, now: Instant) {
        self.queue().sent.insert(session.clone(), now);
    }

    /// How many sessions were sent work that is not yet being done.
    ///
    /// Those already among `working` are left out: they are counted there,
    /// and counting them twice would hold back a fleet that has room.
    pub(super) fn starting(&self, now: Instant, working: &[AttachedId]) -> usize {
        let mut queue = self.queue();
        queue.sent.retain(|session, sent| {
            now.duration_since(*sent) < STARTS_WITHIN && !working.contains(session)
        });
        queue.sent.len()
    }

    /// Puts work aside until a session is free, and says how much is ahead of
    /// it — nought when it is next.
    ///
    /// Replaces whatever that session was already waiting to be told: the
    /// newer instruction is the one the operator meant, exactly as pressing a
    /// pad twice means the second thing.
    pub(super) fn hold(&self, session: &AttachedId, text: &str) -> Result<usize, Full> {
        let mut queue = self.queue();
        // Whatever this session was handed a moment ago is not what the
        // operator wants any more, so a refusal of it must not bring it back.
        queue.given.remove(session);
        if let Some(at) = queue
            .waiting
            .iter()
            .position(|held| &held.session == session)
        {
            let already = &mut queue.waiting[at];
            text.clone_into(&mut already.text);
            // Different work: whatever the agent thought of the last lot is
            // no reason to give up on this sooner, or to make it wait.
            already.refused = 0;
            already.next_try = None;
            return Ok(at);
        }
        if queue.waiting.len() >= MOST_WAITING {
            return Err(Full);
        }
        let token = queue.token();
        queue.waiting.push_back(Waiting {
            session: session.clone(),
            text: text.to_owned(),
            refused: 0,
            next_try: None,
            token,
        });
        Ok(queue.waiting.len() - 1)
    }

    /// Takes the next `free` pieces of work, oldest first.
    ///
    /// Work that was refused is left alone until its wait has passed.
    pub(super) fn take(&self, free: usize, now: Instant) -> Vec<Waiting> {
        let mut queue = self.queue();
        let mut taking = Vec::new();
        let mut index = 0;
        while index < queue.waiting.len() && taking.len() < free {
            if queue.waiting[index].ready(now) {
                if let Some(held) = queue.waiting.remove(index) {
                    queue.given.insert(held.session.clone(), held.token);
                    taking.push(held);
                }
            } else {
                index += 1;
            }
        }
        taking
    }

    /// Puts work back after the agent refused it, to be offered again later.
    ///
    /// Dropped once it has been refused too often, because something is wrong
    /// that waiting will not fix, and dropped if the operator has replaced or
    /// cancelled it since: offering that again would be PushOS arguing with
    /// them.
    pub(super) fn refused(&self, held: Waiting, now: Instant) -> Refused {
        let mut queue = self.queue();
        if queue.given.get(&held.session) != Some(&held.token) {
            return Refused::NoLongerWanted;
        }
        queue.given.remove(&held.session);
        let Some(again) = held.refused_again(now) else {
            return Refused::GivenUpOn;
        };
        queue.waiting.push_front(again);
        // It was in the queue a moment ago, so this is the same queue it came
        // from; the bound still holds, and the oldest goes if it does not.
        queue.waiting.truncate(MOST_WAITING);
        Refused::WaitsAgain
    }

    /// Forgets that a session was handed work, once it has gone or been
    /// dropped.
    pub(super) fn done(&self, session: &AttachedId) {
        self.queue().given.remove(session);
    }

    /// Drops whatever a session was waiting to be told.
    ///
    /// For when it is told something else directly, or stopped: what was
    /// waiting is what the operator has just replaced or cancelled, and
    /// sending it afterwards would be PushOS arguing with them.
    pub(super) fn forget(&self, session: &AttachedId) {
        let mut queue = self.queue();
        queue.waiting.retain(|held| &held.session != session);
        queue.given.remove(session);
        queue.sent.remove(session);
    }

    /// The queue, whatever state a panic elsewhere left the lock in.
    ///
    /// What it holds is text and identifiers, which a panic cannot have made
    /// nonsense of, and a fleet that stopped taking work for the rest of the
    /// day would be far worse than the risk of reading them.
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether a session has work waiting for a turn.
    pub(super) fn holds_work_for(&self, session: &AttachedId) -> bool {
        self.queue()
            .waiting
            .iter()
            .any(|held| &held.session == session)
    }

    /// How much work is waiting.
    pub(super) fn how_much_waiting(&self) -> usize {
        self.queue().waiting.len()
    }
}

/// Nothing more can wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("too much work is already waiting for a turn")]
pub(super) struct Full;

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> AttachedId {
        AttachedId::new(format!("thread:{name}"))
    }

    #[test]
    fn with_no_limit_nothing_ever_waits() {
        let turns = Turns::new();
        assert!(!turns.busy(64), "the operator asked for no limit");
        assert_eq!(turns.limit(), None);
    }

    #[test]
    fn work_waits_only_once_the_fleet_is_full() {
        let turns = Turns::new();
        turns.allow(4);

        assert!(!turns.busy(3));
        assert!(turns.busy(4));
        assert!(turns.busy(9), "and stays waiting past it");
    }

    #[test]
    fn work_the_agent_refused_is_offered_again_later_rather_than_lost() {
        let turns = Turns::new();
        let now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");

        let held = turns.take(1, now).pop().expect("taken");
        assert_eq!(
            turns.refused(held, now),
            Refused::WaitsAgain,
            "kept for another go"
        );
        assert_eq!(turns.how_much_waiting(), 1);

        assert!(
            turns.take(4, now).is_empty(),
            "not offered again immediately: the model said no a moment ago"
        );
        let later = now + FIRST_WAIT;
        assert_eq!(
            turns.take(4, later).len(),
            1,
            "and offered once it has waited"
        );
    }

    #[test]
    fn work_refused_over_and_over_is_given_up_on() {
        let turns = Turns::new();
        let mut now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");

        for _ in 0..TRIES - 1 {
            let held = turns.take(4, now).pop().expect("taken");
            assert_eq!(turns.refused(held, now), Refused::WaitsAgain);
            now += FIRST_WAIT * 8;
        }
        let held = turns.take(4, now).pop().expect("taken");
        assert_eq!(
            turns.refused(held, now),
            Refused::GivenUpOn,
            "something else is wrong"
        );
        assert_eq!(turns.how_much_waiting(), 0);
    }

    #[test]
    fn work_that_was_refused_does_not_hold_up_the_rest() {
        let turns = Turns::new();
        let now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");
        turns.hold(&session("two"), "second").expect("held");

        let held = turns.take(1, now).pop().expect("taken");
        turns.refused(held, now);

        let going = turns.take(1, now);
        assert_eq!(going.len(), 1);
        assert_eq!(going[0].session, session("two"), "the other one goes now");
    }

    #[test]
    fn what_waits_goes_in_the_order_it_was_asked_for() {
        let turns = Turns::new();
        turns.allow(2);
        assert_eq!(
            turns.hold(&session("one"), "first").expect("held"),
            0,
            "the first has nothing ahead of it"
        );
        assert_eq!(turns.hold(&session("two"), "second").expect("held"), 1);
        assert_eq!(turns.how_much_waiting(), 2);

        let going = turns.take(1, Instant::now());
        assert_eq!(going.len(), 1);
        assert_eq!(going[0].session, session("one"));
        assert_eq!(going[0].text, "first");
        assert_eq!(turns.how_much_waiting(), 1, "the other is still waiting");
    }

    #[test]
    fn a_pad_pressed_twice_waits_once_with_the_newer_instruction() {
        let turns = Turns::new();
        turns.allow(1);
        turns.hold(&session("one"), "first").expect("held");
        turns.hold(&session("one"), "no, this").expect("held");

        assert_eq!(turns.how_much_waiting(), 1, "one pad, one piece of work");
        assert_eq!(turns.take(4, Instant::now())[0].text, "no, this");
    }

    #[test]
    fn work_a_session_no_longer_needs_is_dropped() {
        let turns = Turns::new();
        turns.hold(&session("one"), "first").expect("held");
        turns.hold(&session("two"), "second").expect("held");

        turns.forget(&session("one"));

        let left = turns.take(4, Instant::now());
        assert_eq!(left.len(), 1);
        assert_eq!(
            left[0].session,
            session("two"),
            "the one that was told something else waits for nothing"
        );
    }

    #[test]
    fn a_queue_nobody_could_read_is_refused_rather_than_grown() {
        let turns = Turns::new();
        for index in 0..MOST_WAITING {
            turns
                .hold(&session(&index.to_string()), "work")
                .expect("held");
        }
        assert_eq!(turns.hold(&session("one too many"), "work"), Err(Full));
        assert_eq!(turns.how_much_waiting(), MOST_WAITING);
    }

    #[test]
    fn work_the_operator_cancelled_does_not_come_back_when_the_agent_refuses_it() {
        // The refusal arrives after the fact. By then the operator has
        // stopped the session, and sending it anyway would be PushOS arguing
        // with them.
        let turns = Turns::new();
        let now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");
        let held = turns.take(1, now).pop().expect("taken");

        turns.forget(&session("one"));

        assert_eq!(turns.refused(held, now), Refused::NoLongerWanted);
        assert_eq!(turns.how_much_waiting(), 0);
    }

    #[test]
    fn a_pad_pressed_again_while_the_agent_was_deciding_keeps_only_the_newer_work() {
        let turns = Turns::new();
        let now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");
        let held = turns.take(1, now).pop().expect("taken");

        turns.hold(&session("one"), "no, this").expect("held");
        assert_eq!(turns.refused(held, now), Refused::NoLongerWanted);

        assert_eq!(turns.how_much_waiting(), 1, "one pad, one piece of work");
        let going = turns.take(4, now);
        assert_eq!(going.len(), 1);
        assert_eq!(going[0].text, "no, this");
    }

    #[test]
    fn work_that_went_is_not_still_held_for_the_session() {
        // Otherwise the next refusal for that session would be matched
        // against work that finished long ago.
        let turns = Turns::new();
        let now = Instant::now();
        turns.hold(&session("one"), "first").expect("held");
        let held = turns.take(1, now).pop().expect("taken");

        turns.done(&session("one"));

        assert_eq!(turns.refused(held, now), Refused::NoLongerWanted);
        assert_eq!(turns.how_much_waiting(), 0);
    }

    #[test]
    fn taking_more_than_is_waiting_takes_what_there_is() {
        let turns = Turns::new();
        turns.hold(&session("one"), "first").expect("held");
        assert_eq!(turns.take(10, Instant::now()).len(), 1);
        assert!(turns.take(10, Instant::now()).is_empty());
    }
}
