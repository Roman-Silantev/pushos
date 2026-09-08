//! A clock a test drives by hand.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pushos_domain::ports::Clock;

/// A clock that only moves when a test tells it to.
///
/// Lets timeout and gesture behaviour be exercised in microseconds rather than
/// by sleeping, which keeps the suite fast and free of flakiness.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<Instant>>,
}

impl ManualClock {
    /// Starts a clock at the current instant.
    pub fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        if let Ok(mut now) = self.now.lock() {
            *now += by;
        }
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.now
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |now| *now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_stands_still_until_it_is_advanced() {
        let clock = ManualClock::new();
        let start = clock.now();
        assert_eq!(clock.now(), start);

        clock.advance(Duration::from_millis(500));
        assert_eq!(clock.now(), start + Duration::from_millis(500));
    }

    #[test]
    fn clones_share_one_timeline() {
        let clock = ManualClock::new();
        let other = clock.clone();
        clock.advance(Duration::from_secs(1));
        assert_eq!(other.now(), clock.now());
    }
}
