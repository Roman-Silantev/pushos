//! A single-slot mailbox that keeps only the newest value.
//!
//! The display consumes whole frames. If the renderer produces faster than USB
//! drains, the right answer is to skip stale frames rather than queue them, so
//! the panel always shows the most recent state and memory stays bounded.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The shared slot. Writers replace; the reader takes.
#[derive(Debug)]
pub(crate) struct LatestSlot<T> {
    state: Mutex<SlotState<T>>,
    changed: Condvar,
}

#[derive(Debug)]
struct SlotState<T> {
    value: Option<T>,
    closed: bool,
}

/// What a waiting reader was woken for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SlotEvent<T> {
    /// A new value arrived.
    Value(T),
    /// Nothing arrived before the deadline.
    Idle,
    /// The writer went away and no value is pending.
    Closed,
}

impl<T> LatestSlot<T> {
    /// Creates an empty slot.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SlotState {
                value: None,
                closed: false,
            }),
            changed: Condvar::new(),
        })
    }

    /// Replaces the pending value, discarding any the reader has not taken.
    ///
    /// Returns `false` once the slot is closed.
    pub(crate) fn publish(&self, value: T) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }
        state.value = Some(value);
        drop(state);
        self.changed.notify_one();
        true
    }

    /// Waits for a value, giving up after `timeout`.
    ///
    /// A timeout is not an error: the display uses it to re-send the last frame
    /// and keep the panel awake.
    pub(crate) fn take_within(&self, timeout: Duration) -> SlotEvent<T> {
        let Ok(mut state) = self.state.lock() else {
            return SlotEvent::Closed;
        };

        if let Some(value) = state.value.take() {
            return SlotEvent::Value(value);
        }
        if state.closed {
            return SlotEvent::Closed;
        }

        let Ok((mut state, wait)) = self.changed.wait_timeout(state, timeout) else {
            return SlotEvent::Closed;
        };

        match state.value.take() {
            Some(value) => SlotEvent::Value(value),
            None if state.closed => SlotEvent::Closed,
            None if wait.timed_out() => SlotEvent::Idle,
            None => SlotEvent::Idle,
        }
    }

    /// Marks the slot closed and wakes the reader.
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    const INSTANT: Duration = Duration::from_millis(50);

    #[test]
    fn a_published_value_is_taken_once() {
        let slot = LatestSlot::new();
        assert!(slot.publish(7));
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Value(7));
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Idle);
    }

    #[test]
    fn only_the_newest_value_survives() {
        let slot = LatestSlot::new();
        slot.publish(1);
        slot.publish(2);
        slot.publish(3);
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Value(3));
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Idle);
    }

    #[test]
    fn an_idle_slot_reports_a_timeout_rather_than_blocking_forever() {
        let slot = LatestSlot::<u8>::new();
        assert_eq!(slot.take_within(Duration::from_millis(10)), SlotEvent::Idle);
    }

    #[test]
    fn closing_wakes_a_waiting_reader() {
        let slot = LatestSlot::<u8>::new();
        let writer = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            writer.close();
        });

        assert_eq!(slot.take_within(Duration::from_secs(5)), SlotEvent::Closed);
        handle.join().expect("writer thread finished");
        assert!(!slot.publish(1), "a closed slot rejects further values");
    }

    #[test]
    fn a_value_published_before_closing_is_still_delivered() {
        let slot = LatestSlot::new();
        slot.publish(42);
        slot.close();
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Value(42));
        assert_eq!(slot.take_within(INSTANT), SlotEvent::Closed);
    }

    #[test]
    fn a_reader_is_woken_by_a_value_from_another_thread() {
        let slot = LatestSlot::new();
        let writer = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            writer.publish(99);
        });

        assert_eq!(
            slot.take_within(Duration::from_secs(5)),
            SlotEvent::Value(99)
        );
        handle.join().expect("writer thread finished");
    }
}
