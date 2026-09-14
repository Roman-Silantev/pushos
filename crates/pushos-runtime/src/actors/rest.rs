//! Keeping track of whether anyone is using the surface.
//!
//! The rules live in the domain. This is the one thing that knows when the
//! surface was last touched and how many lights are asking for a person, and
//! turns that into a state the renderer can act on. It is plain data driven by
//! the input pipeline, with every moment passed in, so all of it can be tested
//! without waiting for a real half hour.

use std::time::Instant;

use pushos_domain::controls::ControlId;
use pushos_domain::input::ControlEvent;
use pushos_domain::rest::{Levels, Rest, RestPolicy};

/// Whether an input event should reach the gesture recogniser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// It means what it says.
    Act,
    /// It was spent waking a dark surface, and does nothing else.
    Spent,
}

/// When the surface was last used, and what that makes it now.
#[derive(Debug)]
pub(crate) struct RestClock {
    policy: RestPolicy,
    touched: Instant,
    state: Rest,
    /// The control whose press woke a sleeping surface.
    ///
    /// Its release is spent too. Letting the release through would hand the
    /// recogniser half of a gesture, and a binding on release would fire for a
    /// press nobody meant as one.
    waking: Option<ControlId>,
    /// How many lights were asking for a person at the last look.
    asking: usize,
}

impl RestClock {
    /// Starts the clock, with the surface just used.
    pub(crate) const fn new(policy: RestPolicy, now: Instant) -> Self {
        Self {
            policy,
            touched: now,
            state: Rest::Awake,
            waking: None,
            asking: 0,
        }
    }

    /// Where the surface is now.
    pub(crate) const fn state(&self) -> Rest {
        self.state
    }

    /// How bright it should be now.
    pub(crate) fn levels(&self) -> Levels {
        self.policy.levels(self.state)
    }

    /// When time alone will next change anything.
    pub(crate) fn next_change(&self, now: Instant) -> Option<Instant> {
        self.policy.next_change(self.touched, now)
    }

    /// Takes a new policy from a reloaded configuration.
    ///
    /// Returns whether that changed the state. The last touch is kept, so
    /// shortening a timer takes effect at once rather than a whole timer later.
    pub(crate) fn adopt(&mut self, policy: RestPolicy, now: Instant) -> bool {
        self.policy = policy;
        self.settle(now)
    }

    /// Records a hand on the surface.
    ///
    /// Returns what should happen to the event, and whether the state changed.
    pub(crate) fn on_input(&mut self, event: &ControlEvent) -> (Heard, bool) {
        if self.waking == Some(event.control) {
            if event.phase.is_releasing() {
                self.waking = None;
            }
            self.touched = event.at;
            return (Heard::Spent, false);
        }

        self.touched = event.at;
        let was = self.state;
        self.state = Rest::Awake;

        // A release is the end of something that began while awake, so it is
        // let through rather than leaving the recogniser holding a press.
        if was.swallows_waking_input() && !event.phase.is_releasing() {
            if event.phase.is_engaging() {
                self.waking = Some(event.control);
            }
            return (Heard::Spent, true);
        }

        (Heard::Act, was != Rest::Awake)
    }

    /// Records how many lights are asking for a person.
    ///
    /// Something new asking wakes the surface: a board left running to show
    /// questions has to show the question. Returns whether the state changed.
    pub(crate) fn on_asking(&mut self, asking: usize, now: Instant) -> bool {
        let more = asking > self.asking;
        self.asking = asking;
        if !more || self.state == Rest::Awake {
            return false;
        }
        self.touched = now;
        self.state = Rest::Awake;
        true
    }

    /// Lets time pass. Returns whether the state changed.
    pub(crate) fn on_time(&mut self, now: Instant) -> bool {
        self.settle(now)
    }

    fn settle(&mut self, now: Instant) -> bool {
        let next = self
            .policy
            .state_after(now.saturating_duration_since(self.touched));
        let changed = next != self.state;
        self.state = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pushos_domain::controls::{EncoderId, PadIndex};
    use pushos_domain::input::InputPhase;

    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn pad(at: Instant, phase: InputPhase) -> ControlEvent {
        ControlEvent::new(
            ControlId::Pad(PadIndex::new(0).expect("in range")),
            phase,
            at,
        )
    }

    fn asleep(start: Instant) -> RestClock {
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);
        assert!(clock.on_time(start + 31 * MINUTE));
        assert_eq!(clock.state(), Rest::Asleep);
        clock
    }

