//! How hard the surface works when nobody is using it.
//!
//! A Push 2 left on for a twelve-hour day wears in two places. Its pads and
//! buttons are LEDs, and its screen is an LCD lit by a row of LEDs behind it.
//! LEDs lose brightness with the hours they spend driven hard, and an LCD
//! holding the same picture for hours can keep a faint trace of it. Neither is
//! fixed by software after the fact; both are slowed by not running at full
//! brightness when nobody is looking.
//!
//! So the surface has three states. In use it runs at the configured
//! brightness, which is not full. Left alone it dims, still readable. Left
//! alone for longer the screen goes dark and every light goes out except the
//! ones asking for a person, because a board that hides a question it was left
//! running to show would be a board nobody could leave running.

use std::time::{Duration, Instant};

/// How awake the surface is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rest {
    /// Being used, or used recently.
    #[default]
    Awake,
    /// Left alone for a while. Everything is still shown, at lower brightness.
    Dimmed,
    /// Left alone for long enough that nobody is watching. The screen is dark
    /// and only the lights asking for a person stay on.
    Asleep,
}

impl Rest {
    /// Whether the screen shows anything at all.
    pub const fn shows_screen(self) -> bool {
        !matches!(self, Self::Asleep)
    }

    /// Whether a press should be spent waking the surface rather than acting.
    ///
    /// Only from sleep. A dimmed surface is still readable and a press on it
    /// means what it says; a dark one is being pressed blind, and acting on
    /// that press would be acting on a guess.
    pub const fn swallows_waking_input(self) -> bool {
        matches!(self, Self::Asleep)
    }
}

/// How bright the lights and the screen are, in percent of the hardware's
/// maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Levels {
    /// Every pad and button light.
    pub lights: u8,
    /// The screen's backlight. Zero is off.
    pub screen: u8,
}

/// When the surface dims and sleeps, and how bright it is while in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestPolicy {
    /// Brightness while in use, in percent.
    pub brightness: u8,
    /// How long untouched before dimming. `None` never dims.
    pub dim_after: Option<Duration>,
    /// How long untouched before sleeping. `None` never sleeps.
    pub sleep_after: Option<Duration>,
}

impl RestPolicy {
    /// What a surface does when configuration says nothing.
    ///
    /// Seventy percent is bright enough to read across a desk in daylight and
    /// takes the hardest third off every LED for the whole of the day. Ten
    /// minutes is a pause; half an hour is somebody who has gone.
    pub const DEFAULT: Self = Self {
        brightness: 70,
        dim_after: Some(Duration::from_mins(10)),
        sleep_after: Some(Duration::from_mins(30)),
    };

    /// How far below the in-use brightness a dimmed surface runs.
    ///
    /// A third: still readable at arm's length, a small fraction of the work.
    const DIM_DIVISOR: u8 = 3;

    /// The dimmest a lit thing is allowed to get, so dimming never becomes off.
    const FLOOR: u8 = 5;

    /// What the surface should be after `idle` without being touched.
    pub fn state_after(&self, idle: Duration) -> Rest {
        if self.sleep_after.is_some_and(|after| idle >= after) {
            return Rest::Asleep;
        }
        if self.dim_after.is_some_and(|after| idle >= after) {
            return Rest::Dimmed;
        }
        Rest::Awake
    }

    /// When the state will next change, given when it was last touched.
    ///
    /// `None` once there is nothing left to change to, so a sleeping surface
    /// sets no timer and costs nothing until someone reaches for it.
    pub fn next_change(&self, touched: Instant, now: Instant) -> Option<Instant> {
        let idle = now.saturating_duration_since(touched);
        [self.dim_after, self.sleep_after]
            .into_iter()
            .flatten()
            .filter(|after| *after > idle)
            .min()
            .map(|after| touched + after)
    }

    /// How bright things are in a state.
    pub fn levels(&self, rest: Rest) -> Levels {
        let dimmed = (self.brightness / Self::DIM_DIVISOR).max(Self::FLOOR.min(self.brightness));
        match rest {
            Rest::Awake => Levels {
                lights: self.brightness,
                screen: self.brightness,
            },
            Rest::Dimmed => Levels {
                lights: dimmed,
                screen: dimmed,
            },
            // The lights still lit are the ones asking for a person, so they
            // keep the dimmed level rather than going out with the rest.
            Rest::Asleep => Levels {
                lights: dimmed,
                screen: 0,
            },
        }
    }
}

