//! Colour vocabulary for the surface and the display.

use serde::{Deserialize, Serialize};

/// A 24-bit colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb {
    /// Builds a colour from its channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Scales every channel by `factor`, clamped to `0.0..=1.0`.
    #[must_use]
    pub fn dimmed(self, factor: f32) -> Self {
        let factor = factor.clamp(0.0, 1.0);
        let scale = |channel: u8| {
            // `factor` is clamped to `0.0..=1.0` and `channel` to `0..=255`, so the
            // rounded product is always a whole number inside `u8`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let scaled = (f32::from(channel) * factor).round() as u8;
            scaled
        };
        Self::new(scale(self.r), scale(self.g), scale(self.b))
    }

    /// Packs the colour into the display's 16-bit format.
    ///
    /// The Push 2 display stores blue in bits 15..11, green in 10..5 and red in
    /// 4..0, so red and blue lose their three low bits and green loses two.
    pub const fn to_bgr565(self) -> u16 {
        let b = ((self.b as u16) >> 3) << 11;
        let g = ((self.g as u16) >> 2) << 5;
        let r = (self.r as u16) >> 3;
        b | g | r
    }

    /// Pure black.
    pub const BLACK: Self = Self::new(0, 0, 0);
    /// Pure white.
    pub const WHITE: Self = Self::new(255, 255, 255);
}

/// What a control's light is currently saying.
///
/// Colour is never the only signal: pairing a status with an [`LedAnimation`]
/// keeps the surface readable for colour-blind operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusColor {
    /// Configured but not doing anything.
    Idle,
    /// Not configured on this page.
    Unassigned,
    /// Currently executing.
    Working,
    /// Blocked on the operator.
    Waiting,
    /// Finished successfully.
    Complete,
    /// Finished unsuccessfully.
    Failed,
    /// Part of a workflow or a message between components.
    Workflow,
    /// The current selection.
    Selected,
}

impl StatusColor {
    /// The default palette entry for the status.
    pub const fn default_rgb(self) -> Rgb {
        match self {
            Self::Idle => Rgb::new(24, 24, 28),
            Self::Unassigned => Rgb::BLACK,
            Self::Working => Rgb::new(0, 110, 255),
            Self::Waiting => Rgb::new(255, 176, 0),
            Self::Complete => Rgb::new(0, 200, 90),
            Self::Failed => Rgb::new(230, 40, 40),
            Self::Workflow => Rgb::new(160, 70, 235),
            Self::Selected => Rgb::new(240, 240, 245),
        }
    }

    /// The animation that carries the same meaning without relying on colour.
    pub const fn default_animation(self) -> LedAnimation {
        match self {
            Self::Working => LedAnimation::Pulse(AnimationRate::Quarter),
            Self::Waiting => LedAnimation::Blink(AnimationRate::Eighth),
            Self::Failed => LedAnimation::Blink(AnimationRate::Sixteenth),
            _ => LedAnimation::Solid,
        }
    }
}

/// How a light behaves over time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedAnimation {
    /// Constant brightness.
    Solid,
    /// Fades between the colour and the previous colour.
    Pulse(AnimationRate),
    /// Switches sharply between the colour and the previous colour.
    Blink(AnimationRate),
}

/// Animation speed, expressed in musical divisions because the hardware
/// synchronises every animated light to a shared clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationRate {
    /// Twenty-fourth notes.
    TwentyFourth,
    /// Sixteenth notes.
    Sixteenth,
    /// Eighth notes.
    Eighth,
    /// Quarter notes.
    Quarter,
    /// Half notes.
    Half,
}

/// The complete instruction for one control's light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LedState {
    /// The colour to display.
    pub color: Rgb,
    /// How it behaves over time.
    pub animation: LedAnimation,
}

impl LedState {
    /// Builds a light instruction.
    pub const fn new(color: Rgb, animation: LedAnimation) -> Self {
        Self { color, animation }
    }

    /// A steady light of the given colour.
    pub const fn solid(color: Rgb) -> Self {
        Self::new(color, LedAnimation::Solid)
    }

    /// The light for an unassigned control.
    pub const OFF: Self = Self::new(Rgb::BLACK, LedAnimation::Solid);

    /// Derives a light from a semantic status.
    pub const fn from_status(status: StatusColor) -> Self {
        Self::new(status.default_rgb(), status.default_animation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_pack_to_the_display_extremes() {
        assert_eq!(Rgb::BLACK.to_bgr565(), 0x0000);
        assert_eq!(Rgb::WHITE.to_bgr565(), 0xFFFF);
    }

    #[test]
    fn packing_places_each_channel_in_its_documented_bits() {
        assert_eq!(Rgb::new(255, 0, 0).to_bgr565(), 0b0000_0000_0001_1111);
        assert_eq!(Rgb::new(0, 255, 0).to_bgr565(), 0b0000_0111_1110_0000);
        assert_eq!(Rgb::new(0, 0, 255).to_bgr565(), 0b1111_1000_0000_0000);
    }

    #[test]
    fn dimming_is_clamped_at_both_ends() {
        assert_eq!(Rgb::WHITE.dimmed(0.0), Rgb::BLACK);
        assert_eq!(Rgb::WHITE.dimmed(1.0), Rgb::WHITE);
        assert_eq!(Rgb::WHITE.dimmed(2.0), Rgb::WHITE);
        assert_eq!(Rgb::WHITE.dimmed(-1.0), Rgb::BLACK);
    }

    #[test]
    fn attention_states_are_distinguishable_without_colour() {
        for status in [
            StatusColor::Working,
            StatusColor::Waiting,
            StatusColor::Failed,
        ] {
            assert_ne!(
                status.default_animation(),
                LedAnimation::Solid,
                "{status:?} must be readable without relying on hue"
            );
        }
    }
}
