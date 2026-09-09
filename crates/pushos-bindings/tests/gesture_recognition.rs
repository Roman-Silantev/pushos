//! Gesture recognition behaviour, including the concurrency scenarios that
//! `SPEC.md` section 69 and `AGENTS.md` section 43 require to be tested
//! explicitly.
//!
//! Time is supplied rather than slept on, so these run instantly and
//! deterministically.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::{Duration, Instant};

use pushos_bindings::{
    GestureDetail, GestureEvent, GestureInterest, GestureRecognizer, GestureTiming, InterestKind,
};
use pushos_domain::controls::{ButtonId, ControlId, EncoderId, PadIndex};
use pushos_domain::gesture::Gesture;
use pushos_domain::input::{ControlEvent, InputPhase};

/// The default thresholds, restated in milliseconds so the timelines below read
/// as plain arithmetic. The first test keeps them honest.
const HOLD_MS: u64 = 400;
const DOUBLE_TAP_MS: u64 = 280;

#[test]
fn the_constants_used_here_match_the_shipped_defaults() {
    assert_eq!(
        GestureTiming::DEFAULT.hold_threshold,
        Duration::from_millis(HOLD_MS)
    );
    assert_eq!(
        GestureTiming::DEFAULT.double_tap_window,
        Duration::from_millis(DOUBLE_TAP_MS)
    );
}

fn pad(index: u8) -> ControlId {
    ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
}

const SHIFT: ControlId = ControlId::Button(ButtonId::Shift);

/// A recogniser plus a synthetic clock, so tests read as a timeline.
struct Surface {
    recognizer: GestureRecognizer,
    origin: Instant,
    produced: Vec<GestureEvent>,
}

impl Surface {
    fn with_interest(interest: GestureInterest) -> Self {
        Self {
            recognizer: GestureRecognizer::new(GestureTiming::DEFAULT, interest),
            origin: Instant::now(),
            produced: Vec::new(),
        }
    }

    fn bare() -> Self {
        Self::with_interest(GestureInterest::none())
    }

    fn watching(entries: impl IntoIterator<Item = (ControlId, InterestKind)>) -> Self {
        Self::with_interest(GestureInterest::from_iter(entries))
    }

    fn at(&self, offset_ms: u64) -> Instant {
        self.origin + Duration::from_millis(offset_ms)
    }

    fn input(&mut self, control: ControlId, phase: InputPhase, offset_ms: u64) {
        let at = self.at(offset_ms);
        self.recognizer
            .observe(ControlEvent::new(control, phase, at), &mut self.produced);
    }

    fn press(&mut self, control: ControlId, offset_ms: u64) {
        self.input(control, InputPhase::Down { velocity: 100 }, offset_ms);
    }

    fn release(&mut self, control: ControlId, offset_ms: u64) {
        self.input(control, InputPhase::Up, offset_ms);
    }

    fn advance_to(&mut self, offset_ms: u64) {
        let now = self.at(offset_ms);
        self.recognizer.poll(now, &mut self.produced);
    }

    /// Takes the gestures produced so far, leaving the buffer empty.
    fn drain(&mut self) -> Vec<Gesture> {
        self.drain_events()
            .into_iter()
            .map(|event| event.gesture)
            .collect()
    }

    fn drain_events(&mut self) -> Vec<GestureEvent> {
        std::mem::take(&mut self.produced)
    }
}

#[test]
fn a_press_reports_immediately_and_carries_velocity() {
    let mut surface = Surface::bare();
    surface.press(pad(0), 0);

    let events = surface.drain_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].gesture, Gesture::Press);
    assert_eq!(events[0].detail, GestureDetail::Velocity(100));
}

#[test]
fn a_tap_is_reported_on_release_when_no_double_tap_binding_exists() {
    let mut surface = Surface::bare();
    surface.press(pad(0), 0);
    surface.release(pad(0), 50);

    assert_eq!(
        surface.drain(),
        [Gesture::Press, Gesture::Release, Gesture::Tap]
    );
}

#[test]
fn a_tap_waits_for_the_double_tap_window_only_where_a_binding_needs_it() {
    let mut surface = Surface::watching([(pad(0), InterestKind::DoubleTap)]);
    surface.press(pad(0), 0);
    surface.release(pad(0), 50);

    assert_eq!(surface.drain(), [Gesture::Press, Gesture::Release]);

    surface.advance_to(50 + DOUBLE_TAP_MS - 1);
    assert!(surface.drain().is_empty(), "the window has not closed yet");

    surface.advance_to(50 + DOUBLE_TAP_MS);
    assert_eq!(surface.drain(), [Gesture::Tap]);
}

#[test]
fn two_presses_inside_the_window_become_one_double_tap() {
    let mut surface = Surface::watching([(pad(0), InterestKind::DoubleTap)]);
    surface.press(pad(0), 0);
    surface.release(pad(0), 40);
    surface.press(pad(0), 100);
    surface.release(pad(0), 140);

    let gestures = surface.drain();
    assert_eq!(
        gestures,
        [
            Gesture::Press,
            Gesture::Release,
            Gesture::Press,
            Gesture::Release,
            Gesture::DoubleTap
        ]
    );
    assert!(
        !gestures.contains(&Gesture::Tap),
        "a double tap must not also report a tap"
    );

    surface.advance_to(1_000);
    assert!(
        surface.drain().is_empty(),
        "the double tap settled the pending window"
    );
}