impl Default for RestPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn a_surface_being_used_is_awake() {
        let policy = RestPolicy::DEFAULT;
        assert_eq!(policy.state_after(Duration::ZERO), Rest::Awake);
        assert_eq!(policy.state_after(9 * MINUTE), Rest::Awake);
    }

    #[test]
    fn left_alone_it_dims_and_then_sleeps() {
        let policy = RestPolicy::DEFAULT;
        assert_eq!(policy.state_after(10 * MINUTE), Rest::Dimmed);
        assert_eq!(policy.state_after(29 * MINUTE), Rest::Dimmed);
        assert_eq!(policy.state_after(30 * MINUTE), Rest::Asleep);
        assert_eq!(policy.state_after(12 * 60 * MINUTE), Rest::Asleep);
    }

    #[test]
    fn a_stage_that_is_switched_off_never_happens() {
        let never_sleeps = RestPolicy {
            sleep_after: None,
            ..RestPolicy::DEFAULT
        };
        assert_eq!(never_sleeps.state_after(12 * 60 * MINUTE), Rest::Dimmed);

        let always_on = RestPolicy {
            dim_after: None,
            sleep_after: None,
            ..RestPolicy::DEFAULT
        };
        assert_eq!(always_on.state_after(12 * 60 * MINUTE), Rest::Awake);
    }

    #[test]
    fn it_can_sleep_without_dimming_first() {
        let straight_to_sleep = RestPolicy {
            dim_after: None,
            ..RestPolicy::DEFAULT
        };
        assert_eq!(straight_to_sleep.state_after(20 * MINUTE), Rest::Awake);
        assert_eq!(straight_to_sleep.state_after(30 * MINUTE), Rest::Asleep);
    }

    #[test]
    fn the_next_timer_is_the_next_stage_and_there_is_none_once_asleep() {
        let policy = RestPolicy::DEFAULT;
        let touched = Instant::now();

        assert_eq!(
            policy.next_change(touched, touched),
            Some(touched + 10 * MINUTE)
        );
        assert_eq!(
            policy.next_change(touched, touched + 15 * MINUTE),
            Some(touched + 30 * MINUTE)
        );
        assert_eq!(
            policy.next_change(touched, touched + 31 * MINUTE),
            None,
            "a sleeping surface waits for a hand, not a clock"
        );
    }

    #[test]
    fn in_use_never_means_full_brightness_unless_asked() {
        let levels = RestPolicy::DEFAULT.levels(Rest::Awake);
        assert!(levels.lights < 100 && levels.screen < 100);
    }

    #[test]
    fn dimming_lowers_everything_and_sleep_turns_the_screen_off() {
        let policy = RestPolicy::DEFAULT;
        let awake = policy.levels(Rest::Awake);
        let dimmed = policy.levels(Rest::Dimmed);
        let asleep = policy.levels(Rest::Asleep);

        assert!(dimmed.lights < awake.lights && dimmed.screen < awake.screen);
        assert_eq!(asleep.screen, 0, "a dark screen holds no picture");
        assert!(
            asleep.lights > 0,
            "the lights asking for a person must stay visible"
        );
    }

    #[test]
    fn dimming_a_dim_surface_never_turns_it_off() {
        let dim = RestPolicy {
            brightness: 6,
            ..RestPolicy::DEFAULT
        };
        assert!(dim.levels(Rest::Dimmed).lights > 0);
        assert!(dim.levels(Rest::Dimmed).screen > 0);
        assert!(dim.levels(Rest::Dimmed).lights <= dim.brightness);
    }

    #[test]
    fn only_a_sleeping_surface_spends_a_press_on_waking() {
        assert!(Rest::Asleep.swallows_waking_input());
        assert!(!Rest::Dimmed.swallows_waking_input());
        assert!(!Rest::Awake.swallows_waking_input());
    }
}
