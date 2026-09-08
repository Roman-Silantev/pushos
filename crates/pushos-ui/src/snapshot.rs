//! What the display should show.
//!
//! A snapshot is immutable and self-contained. The renderer holds no reference
//! to live state, which is what guarantees it can never block behind a lock, a
//! model, the network or the database.

use pushos_domain::color::LedState;
use pushos_domain::controls::ControlId;
use pushos_domain::ids::PageId;

/// How many labelled slots the display shows, matching the eight encoders above
/// it and the eight buttons below it.
pub const SLOT_COUNT: usize = 8;

/// Everything the display needs for one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiSnapshot {
    /// The page in effect.
    pub page: PageView,
    /// The workspace in effect, when one is selected.
    pub workspace: Option<String>,
    /// Whether the surface is attached.
    pub connected: bool,
    /// The eight labelled columns.
    pub slots: [Option<Slot>; SLOT_COUNT],
    /// The line at the foot of the display.
    pub footer: Option<String>,
    /// A transient message that takes over the footer.
    pub notice: Option<Notice>,
    /// A panel that takes over the whole display.
    pub overlay: Option<Overlay>,
}

impl UiSnapshot {
    /// A snapshot for a surface that is not attached.
    pub fn disconnected() -> Self {
        Self {
            page: PageView::default(),
            connected: false,
            footer: Some("waiting for Push 2".to_owned()),
            ..Self::default()
        }
    }

    /// Whether anything visible differs from another snapshot.
    ///
    /// The renderer uses this to skip redrawing a static screen, which is what
    /// keeps an idle PushOS near zero processor use.
    pub fn differs_from(&self, other: &Self) -> bool {
        self != other
    }
}

/// The page shown in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageView {
    /// The page's identity.
    pub id: Option<PageId>,
    /// The page's display name.
    pub name: String,
    /// Position in the configured page order, one-based.
    pub position: Option<(usize, usize)>,
}

impl PageView {
    /// Builds a page view.
    pub fn new(id: impl Into<PageId>, name: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            name: name.into(),
            position: None,
        }
    }

    /// Records where the page sits in the configured order.
    #[must_use]
    pub const fn at(mut self, index: usize, total: usize) -> Self {
        self.position = Some((index, total));
        self
    }
}

/// One labelled column of the display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The heading, usually what the control does.
    pub label: String,
    /// The secondary line, usually the current value or state.
    pub value: Option<String>,
    /// How the slot should read at a glance.
    pub tone: Tone,
    /// Whether this slot is the current selection.
    pub selected: bool,
}

impl Slot {
    /// Builds a slot with a label only.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: None,
            tone: Tone::Normal,
            selected: false,
        }
    }

    /// Adds the secondary line.
    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets how the slot reads.
    #[must_use]
    pub const fn with_tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Marks the slot as the current selection.
    #[must_use]
    pub const fn selected(mut self) -> Self {
        self.selected = true;
        self
    }
}

/// How a piece of the display should read at a glance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// Nothing special.
    #[default]
    Normal,
    /// Less important than its neighbours.
    Muted,
    /// Something is happening.
    Active,
    /// Something needs the operator.
    Attention,
    /// Something failed.
    Failure,
}

/// A transient message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The message.
    pub text: String,
    /// How it should read.
    pub tone: Tone,
}

impl Notice {
    /// Builds a notice.
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// A panel that temporarily takes over the display.
///
/// Not `Eq`, because progress is a fraction. Comparison is exact and that is
/// what the dirty check wants: a progress value that has not been recomputed
/// compares equal and does not force a redraw.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    /// A short label above the headline, such as the source of the information.
    pub kind: String,
    /// The headline.
    pub title: String,
    /// The supporting line.
    pub detail: Option<String>,
    /// Progress through something, from zero to one.
    pub progress: Option<f32>,
    /// The choices offered, if this overlay is asking something.
    pub choices: Vec<String>,
}

impl Overlay {
    /// Builds an overlay.
    pub fn new(kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            detail: None,
            progress: None,
            choices: Vec::new(),
        }
    }

    /// Adds the supporting line.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Adds a progress bar.
    #[must_use]
    pub fn with_progress(mut self, fraction: f32) -> Self {
        self.progress = Some(fraction.clamp(0.0, 1.0));
        self
    }

    /// Offers the operator a set of choices.
    #[must_use]
    pub fn asking(mut self, choices: impl IntoIterator<Item = String>) -> Self {
        self.choices = choices.into_iter().collect();
        self
    }
}

