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

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use pushos_domain::ids::AttachedId;

/// The most work that may wait at once.
///
/// One per pad and some to spare. Past this, a pad says so rather than adding
/// to a queue nobody could read.
const MOST_WAITING: usize = 128;

/// Work that has not started yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Waiting {
    /// Which session it is for.
    pub(super) session: AttachedId,
    /// What to tell it.
    pub(super) text: String,
}

/// How many may work at once, and what is waiting for a turn.
#[derive(Debug, Default)]
pub(super) struct Turns {
    /// The most sessions that may be working at once. Nought is no limit.
    allowed: AtomicUsize,
    waiting: Mutex<VecDeque<Waiting>>,
}

impl Turns {
    /// A book with no limit, until the runtime says what the limit is.
    pub(super) const fn new() -> Self {
        Self {
            allowed: AtomicUsize::new(0),
            waiting: Mutex::new(VecDeque::new()),
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

    /// Puts work aside until a session is free, and says how much is ahead.
    ///
    /// Replaces whatever that session was already waiting to be told: the
    /// newer instruction is the one the operator meant, exactly as pressing a
    /// pad twice means the second thing.
    pub(super) fn hold(&self, session: &AttachedId, text: &str) -> Result<usize, Full> {
        let Ok(mut waiting) = self.waiting.lock() else {
            return Err(Full);
        };
        if let Some(already) = waiting.iter_mut().find(|held| &held.session == session) {
            text.clone_into(&mut already.text);
            return Ok(waiting.len());
        }
        if waiting.len() >= MOST_WAITING {
            return Err(Full);
        }
        waiting.push_back(Waiting {
            session: session.clone(),
            text: text.to_owned(),
        });
        Ok(waiting.len())
    }

    /// Takes the next `free` pieces of work, oldest first.
    pub(super) fn take(&self, free: usize) -> Vec<Waiting> {
        let Ok(mut waiting) = self.waiting.lock() else {
            return Vec::new();
        };
        let taking = free.min(waiting.len());
        waiting.drain(..taking).collect()
    }

    /// Drops work for any session that is no longer there.
    ///
    /// A pad whose session was closed elsewhere would otherwise keep its work
    /// waiting for a turn that can never come.
    pub(super) fn keep_only(&self, open: &[AttachedId]) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.retain(|held| open.contains(&held.session));
        }
    }

    /// Whether a session has work waiting for a turn.
    pub(super) fn holds_work_for(&self, session: &AttachedId) -> bool {
        self.waiting
            .lock()
            .is_ok_and(|waiting| waiting.iter().any(|held| &held.session == session))
    }

    /// How much work is waiting.
    pub(super) fn waiting(&self) -> usize {
        self.waiting.lock().map_or(0, |waiting| waiting.len())
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
    fn what_waits_goes_in_the_order_it_was_asked_for() {
        let turns = Turns::new();
        turns.allow(2);
        assert_eq!(turns.hold(&session("one"), "first").expect("held"), 1);
        assert_eq!(turns.hold(&session("two"), "second").expect("held"), 2);
        assert_eq!(turns.waiting(), 2);

        let going = turns.take(1);
        assert_eq!(going.len(), 1);
        assert_eq!(going[0].session, session("one"));
        assert_eq!(going[0].text, "first");
        assert_eq!(turns.waiting(), 1, "the other is still waiting");
    }

    #[test]
    fn a_pad_pressed_twice_waits_once_with_the_newer_instruction() {
        let turns = Turns::new();
        turns.allow(1);
        turns.hold(&session("one"), "first").expect("held");
        turns.hold(&session("one"), "no, this").expect("held");

        assert_eq!(turns.waiting(), 1, "one pad, one piece of work");
        assert_eq!(turns.take(4)[0].text, "no, this");
    }

    #[test]
    fn a_session_that_goes_takes_its_waiting_work_with_it() {
        let turns = Turns::new();
        turns.hold(&session("one"), "first").expect("held");
        turns.hold(&session("two"), "second").expect("held");

        turns.keep_only(&[session("two")]);

        let left = turns.take(4);
        assert_eq!(left.len(), 1);
        assert_eq!(
            left[0].session,
            session("two"),
            "the one whose session closed waits for nothing"
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
        assert_eq!(turns.waiting(), MOST_WAITING);
    }

    #[test]
    fn taking_more_than_is_waiting_takes_what_there_is() {
        let turns = Turns::new();
        turns.hold(&session("one"), "first").expect("held");
        assert_eq!(turns.take(10).len(), 1);
        assert!(turns.take(10).is_empty());
    }
}
