//! Which controls need delayed gesture recognition.
//!
//! A tap can only be confirmed once the double-tap window has passed without a
//! second press. Waiting that long on every control would add perceptible
//! latency to the most common gesture on the surface, so the recogniser only
//! waits for controls that actually have a double-tap binding. Everything else
//! reports its tap the moment the control is released.

use std::collections::HashSet;

use pushos_domain::controls::ControlId;

/// The controls whose gestures need timers.
///
/// Rebuilt whenever configuration is replaced, so it always reflects the
/// bindings currently in force.
#[derive(Clone, Debug, Default)]
pub struct GestureInterest {
    double_tap: HashSet<ControlId>,
    hold: HashSet<ControlId>,
}

impl GestureInterest {
    /// An interest set for a surface with no bindings.
    pub fn none() -> Self {
        Self::default()
    }

    /// Records that a control has a binding needing a double-tap timer.
    pub fn watch_double_tap(&mut self, control: ControlId) {
        self.double_tap.insert(control);
    }

    /// Records that a control has a binding needing a hold timer.
    pub fn watch_hold(&mut self, control: ControlId) {
        self.hold.insert(control);
    }

    /// Whether a tap on this control must wait for a possible second tap.
    pub fn wants_double_tap(&self, control: ControlId) -> bool {
        self.double_tap.contains(&control)
    }

    /// Whether a press on this control should arm a hold timer.
    pub fn wants_hold(&self, control: ControlId) -> bool {
        self.hold.contains(&control)
    }
}

impl FromIterator<(ControlId, InterestKind)> for GestureInterest {
    fn from_iter<T: IntoIterator<Item = (ControlId, InterestKind)>>(iter: T) -> Self {
        let mut interest = Self::none();
        for (control, kind) in iter {
            match kind {
                InterestKind::DoubleTap => interest.watch_double_tap(control),
                InterestKind::Hold => interest.watch_hold(control),
            }
        }
        interest
    }
}

/// The kind of timer a binding requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterestKind {
    /// The control has a `double_tap` binding.
    DoubleTap,
    /// The control has a `hold` or `shift_hold` binding.
    Hold,
}

#[cfg(test)]
mod tests {
    use pushos_domain::controls::{ButtonId, PadIndex};

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    #[test]
    fn an_empty_interest_set_wants_no_timers() {
        let interest = GestureInterest::none();
        assert!(!interest.wants_double_tap(pad(0)));
        assert!(!interest.wants_hold(pad(0)));
    }

    #[test]
    fn interests_are_tracked_per_control_and_per_kind() {
        let interest = GestureInterest::from_iter([
            (pad(3), InterestKind::DoubleTap),
            (ControlId::Button(ButtonId::Play), InterestKind::Hold),
        ]);

        assert!(interest.wants_double_tap(pad(3)));
        assert!(!interest.wants_hold(pad(3)));
        assert!(interest.wants_hold(ControlId::Button(ButtonId::Play)));
        assert!(!interest.wants_double_tap(ControlId::Button(ButtonId::Play)));
    }
}
