//! The visual language of the display.
//!
//! One weight of one typeface is used throughout, so hierarchy comes from size,
//! colour and position. That is a deliberate choice for a 960 by 160 panel read
//! at arm's length while operating hardware.

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
    /// The current selection.
    pub accent: Rgb,
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
        muted: Rgb::new(132, 138, 152),
        accent: Rgb::new(88, 166, 255),
        attention: Rgb::new(255, 176, 0),
        failure: Rgb::new(235, 87, 87),
        sizes: TypeScale {
            headline: 28.0,
            body: 18.0,
            caption: 13.0,
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
