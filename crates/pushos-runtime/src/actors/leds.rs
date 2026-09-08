//! Working out what the lights should say.
//!
//! A pad that does something is lit; a pad that does nothing is dark. That is
//! the whole contract, and it is derived from the same binding table the
//! resolver uses, so the surface can never advertise a binding that is not
//! there.

use pushos_config::RuntimeConfig;
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
pub(crate) fn plan_for(config: &RuntimeConfig, context: &SurfaceContext) -> LedPlan {
    let mut plan = LedPlan::new();

    for pad in PadIndex::all() {
        let control = ControlId::Pad(pad);
        plan.set(control, light_for(config, context, control));
    }

    for control in ControlId::all().filter(|control| matches!(control, ControlId::Button(_))) {
        plan.set(control, light_for(config, context, control));
    }

    plan
}

fn light_for(config: &RuntimeConfig, context: &SurfaceContext, control: ControlId) -> LedState {
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

        let plan = plan_for(&config, &SurfaceContext::empty());
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

        let on_home = plan_for(&config, &SurfaceContext::empty().on_page("home"));
        let on_music = plan_for(&config, &SurfaceContext::empty().on_page("music"));

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

        let plan = plan_for(&config, &SurfaceContext::empty());
        assert!(
            plan.states()
                .iter()
                .all(|(_, state)| *state == LedState::OFF)
        );
    }

    #[test]
    fn a_plan_covers_every_pad_and_button_and_nothing_else() {
        let plan = plan_for(&config(PAGES), &SurfaceContext::empty());
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

        let on_home = plan_for(&config, &SurfaceContext::empty().on_page("home"));
        let on_music = plan_for(&config, &SurfaceContext::empty().on_page("music"));

        let changes = on_music.changes_from(&on_home);
        assert_eq!(changes.len(), 1, "only pad 1 differs between the two pages");
        assert_eq!(changes[0].0, pad(1));
    }
}
