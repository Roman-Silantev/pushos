//! The visual language of the display.
//!
//! One weight of one typeface is used throughout, so hierarchy comes from size,
//! colour and position. That is a deliberate choice for a 960 by 160 panel read
//! at arm's length while operating hardware.
//!
//! The type is small on purpose. Eight columns across 960 pixels is 120 pixels
//! each, and a session called "Review project tasks before integrations" has to
//! be recognisable in that. Ableton's own Push 2 screens do the same thing: a
//! small dim caption naming the thing, a larger line for its value, and a bar
//! underneath carrying the state at a glance without being read at all.

use pushos_domain::color::Rgb;

/// Colours and metrics for the display.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The page background.
    pub background: Rgb,
    /// The status bar background.
    pub chrome: Rgb,
    /// Divider lines between slots.
    pub divider: Rgb,
    /// Primary text.
    pub text: Rgb,
    /// Secondary text.
    pub muted: Rgb,
    /// The current selection, and anything PushOS itself is saying.
    pub accent: Rgb,
    /// Something is working.
    pub working: Rgb,
    /// Something needs the operator.
    pub attention: Rgb,
    /// Something failed.
    pub failure: Rgb,
    /// Type sizes, largest first.
    pub sizes: TypeScale,
}

/// The three type sizes the display uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeScale {
    /// Overlay headlines and the page name.
    pub headline: f32,
    /// Slot labels and the notice line.
    pub body: f32,
    /// Status bar and secondary detail.
    pub caption: f32,
    /// Column captions and anything that is read rather than glanced at.
    pub label: f32,
}

impl Theme {
    /// The default dark theme.
    ///
    /// Dark by default because the panel sits under studio lighting and beside
    /// unlit pads; a bright screen would be the only glare in the room.
    pub const DARK: Self = Self {
        background: Rgb::new(10, 11, 14),
        chrome: Rgb::new(22, 24, 30),
        divider: Rgb::new(38, 41, 50),
        text: Rgb::new(238, 240, 245),
        muted: Rgb::new(126, 132, 146),
        // The same orange the mascot is drawn in, so PushOS speaks with one
        // voice: what the surface is doing and what it is called are the same
        // colour, and everything else on the panel belongs to the operator's
        // work rather than to PushOS.
        accent: crate::mascot::MASCOT,
        working: Rgb::new(88, 166, 255),
        attention: Rgb::new(255, 176, 0),
        failure: Rgb::new(235, 87, 87),
        sizes: TypeScale {
            headline: 22.0,
            body: 15.0,
            caption: 12.0,
            label: 10.0,
        },
    };
}

impl Default for Theme {
    fn default() -> Self {
        Self::DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_scale_is_strictly_decreasing() {
        let sizes = Theme::DARK.sizes;
        assert!(sizes.headline > sizes.body);
        assert!(sizes.body > sizes.caption);
        assert!(sizes.caption > sizes.label);
    }

    #[test]
    fn three_lines_of_a_column_fit_between_the_bars() {
        // Eight columns of a hundred and twenty pixels, and each one has to
        // hold a caption, a name and a bar. If the type grew past this the
        // bottom line would be drawn off the panel.
        let theme = Theme::DARK;
        let sizes = theme.sizes;
        let column = 160.0 - 22.0 - 26.0;
        let needed = sizes.label + sizes.body + sizes.caption + 18.0;
        assert!(needed < column, "{needed} does not fit in {column}");
    }

    #[test]
    fn what_pushos_says_is_the_colour_pushos_is() {
        // One voice: the mascot, the selection and the page name are the same
        // orange, and everything else on the panel belongs to the work.
        assert_eq!(Theme::DARK.accent, crate::mascot::MASCOT);
        assert_ne!(Theme::DARK.accent, Theme::DARK.working);
    }

    #[test]
    fn text_is_readable_against_the_background() {
        // A crude luminance gap, enough to catch a theme that inverts by accident.
        let luma = |c: Rgb| u16::from(c.r) + u16::from(c.g) * 2 + u16::from(c.b);
        let theme = Theme::DARK;
        assert!(luma(theme.text) > luma(theme.background) * 4);
        assert!(luma(theme.muted) > luma(theme.background));
    }
}
