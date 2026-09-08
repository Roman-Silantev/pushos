//! The hardware surface port.
//!
//! Implementations translate between MIDI/USB and this interface. Callers of
//! these traits never see a note number.

use async_trait::async_trait;

use crate::color::{LedState, Rgb};
use crate::controls::ControlId;
use crate::input::ControlEvent;

/// Horizontal resolution of the Push 2 display, in pixels.
pub const DISPLAY_WIDTH: usize = 960;
/// Vertical resolution of the Push 2 display, in pixels.
pub const DISPLAY_HEIGHT: usize = 160;

/// One complete display image, as packed 16-bit pixels in the display's format.
///
/// The buffer is allocated once and reused; the renderer writes into it rather
/// than returning a fresh image per frame.
#[derive(Clone, PartialEq, Eq)]
pub struct DisplayFrame {
    pixels: Box<[u16]>,
}

impl DisplayFrame {
    /// Allocates a black frame at the display's native size.
    pub fn blank() -> Self {
        Self {
            pixels: vec![0; DISPLAY_WIDTH * DISPLAY_HEIGHT].into_boxed_slice(),
        }
    }

    /// Borrows the packed pixels, row-major from the top-left.
    pub fn pixels(&self) -> &[u16] {
        &self.pixels
    }

    /// Borrows the packed pixels for writing.
    pub fn pixels_mut(&mut self) -> &mut [u16] {
        &mut self.pixels
    }

    /// Overwrites the whole frame with one colour.
    pub fn fill(&mut self, color: Rgb) {
        self.pixels.fill(color.to_bgr565());
    }

    /// Reads one pixel, or `None` when the coordinates fall outside the frame.
    pub fn pixel(&self, x: usize, y: usize) -> Option<u16> {
        (x < DISPLAY_WIDTH && y < DISPLAY_HEIGHT).then(|| self.pixels[y * DISPLAY_WIDTH + x])
    }
}

impl Default for DisplayFrame {
    fn default() -> Self {
        Self::blank()
    }
}

impl std::fmt::Debug for DisplayFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayFrame")
            .field("width", &DISPLAY_WIDTH)
            .field("height", &DISPLAY_HEIGHT)
            .finish_non_exhaustive()
    }
}

/// Drives the lights and the display.
///
/// Implementations must be cheap enough to call from the render loop: no
/// blocking IO, no allocation per LED.
#[async_trait]
pub trait PushOutput: Send + Sync + std::fmt::Debug {
    /// Sets one control's light.
    async fn set_led(&self, control: ControlId, state: LedState) -> Result<(), PushSurfaceError>;

    /// Sets many lights at once, so the adapter can batch the wire traffic.
    async fn set_leds(&self, states: &[(ControlId, LedState)]) -> Result<(), PushSurfaceError>;

    /// Sends a complete image to the display.
    ///
    /// The hardware blanks itself if no frame arrives for two seconds, so the
    /// render loop must keep calling this even when nothing has changed.
    async fn present(&self, frame: &DisplayFrame) -> Result<(), PushSurfaceError>;

    /// Turns every light off. Used on shutdown and before handing the device back.
    async fn clear(&self) -> Result<(), PushSurfaceError>;
}

/// Delivers normalised input from the surface.
#[async_trait]
pub trait PushInput: Send + std::fmt::Debug {
    /// Waits for the next event.
    ///
    /// Returns `None` once the device is gone and will produce nothing further.
    async fn next_event(&mut self) -> Option<ControlEvent>;
}

/// Why a surface operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PushSurfaceError {
    /// The device is not currently attached.
    #[error("Push 2 is not connected")]
    Disconnected,

    /// The control has no light to drive.
    #[error("control `{control}` has no addressable light")]
    NotIlluminated {
        /// The control that was addressed.
        control: ControlId,
    },

    /// The transport reported a fault.
    #[error("{context}")]
    Transport {
        /// What PushOS was attempting.
        context: String,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PushSurfaceError {
    /// Wraps a transport fault with context.
    pub fn transport(
        context: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Transport {
            context: context.into(),
            source: Box::new(source),
        }
    }

    /// Whether the fault means the device should be treated as gone.
    pub const fn is_disconnect(&self) -> bool {
        matches!(self, Self::Disconnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_frame_is_the_native_display_size_and_is_black() {
        let frame = DisplayFrame::blank();
        assert_eq!(frame.pixels().len(), DISPLAY_WIDTH * DISPLAY_HEIGHT);
        assert!(frame.pixels().iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn filling_writes_the_packed_colour_everywhere() {
        let mut frame = DisplayFrame::blank();
        frame.fill(Rgb::WHITE);
        assert!(frame.pixels().iter().all(|&pixel| pixel == 0xFFFF));
    }

    #[test]
    fn reads_outside_the_frame_return_none_rather_than_wrapping() {
        let frame = DisplayFrame::blank();
        assert!(frame.pixel(0, 0).is_some());
        assert!(frame.pixel(DISPLAY_WIDTH - 1, DISPLAY_HEIGHT - 1).is_some());
        assert!(frame.pixel(DISPLAY_WIDTH, 0).is_none());
        assert!(frame.pixel(0, DISPLAY_HEIGHT).is_none());
    }
}
