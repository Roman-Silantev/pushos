//! Surviving the hardware coming and going.
//!
//! PushOS is expected to run with the Push 2 unplugged. The runtime stays up,
//! waits, and takes the surface back when it returns. Retry backs off so an
//! absent device costs nothing, and reconnection restores the display and the
//! lights rather than assuming they survived.

use std::time::Duration;

/// How long to wait before the first retry.
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// The longest PushOS will wait between attempts.
pub const MAXIMUM_BACKOFF: Duration = Duration::from_secs(10);

/// Works out how long to wait before the next connection attempt.
///
/// Backing off matters because an unplugged Push 2 is a normal, indefinite
/// state, and probing it every half second forever would keep a laptop awake
/// for nothing.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    current: Duration,
}

impl Backoff {
    /// A backoff that has not yet waited.
    pub const fn new() -> Self {
        Self {
            current: INITIAL_BACKOFF,
        }
    }

    /// The wait before the next attempt, doubling up to the maximum.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(MAXIMUM_BACKOFF);
        delay
    }

    /// Returns to the shortest wait, after a successful connection.
    pub fn reset(&mut self) {
        self.current = INITIAL_BACKOFF;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_wait_is_short_so_a_quick_replug_is_picked_up_immediately() {
        assert_eq!(Backoff::new().next_delay(), INITIAL_BACKOFF);
    }

    #[test]
    fn waiting_doubles_up_to_the_maximum_and_stops_there() {
        let mut backoff = Backoff::new();
        let mut previous = backoff.next_delay();

        for _ in 0..20 {
            let delay = backoff.next_delay();
            assert!(delay >= previous, "backoff must not shrink on its own");
            assert!(delay <= MAXIMUM_BACKOFF, "backoff must stay bounded");
            previous = delay;
        }

        assert_eq!(previous, MAXIMUM_BACKOFF);
    }

    #[test]
    fn a_successful_connection_returns_to_the_shortest_wait() {
        let mut backoff = Backoff::new();
        for _ in 0..10 {
            backoff.next_delay();
        }
        backoff.reset();
        assert_eq!(backoff.next_delay(), INITIAL_BACKOFF);
    }
}
