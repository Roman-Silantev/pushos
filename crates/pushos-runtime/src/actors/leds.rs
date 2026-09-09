//! Working out what the lights should say.
//!
//! A pad that does something is lit; a pad that does nothing is dark. That is
//! the whole contract, and it is derived from the same binding table the
//! resolver uses, so the surface can never advertise a binding that is not
//! there.
//!
//! A pad that stands for something with a state of its own shows that state
//! instead. Eight pads for eight coding sessions, all the same colour, tell an
//! operator nothing they did not already know; the one that is stuck on a
//! question needs to be the one that looks different.

use pushos_config::RuntimeConfig;
use pushos_domain::attached::{Activity, Attached, AttachedTarget};
use pushos_domain::binding::BindingKey;
use pushos_domain::color::{LedState, StatusColor};
use pushos_domain::context::SurfaceContext;
use pushos_domain::controls::{ControlId, PadIndex};
use pushos_domain::gesture::Gesture;
use pushos_ui::LedPlan;

/// The gestures a control can carry that make it worth lighting.
///
/// A control bound only to `release` is not advertised: the operator has
/// nothing to aim at until they have already pressed it.
const ADVERTISED: [Gesture; 5] = [
    Gesture::Tap,
    Gesture::DoubleTap,
    Gesture::Hold,
    Gesture::Press,
    Gesture::ShiftHold,
];

/// Builds the light plan for the current surface.
pub(crate) fn plan_showing(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    sessions: &[Attached],
    from: usize,
) -> LedPlan {
    let mut plan = LedPlan::new();

    for pad in PadIndex::all() {
        let control = ControlId::Pad(pad);
        plan.set(control, light_for(config, context, control, sessions, from));
    }

    for control in ControlId::all().filter(|control| matches!(control, ControlId::Button(_))) {
        plan.set(control, light_for(config, context, control, sessions, from));
    }

    plan
}

/// The namespace whose bindings stand for a session rather than an instruction.
const SESSIONS: &str = "session";

/// What a control stands for, when it stands for a session.
///
/// Read from the binding itself rather than configured twice: a pad bound to
/// `session.show` with a target already says which session it is, and asking
/// the operator to say so again would be a second place to get it wrong.
fn session_on(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    control: ControlId,
    sessions: &[Attached],
    from: usize,
) -> Option<Activity> {
    let target: AttachedTarget = ADVERTISED
        .iter()
        .flat_map(|gesture| {
            config
                .bindings
                .candidates(BindingKey::new(control, *gesture))
        })
        .find(|binding| {
            binding.scope.applies_to(context)
                && binding.action.selector.provider.as_str() == SESSIONS
        })
        .and_then(|binding| binding.action.params.text("target"))
        .and_then(|written| written.parse().ok())?;

    // A slot is a position in the bank being shown, so it is resolved the same
    // way the provider resolves it. Anything else answers for itself.
    if let AttachedTarget::Slot(at) = target {
        return sessions
            .get(from + usize::from(at) - 1)
            .map(|session| session.activity);
    }

    sessions
        .iter()
        .find(|session| target.matches(session))
        .map(|session| session.activity)
}

fn light_for(
    config: &RuntimeConfig,
    context: &SurfaceContext,
    control: ControlId,
    sessions: &[Attached],
    from: usize,
) -> LedState {
    // What a session is doing outranks the fact that a pad is bound: the pad
    // being bound is what the operator already knows.
    if let Some(activity) = session_on(config, context, control, sessions, from) {
        return LedState::from_status(activity.status_color());
    }

    let bound = ADVERTISED.iter().any(|gesture| {
        config
            .bindings
            .candidates(BindingKey::new(control, *gesture))
            .iter()
            .any(|binding| binding.scope.applies_to(context))
    });

    if bound {
        LedState::from_status(StatusColor::Idle)
    } else {
        LedState::OFF
    }
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile =
            toml::from_str(text).expect("the test configuration is well-formed");
        RuntimeConfig::build(&parsed).expect("the test configuration is valid")
    }

    fn pad(index: u8) -> ControlId {
        ControlId::Pad(PadIndex::new(index).expect("test pad index is in range"))
    }

    const PAGES: &str = r#"
        [[pages]]
        id = "home"
        name = "Home"

        [[pages]]
        id = "music"
        name = "Music"
    "#;

    #[test]
    fn a_bound_pad_is_lit_and_an_unbound_one_is_dark() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            action = "page.next"
            "#
        ));

        let plan = plan_showing(&config, &SurfaceContext::empty(), &[], 0);
        let lit: Vec<_> = plan
            .states()
            .iter()
            .filter(|(_, state)| *state != LedState::OFF)
            .map(|(control, _)| *control)
            .collect();

        assert_eq!(lit, [pad(0)]);
    }

    #[test]
    fn a_pad_bound_on_another_page_is_dark_here() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            page = "music"
            action = "page.next"
            "#
        ));

        let on_home = plan_showing(&config, &SurfaceContext::empty().on_page("home"), &[], 0);
        let on_music = plan_showing(&config, &SurfaceContext::empty().on_page("music"), &[], 0);

        assert_eq!(
            on_home
                .states()
                .iter()
                .find(|(c, _)| *c == pad(0))
                .map(|(_, s)| *s),
            Some(LedState::OFF)
        );
        assert_ne!(
            on_music
                .states()
                .iter()
                .find(|(c, _)| *c == pad(0))
                .map(|(_, s)| *s),
            Some(LedState::OFF)
        );
    }

    #[test]
    fn a_control_bound_only_to_release_is_not_advertised() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.5"
            gesture = "release"
            action = "page.next"
            "#
        ));

        let plan = plan_showing(&config, &SurfaceContext::empty(), &[], 0);
        assert!(
            plan.states()
                .iter()
                .all(|(_, state)| *state == LedState::OFF)
        );
    }

    #[test]
    fn a_plan_covers_every_pad_and_button_and_nothing_else() {
        let plan = plan_showing(&config(PAGES), &SurfaceContext::empty(), &[], 0);
        assert!(
            plan.states()
                .iter()
                .all(|(control, _)| control.is_illuminated())
        );
        assert_eq!(
            plan.states().len(),
            64 + pushos_domain::controls::ALL_BUTTONS.len()
        );
    }

    #[test]
    fn moving_page_only_changes_the_lights_that_differ() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "pad.0"
            gesture = "tap"
            action = "page.next"

            [[bindings]]
            control = "pad.1"
            gesture = "tap"
            page = "music"
            action = "page.previous"
            "#
        ));

        let on_home = plan_showing(&config, &SurfaceContext::empty().on_page("home"), &[], 0);
        let on_music = plan_showing(&config, &SurfaceContext::empty().on_page("music"), &[], 0);

        let changes = on_music.changes_from(&on_home);
        assert_eq!(changes.len(), 1, "only pad 1 differs between the two pages");
        assert_eq!(changes[0].0, pad(1));
    }
}
