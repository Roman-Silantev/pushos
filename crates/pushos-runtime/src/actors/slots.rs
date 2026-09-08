//! Labelling the eight columns of the display.
//!
//! The columns line up with the eight buttons directly above the screen and the
//! eight directly below it, so each column describes the two controls it sits
//! between. Labels come from the same binding table the resolver uses, which is
//! what stops the display advertising something that will not happen.

use pushos_config::RuntimeConfig;
use pushos_domain::binding::{Binding, BindingKey};
use pushos_domain::context::SurfaceContext;
use pushos_domain::controls::{ButtonId, ControlId};
use pushos_domain::gesture::Gesture;
use pushos_ui::{SLOT_COUNT, Slot, Tone};

/// The buttons along the top edge of the display, left to right.
const UPPER: [ButtonId; SLOT_COUNT] = [
    ButtonId::Upper1,
    ButtonId::Upper2,
    ButtonId::Upper3,
    ButtonId::Upper4,
    ButtonId::Upper5,
    ButtonId::Upper6,
    ButtonId::Upper7,
    ButtonId::Upper8,
];

/// The buttons along the bottom edge of the display, left to right.
const LOWER: [ButtonId; SLOT_COUNT] = [
    ButtonId::Lower1,
    ButtonId::Lower2,
    ButtonId::Lower3,
    ButtonId::Lower4,
    ButtonId::Lower5,
    ButtonId::Lower6,
    ButtonId::Lower7,
    ButtonId::Lower8,
];

/// The gestures a column's label is taken from, in order of preference.
const LABELLED: [Gesture; 3] = [Gesture::Tap, Gesture::Press, Gesture::Hold];

/// Builds the eight column labels for the current surface.
pub(crate) fn labels_for(
    config: &RuntimeConfig,
    context: &SurfaceContext,
) -> [Option<Slot>; SLOT_COUNT] {
    std::array::from_fn(|index| {
        let above = label(config, context, ControlId::Button(UPPER[index]));
        let below = label(config, context, ControlId::Button(LOWER[index]));

        match (above, below) {
            (None, None) => None,
            (Some(above), below) => {
                let slot = Slot::new(above).with_tone(Tone::Normal);
                Some(match below {
                    Some(below) => slot.with_value(below),
                    None => slot,
                })
            }
            // Nothing above, something below: the column still describes its
            // one binding rather than going blank.
            (None, Some(below)) => Some(Slot::new(below).with_tone(Tone::Muted)),
        }
    })
}

/// The label a control advertises, if it has one.
///
/// A binding without a label is deliberately not shown: an action selector is
/// not a caption, and half a caption is worse than none on a small panel.
fn label(config: &RuntimeConfig, context: &SurfaceContext, control: ControlId) -> Option<String> {
    LABELLED.iter().find_map(|gesture| {
        config
            .bindings
            .candidates(BindingKey::new(control, *gesture))
            .iter()
            .find(|binding| binding.scope.applies_to(context))
            .and_then(|binding: &Binding| binding.label.clone())
    })
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile = toml::from_str(text).expect("well-formed");
        RuntimeConfig::build(&parsed).expect("valid")
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
    fn a_column_shows_the_button_above_it_and_the_one_below() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "button.upper_1"
            gesture = "press"
            action = "page.next"
            label = "Next page"

            [[bindings]]
            control = "button.lower_1"
            gesture = "press"
            action = "page.previous"
            label = "Previous"
            "#
        ));

        let slots = labels_for(&config, &SurfaceContext::empty());
        let first = slots[0].as_ref().expect("the first column is labelled");
        assert_eq!(first.label, "Next page");
        assert_eq!(first.value.as_deref(), Some("Previous"));
        assert!(slots[1].is_none(), "an unbound column stays empty");
    }

    #[test]
    fn a_binding_without_a_label_is_not_advertised() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "button.upper_1"
            gesture = "press"
            action = "page.next"
            "#
        ));

        assert!(labels_for(&config, &SurfaceContext::empty())[0].is_none());
    }

    #[test]
    fn labels_follow_the_page_the_operator_is_on() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "button.upper_1"
            gesture = "press"
            page = "music"
            action = "media.play_pause"
            label = "Play"
            "#
        ));

        assert!(labels_for(&config, &SurfaceContext::empty().on_page("home"))[0].is_none());
        assert_eq!(
            labels_for(&config, &SurfaceContext::empty().on_page("music"))[0]
                .as_ref()
                .map(|slot| slot.label.as_str()),
            Some("Play")
        );
    }

    #[test]
    fn a_column_with_only_a_lower_binding_still_describes_it() {
        let config = config(&format!(
            r#"{PAGES}
            [[bindings]]
            control = "button.lower_3"
            gesture = "press"
            action = "page.home"
            label = "Home"
            "#
        ));

        let slot = labels_for(&config, &SurfaceContext::empty())[2]
            .clone()
            .expect("the third column is labelled");
        assert_eq!(slot.label, "Home");
        assert_eq!(slot.tone, Tone::Muted);
    }

    #[test]
    fn every_column_maps_to_a_distinct_pair_of_buttons() {
        let mut buttons: Vec<_> = UPPER.iter().chain(LOWER.iter()).collect();
        buttons.sort_unstable();
        let total = buttons.len();
        buttons.dedup();
        assert_eq!(total, buttons.len());
        assert_eq!(total, SLOT_COUNT * 2);
    }
}