/// The lights the surface should show, alongside the display.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedPlan {
    states: Vec<(ControlId, LedState)>,
}

impl LedPlan {
    /// Builds an empty plan.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets one control's light, replacing any earlier entry for it.
    pub fn set(&mut self, control: ControlId, state: LedState) {
        match self
            .states
            .iter_mut()
            .find(|(candidate, _)| *candidate == control)
        {
            Some(entry) => entry.1 = state,
            None => self.states.push((control, state)),
        }
    }

    /// The lights in the plan.
    pub fn states(&self) -> &[(ControlId, LedState)] {
        &self.states
    }

    /// The entries that differ from `previous`, plus any control `previous` lit
    /// that this plan does not.
    ///
    /// Sending only differences is what keeps light updates off the critical
    /// path when a page changes one pad out of sixty-four.
    pub fn changes_from(&self, previous: &Self) -> Vec<(ControlId, LedState)> {
        let mut changes: Vec<_> = self
            .states
            .iter()
            .filter(|(control, state)| {
                previous
                    .states
                    .iter()
                    .find(|(candidate, _)| candidate == control)
                    .is_none_or(|(_, was)| was != state)
            })
            .copied()
            .collect();

        changes.extend(
            previous
                .states
                .iter()
                .filter(|(control, _)| {
                    !self
                        .states
                        .iter()
                        .any(|(candidate, _)| candidate == control)
                })
                .map(|(control, _)| (*control, LedState::OFF)),
        );

        changes
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::color::Rgb;
    use pushos_domain::controls::PadIndex;

    use super::*;

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    #[test]
    fn an_identical_snapshot_is_not_a_reason_to_redraw() {
        let snapshot = UiSnapshot {
            page: PageView::new("home", "Home"),
            ..UiSnapshot::default()
        };
        assert!(!snapshot.differs_from(&snapshot.clone()));
    }

    #[test]
    fn any_visible_change_is_a_reason_to_redraw() {
        let base = UiSnapshot {
            page: PageView::new("home", "Home"),
            ..UiSnapshot::default()
        };
        let mut changed = base.clone();
        changed.footer = Some("running tests".to_owned());
        assert!(changed.differs_from(&base));
    }

    #[test]
    fn setting_the_same_light_twice_replaces_rather_than_duplicates() {
        let mut plan = LedPlan::new();
        plan.set(pad(0), LedState::solid(Rgb::WHITE));
        plan.set(pad(0), LedState::solid(Rgb::BLACK));

        assert_eq!(plan.states().len(), 1);
        assert_eq!(plan.states()[0].1, LedState::solid(Rgb::BLACK));
    }

    #[test]
    fn only_changed_lights_are_sent() {
        let mut previous = LedPlan::new();
        previous.set(pad(0), LedState::solid(Rgb::WHITE));
        previous.set(pad(1), LedState::solid(Rgb::WHITE));

        let mut next = LedPlan::new();
        next.set(pad(0), LedState::solid(Rgb::WHITE));
        next.set(pad(1), LedState::solid(Rgb::new(255, 0, 0)));

        let changes = next.changes_from(&previous);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, pad(1));
    }

    #[test]
    fn a_light_that_is_no_longer_in_the_plan_is_turned_off() {
        let mut previous = LedPlan::new();
        previous.set(pad(0), LedState::solid(Rgb::WHITE));

        let changes = LedPlan::new().changes_from(&previous);
        assert_eq!(changes, [(pad(0), LedState::OFF)]);
    }

    #[test]
    fn an_unchanged_plan_sends_nothing() {
        let mut plan = LedPlan::new();
        plan.set(pad(0), LedState::solid(Rgb::WHITE));
        assert!(plan.changes_from(&plan.clone()).is_empty());
    }

    #[test]
    fn progress_is_clamped_to_a_fraction() {
        assert_eq!(
            Overlay::new("test", "t").with_progress(5.0).progress,
            Some(1.0)
        );
        assert_eq!(
            Overlay::new("test", "t").with_progress(-5.0).progress,
            Some(0.0)
        );
    }
}