#[test]
fn a_second_press_after_the_window_is_two_separate_taps() {
    let mut surface = Surface::watching([(pad(0), InterestKind::DoubleTap)]);
    surface.press(pad(0), 0);
    surface.release(pad(0), 40);
    surface.advance_to(400);
    assert_eq!(
        surface.drain(),
        [Gesture::Press, Gesture::Release, Gesture::Tap]
    );

    surface.press(pad(0), 500);
    surface.release(pad(0), 540);
    surface.advance_to(900);
    assert_eq!(
        surface.drain(),
        [Gesture::Press, Gesture::Release, Gesture::Tap]
    );
}

#[test]
fn a_hold_fires_once_at_its_threshold_and_suppresses_the_tap() {
    let mut surface = Surface::watching([(pad(0), InterestKind::Hold)]);
    surface.press(pad(0), 0);
    surface.advance_to(HOLD_MS - 1);
    assert_eq!(surface.drain(), [Gesture::Press]);

    surface.advance_to(HOLD_MS);
    assert_eq!(surface.drain(), [Gesture::Hold]);

    surface.advance_to(2_000);
    assert!(
        surface.drain().is_empty(),
        "a hold must not repeat while the pad stays down"
    );

    surface.release(pad(0), 3_000);
    assert_eq!(
        surface.drain(),
        [Gesture::Release],
        "a held pad must not also report a tap"
    );
}

#[test]
fn shift_state_is_captured_at_press_and_survives_shift_being_released() {
    let mut surface = Surface::watching([(pad(0), InterestKind::Hold)]);
    surface.press(SHIFT, 0);
    surface.press(pad(0), 10);
    // Shift comes back up while the pad is still down.
    surface.release(SHIFT, 20);
    surface.advance_to(10 + HOLD_MS);

    let gestures = surface.drain();
    assert!(
        gestures.contains(&Gesture::ShiftPress),
        "the pad press was a shift press"
    );
    assert!(
        gestures.contains(&Gesture::ShiftHold),
        "the hold keeps the shift it started with"
    );
    assert!(!gestures.contains(&Gesture::Hold));
}

#[test]
fn encoder_turns_report_direction_and_delta() {
    let encoder = ControlId::Encoder(EncoderId::Track(2));
    let mut surface = Surface::bare();
    surface.input(encoder, InputPhase::Turn { delta: 3 }, 0);
    surface.input(encoder, InputPhase::Turn { delta: -2 }, 10);

    let events = surface.drain_events();
    assert_eq!(events[0].gesture, Gesture::TurnRight);
    assert_eq!(events[0].detail, GestureDetail::Delta(3));
    assert_eq!(events[1].gesture, Gesture::TurnLeft);
    assert_eq!(events[1].detail, GestureDetail::Delta(-2));
}

/// The lowest reading that stands for `percent` of the way up the strip.
fn along(percent: u32) -> InputPhase {
    InputPhase::Position {
        value: u16::try_from(percent.min(100) * 16_383 / 100 + 1).expect("within the strip"),
    }
}

#[test]
fn a_finger_on_the_strip_reports_where_it_is() {
    let strip = ControlId::TouchStrip;
    let mut surface = Surface::bare();
    surface.input(strip, InputPhase::Touch, 0);
    surface.input(strip, along(40), 5);

    let events = surface.drain_events();
    assert_eq!(events[0].gesture, Gesture::Touch);
    assert_eq!(events[1].gesture, Gesture::Slide);
    assert_eq!(events[1].detail.percent(), Some(40));
}

#[test]
fn a_strip_reports_once_per_percent_travelled() {
    // The hardware sends a reading whenever the finger moves at all, which is
    // far finer than anything can act on. One action per percent is as fine as
    // a finger is, and a hundredth of the work.
    let strip = ControlId::TouchStrip;
    let mut surface = Surface::bare();
    surface.input(strip, InputPhase::Touch, 0);
    // A percent of the strip is a hundred and sixty-odd of these, so all of
    // these readings stand for the same place on it.
    for step in 0..25u16 {
        surface.input(
            strip,
            InputPhase::Position {
                value: 8_000 + step,
            },
            5 + u64::from(step),
        );
    }

    let slides = surface
        .drain()
        .into_iter()
        .filter(|gesture| *gesture == Gesture::Slide)
        .count();
    assert_eq!(slides, 1, "readings inside one percent are one gesture");
}

#[test]
fn a_new_finger_reports_where_it_landed_even_in_the_same_place() {
    let strip = ControlId::TouchStrip;
    let mut surface = Surface::bare();
    surface.input(strip, InputPhase::Touch, 0);
    surface.input(strip, along(60), 5);
    surface.input(strip, InputPhase::TouchRelease, 10);
    surface.input(strip, InputPhase::Touch, 500);
    surface.input(strip, along(60), 505);

    let slides = surface
        .drain()
        .into_iter()
        .filter(|gesture| *gesture == Gesture::Slide)
        .count();
    assert_eq!(slides, 2, "a fresh touch is news wherever it lands");
}

