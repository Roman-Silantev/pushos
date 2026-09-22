//! How many agents may be talking at once.
//!
//! Sixty-four threads cost almost nothing to hold, but a vendor will not take
//! sixty-four turns at once: measured on both Claude and Codex, more than two
//! or three simultaneous streaming turns draws `429`s. That is not a fault in
//! the fleet, it is the shape of the thing — a fleet is mostly agents waiting,
//! the same way a mixing desk is mostly channels nobody is touching.
//!
//! So a turn takes a place before it starts, and gives it up when it finishes.
//! An agent with no place to take is `Queued`, which the domain already has a
//! word for and the surface already has a colour for. Nothing is dropped and
//! nothing is retried: the work happens in the order it was asked for.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::debug;

/// How many agents may be mid-turn at once, when nothing says otherwise.
///
/// Three. Both vendors start refusing at around this point, and a fourth in
/// flight would not finish sooner — it would fail, and be indistinguishable on
/// the surface from an agent that had gone wrong.
pub const AT_ONCE: usize = 3;

/// The places available to agents that want to talk.
#[derive(Debug)]
pub struct TurnGate {
    places: Arc<Semaphore>,
    /// The place each mid-turn thread is holding, given up when it finishes.
    held: Mutex<HashMap<String, OwnedSemaphorePermit>>,
}

impl TurnGate {
    /// Builds a gate allowing `at_once` turns to run together.
    ///
    /// Zero would stop the fleet entirely, so it is read as one: a surface
    /// that does nothing is a worse answer than a slow one.
    pub fn new(at_once: usize) -> Self {
        Self {
            places: Arc::new(Semaphore::new(at_once.max(1))),
            held: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a turn could start right now without waiting.
    ///
    /// Asked before waiting, so an agent that is about to queue can be shown
    /// as queued rather than appearing to have stalled.
    pub fn would_wait(&self) -> bool {
        self.places.available_permits() == 0
    }

    /// Takes a place for a thread, waiting until one is free.
    ///
    /// The place is held until [`Self::finished`] is called for that thread,
    /// which is what makes the limit about turns rather than about calls.
    pub async fn take(&self, thread: &str) {
        let Ok(place) = Arc::clone(&self.places).acquire_owned().await else {
            // The semaphore is never closed, so this cannot happen; if it ever
            // did, letting the turn through is better than stopping the fleet.
            debug!(thread, "the turn gate was closed");
            return;
        };
        self.held.lock().await.insert(thread.to_owned(), place);
    }

    /// Gives up the place a thread was holding.
    ///
    /// Safe to call for a thread that holds none: a turn can end more than one
    /// way, and every one of them says so.
    pub async fn finished(&self, thread: &str) {
        if self.held.lock().await.remove(thread).is_some() {
            debug!(thread, "gave up its place");
        }
    }

    /// How many turns are running.
    pub async fn running(&self) -> usize {
        self.held.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn the_first_few_turns_start_without_waiting() {
        let gate = TurnGate::new(3);
        for thread in ["a", "b", "c"] {
            assert!(!gate.would_wait(), "{thread} should not have to wait");
            gate.take(thread).await;
        }
        assert_eq!(gate.running().await, 3);
        assert!(gate.would_wait(), "the fourth has nowhere to go");
    }

    #[tokio::test]
    async fn a_fourth_turn_waits_until_one_finishes() {
        let gate = Arc::new(TurnGate::new(3));
        for thread in ["a", "b", "c"] {
            gate.take(thread).await;
        }

        let waiting = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move {
                gate.take("d").await;
            }
        });

        // It must still be waiting: nothing has finished.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the fourth turn should be queued");

        gate.finished("b").await;
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("the fourth turn starts once a place frees")
            .expect("the task did not panic");

        assert_eq!(
            gate.running().await,
            3,
            "still three, with `d` in `b`'s place"
        );
    }

    #[tokio::test]
    async fn a_thread_that_holds_no_place_can_still_say_it_finished() {
        // A turn can end by completing, by failing, or by being interrupted,
        // and all three report it. Only the first should give up a place.
        let gate = TurnGate::new(2);
        gate.finished("never-started").await;
        gate.take("a").await;
        gate.finished("a").await;
        gate.finished("a").await;
        assert_eq!(gate.running().await, 0);
        assert!(!gate.would_wait());
    }

    #[tokio::test]
    async fn a_gate_of_none_still_lets_one_through() {
        // Misconfiguration should slow the fleet, not stop it.
        let gate = TurnGate::new(0);
        assert!(!gate.would_wait());
        gate.take("a").await;
        assert_eq!(gate.running().await, 1);
    }

    #[tokio::test]
    async fn sixty_four_agents_queue_rather_than_all_talking_at_once() {
        // The claim the whole gate exists for: a fleet far larger than the
        // vendor's ceiling still runs, three at a time.
        let gate = Arc::new(TurnGate::new(AT_ONCE));
        let mut turns = Vec::new();

        for i in 0..64 {
            let gate = Arc::clone(&gate);
            turns.push(tokio::spawn(async move {
                let thread = format!("t{i}");
                gate.take(&thread).await;
                assert!(
                    gate.running().await <= AT_ONCE,
                    "more than {AT_ONCE} turns were running at once"
                );
                gate.finished(&thread).await;
            }));
        }

        for turn in turns {
            tokio::time::timeout(Duration::from_secs(10), turn)
                .await
                .expect("every turn gets its place eventually")
                .expect("no turn panicked");
        }
        assert_eq!(gate.running().await, 0, "every place was given back");
    }
}
