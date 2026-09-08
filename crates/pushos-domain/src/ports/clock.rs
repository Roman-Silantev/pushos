//! The time port.
//!
//! Gesture recognition, timeouts and animation phase all depend on time.
//! Injecting it keeps those testable without sleeping.

use std::time::Instant;

/// Supplies the monotonic time the runtime reasons about.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The current instant.
    fn now(&self) -> Instant;
}

/// The real system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_moves_forward() {
        let clock = SystemClock;
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }
}