    #[test]
    fn time_alone_dims_the_surface_and_then_puts_it_to_sleep() {
        let start = Instant::now();
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);

        assert!(!clock.on_time(start + 5 * MINUTE));
        assert!(clock.on_time(start + 10 * MINUTE));
        assert_eq!(clock.state(), Rest::Dimmed);
        assert!(clock.on_time(start + 30 * MINUTE));
        assert_eq!(clock.state(), Rest::Asleep);
        assert_eq!(clock.levels().screen, 0);
    }

    #[test]
    fn a_press_on_a_dark_surface_wakes_it_and_does_nothing_else() {
        // Pressed blind, it is a guess. The press and its release are both
        // spent, so no binding on either fires.
        let start = Instant::now();
        let mut clock = asleep(start);
        let later = start + 40 * MINUTE;

        assert_eq!(
            clock.on_input(&pad(later, InputPhase::Down { velocity: 100 })),
            (Heard::Spent, true)
        );
        assert_eq!(clock.state(), Rest::Awake);
        assert_eq!(
            clock.on_input(&pad(later, InputPhase::Up)),
            (Heard::Spent, false),
            "the release belongs to the press that woke it"
        );

        // The next press is a real one.
        assert_eq!(
            clock.on_input(&pad(later, InputPhase::Down { velocity: 100 })),
            (Heard::Act, false)
        );
    }

    #[test]
    fn a_knob_turned_on_a_dark_surface_wakes_it_without_moving_anything() {
        let start = Instant::now();
        let mut clock = asleep(start);
        let turn = ControlEvent::new(
            ControlId::Encoder(EncoderId::Track(3)),
            InputPhase::Turn { delta: 1 },
            start + 40 * MINUTE,
        );
        assert_eq!(clock.on_input(&turn), (Heard::Spent, true));
        assert_eq!(clock.on_input(&turn), (Heard::Act, false));
    }

    #[test]
    fn a_press_on_a_dimmed_surface_means_what_it_says() {
        // Still readable, so not a guess.
        let start = Instant::now();
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);
        clock.on_time(start + 15 * MINUTE);

        assert_eq!(
            clock.on_input(&pad(
                start + 15 * MINUTE,
                InputPhase::Down { velocity: 100 }
            )),
            (Heard::Act, true)
        );
        assert_eq!(clock.state(), Rest::Awake);
    }

    #[test]
    fn using_the_surface_starts_the_timers_again() {
        let start = Instant::now();
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);
        clock.on_input(&pad(start + 9 * MINUTE, InputPhase::Down { velocity: 1 }));

        assert!(!clock.on_time(start + 15 * MINUTE), "only six minutes idle");
        assert_eq!(clock.state(), Rest::Awake);
        assert_eq!(
            clock.next_change(start + 15 * MINUTE),
            Some(start + 19 * MINUTE)
        );
    }

    #[test]
    fn something_new_asking_for_a_person_wakes_the_surface() {
        let start = Instant::now();
        let mut clock = asleep(start);
        let later = start + 2 * 60 * MINUTE;

        assert!(clock.on_asking(1, later), "a question appeared");
        assert_eq!(clock.state(), Rest::Awake);

        // And it rests again if nobody comes.
        assert!(clock.on_time(later + 30 * MINUTE));
        assert_eq!(clock.state(), Rest::Asleep);
    }

    #[test]
    fn the_same_questions_still_asking_do_not_keep_it_awake() {
        // Otherwise one session waiting overnight would hold the screen on all
        // night, which is the thing this exists to prevent.
        let start = Instant::now();
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);
        clock.on_asking(2, start);
        clock.on_time(start + 31 * MINUTE);

        assert!(!clock.on_asking(2, start + 32 * MINUTE));
        assert!(!clock.on_asking(1, start + 33 * MINUTE), "one was answered");
        assert_eq!(clock.state(), Rest::Asleep);
    }

    #[test]
    fn a_shorter_timer_from_a_reload_applies_at_once() {
        let start = Instant::now();
        let mut clock = RestClock::new(RestPolicy::DEFAULT, start);
        let impatient = RestPolicy {
            dim_after: Some(MINUTE),
            sleep_after: Some(2 * MINUTE),
            ..RestPolicy::DEFAULT
        };
        assert!(clock.adopt(impatient, start + 5 * MINUTE));
        assert_eq!(clock.state(), Rest::Asleep);
    }
}