#[test]
fn aftertouch_is_not_a_gesture() {
    // Pressure drives widgets directly. Making it one would put an action
    // behind every gram of a finger resting on a pad. A place along the strip
    // is different: it is where the operator asked to be, and nothing else on
    // the surface says it.
    let mut surface = Surface::bare();
    surface.press(pad(0), 0);
    surface.input(pad(0), InputPhase::Pressure { amount: 90 }, 5);

    assert_eq!(surface.drain(), [Gesture::Press]);
}

#[test]
fn a_turn_while_shift_is_held_is_a_shift_turn_in_either_direction() {
    let encoder = ControlId::Encoder(EncoderId::Track(0));
    let mut surface = Surface::bare();
    surface.press(SHIFT, 0);
    surface.input(encoder, InputPhase::Turn { delta: 1 }, 10);
    surface.input(encoder, InputPhase::Turn { delta: -1 }, 20);

    let gestures = surface.drain();
    assert_eq!(
        gestures
            .iter()
            .filter(|g| **g == Gesture::ShiftTurn)
            .count(),
        2
    );
}

#[test]
fn a_zero_delta_turn_produces_nothing() {
    let mut surface = Surface::bare();
    surface.input(
        ControlId::Encoder(EncoderId::Master),
        InputPhase::Turn { delta: 0 },
        0,
    );
    assert!(surface.drain().is_empty());
}

// --- Concurrency and recovery scenarios required by the specification --------

#[test]
fn a_hold_interrupted_by_a_disconnect_never_completes() {
    let mut surface = Surface::watching([(pad(0), InterestKind::Hold)]);
    surface.press(pad(0), 0);
    assert_eq!(surface.drain(), [Gesture::Press]);

    // The surface goes away mid-press.
    surface.recognizer.reset();

    surface.advance_to(HOLD_MS + 100);
    assert!(
        surface.drain().is_empty(),
        "a hold must not fire against a device that is gone"
    );
    assert!(surface.recognizer.next_deadline().is_none());
}

#[test]
fn a_release_arriving_after_a_reset_reports_only_the_release() {
    let mut surface = Surface::watching([(pad(0), InterestKind::Hold)]);
    surface.press(pad(0), 0);
    surface.drain();

    surface.recognizer.reset();
    surface.release(pad(0), 100);

    assert_eq!(
        surface.drain(),
        [Gesture::Release],
        "an orphaned release must not synthesise a tap"
    );
}

#[test]
fn shift_is_forgotten_across_a_reconnect() {
    let mut surface = Surface::bare();
    surface.press(SHIFT, 0);
    assert!(surface.recognizer.shift_held());

    surface.recognizer.reset();
    assert!(!surface.recognizer.shift_held());

    surface.press(pad(0), 100);
    assert!(
        surface.drain().contains(&Gesture::Press),
        "the press is plain, not a shift press"
    );
}

#[test]
fn two_controls_held_at_once_keep_independent_timers() {
    let mut surface =
        Surface::watching([(pad(0), InterestKind::Hold), (pad(1), InterestKind::Hold)]);
    surface.press(pad(0), 0);
    surface.press(pad(1), 200);
    surface.drain();

    surface.advance_to(HOLD_MS);
    let first = surface.drain_events();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].control, pad(0));

    surface.advance_to(200 + HOLD_MS);
    let second = surface.drain_events();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].control, pad(1));
}

#[test]
fn the_next_deadline_is_the_soonest_pending_one() {
    let mut surface =
        Surface::watching([(pad(0), InterestKind::Hold), (pad(1), InterestKind::Hold)]);
    assert!(surface.recognizer.next_deadline().is_none());

    surface.press(pad(0), 0);
    surface.press(pad(1), 100);

    assert_eq!(
        surface.recognizer.next_deadline(),
        Some(surface.at(HOLD_MS))
    );
}

#[test]
fn a_configuration_reload_does_not_strand_a_press_already_in_flight() {
    let mut surface = Surface::watching([(pad(0), InterestKind::Hold)]);
    surface.press(pad(0), 0);
    surface.drain();

    // Configuration changes while the pad is still down; the new one no longer
    // binds a hold to this pad.
    surface.recognizer.set_interest(GestureInterest::none());

    surface.advance_to(HOLD_MS);
    assert_eq!(
        surface.drain(),
        [Gesture::Hold],
        "the press was armed under the old configuration and must still resolve"
    );
}

#[test]
fn a_double_tap_racing_a_reset_does_not_produce_a_stale_tap() {
    let mut surface = Surface::watching([(pad(0), InterestKind::DoubleTap)]);
    surface.press(pad(0), 0);
    surface.release(pad(0), 40);
    surface.drain();

    surface.recognizer.reset();
    surface.advance_to(1_000);
    assert!(surface.drain().is_empty());
}
