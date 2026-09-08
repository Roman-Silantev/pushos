//! A Push 2 that exists only in memory.
//!
//! FakePush satisfies exactly the same ports as the real adapter, so the core
//! can be exercised, and the whole test suite can run, with no hardware
//! attached. It is a development and testing tool: nothing in the runtime
//! should reach for it as a production surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use pushos_domain::color::LedState;
use pushos_domain::controls::{ControlId, PadIndex};
use pushos_domain::input::{ControlEvent, InputPhase};
use pushos_domain::ports::{DisplayFrame, PushInput, PushOutput, PushSurfaceError};
use tokio::sync::{Mutex, mpsc};

/// How many injected events may queue before a test is producing faster than
/// the code under test consumes.
const INPUT_CAPACITY: usize = 256;

/// The observable state of a fake surface.
#[derive(Debug)]
pub struct SurfaceState {
    leds: HashMap<ControlId, LedState>,
    frames: Vec<DisplayFrame>,
    connected: bool,
    /// Held here rather than on the handle so that unplugging ends input for
    /// every clone at once, the way pulling a cable does.
    input: Option<mpsc::Sender<ControlEvent>>,
}

impl SurfaceState {
    /// The light currently set on a control, if any has been set.
    pub fn led(&self, control: ControlId) -> Option<LedState> {
        self.leds.get(&control).copied()
    }

    /// How many lights have been set.
    pub fn lit_count(&self) -> usize {
        self.leds
            .values()
            .filter(|state| **state != LedState::OFF)
            .count()
    }

    /// How many frames have been presented.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// The most recently presented frame.
    pub fn last_frame(&self) -> Option<&DisplayFrame> {
        self.frames.last()
    }
}

/// An in-memory Push 2.
#[derive(Debug, Clone)]
pub struct FakePush {
    state: Arc<Mutex<SurfaceState>>,
}

impl FakePush {
    /// Builds a fake surface and the input half that feeds it.
    pub fn new() -> (Self, FakePushInput) {
        let (input, events) = mpsc::channel(INPUT_CAPACITY);
        let state = Arc::new(Mutex::new(SurfaceState {
            leds: HashMap::new(),
            frames: Vec::new(),
            connected: true,
            input: Some(input),
        }));
        (Self { state }, FakePushInput { events })
    }

    /// Reads the surface state.
    pub async fn state(&self) -> tokio::sync::MutexGuard<'_, SurfaceState> {
        self.state.lock().await
    }

    /// Injects one normalised input event, as the hardware would produce it.
    pub async fn inject(&self, event: ControlEvent) {
        let sender = self.state.lock().await.input.clone();
        // A closed receiver means the test dropped the consumer, and an absent
        // sender means the surface was unplugged. Neither is a surface fault.
        if let Some(sender) = sender {
            let _ = sender.send(event).await;
        }
    }

    /// Injects a pad strike.
    pub async fn press_pad(&self, pad: PadIndex, at: Instant) {
        self.inject(ControlEvent::new(
            ControlId::Pad(pad),
            InputPhase::Down { velocity: 100 },
            at,
        ))
        .await;
    }

    /// Injects a pad release.
    pub async fn release_pad(&self, pad: PadIndex, at: Instant) {
        self.inject(ControlEvent::new(ControlId::Pad(pad), InputPhase::Up, at))
            .await;
    }

    /// Simulates the cable being pulled out.
    ///
    /// Input ends and subsequent output calls fail with
    /// [`PushSurfaceError::Disconnected`], which is what the real adapter
    /// reports. Both halves go at once, because a cable does not come out
    /// halfway.
    pub async fn disconnect(&self) {
        let mut state = self.state.lock().await;
        state.connected = false;
        state.input = None;
    }

    /// Simulates the panel becoming reachable again.
    ///
    /// Output works once more and the lights come back blank, so the renderer
    /// must redraw. Input does not resume: the real adapter builds a fresh
    /// input stream when it reconnects, and so should a test.
    pub async fn reconnect(&self) {
        let mut state = self.state.lock().await;
        state.connected = true;
        state.leds.clear();
    }

    async fn require_connected(&self) -> Result<(), PushSurfaceError> {
        if self.state.lock().await.connected {
            Ok(())
        } else {
            Err(PushSurfaceError::Disconnected)
        }
    }
}

#[async_trait]
impl PushOutput for FakePush {
    async fn set_led(&self, control: ControlId, state: LedState) -> Result<(), PushSurfaceError> {
        self.require_connected().await?;
        if !control.is_illuminated() {
            return Err(PushSurfaceError::NotIlluminated { control });
        }
        self.state.lock().await.leds.insert(control, state);
        Ok(())
    }

    async fn set_leds(&self, states: &[(ControlId, LedState)]) -> Result<(), PushSurfaceError> {
        self.require_connected().await?;
        // Validate the whole batch first, so a rejected control cannot leave the
        // surface half-updated. The real adapter makes the same guarantee.
        if let Some(&(control, _)) = states.iter().find(|(control, _)| !control.is_illuminated()) {
            return Err(PushSurfaceError::NotIlluminated { control });
        }

        let mut surface = self.state.lock().await;
        for &(control, led) in states {
            surface.leds.insert(control, led);
        }
        Ok(())
    }

    async fn present(&self, frame: &DisplayFrame) -> Result<(), PushSurfaceError> {
        self.require_connected().await?;
        self.state.lock().await.frames.push(frame.clone());
        Ok(())
    }

    async fn clear(&self) -> Result<(), PushSurfaceError> {
        self.require_connected().await?;
        self.state.lock().await.leds.clear();
        Ok(())
    }
}

/// The input half of a fake surface.
#[derive(Debug)]
pub struct FakePushInput {
    events: mpsc::Receiver<ControlEvent>,
}

#[async_trait]
impl PushInput for FakePushInput {
    async fn next_event(&mut self) -> Option<ControlEvent> {
        self.events.recv().await
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::color::Rgb;
    use pushos_domain::controls::EncoderId;

    use super::*;

    fn pad(index: u8) -> PadIndex {
        PadIndex::new(index).expect("test pad index is in range")
    }

    #[tokio::test]
    async fn lights_are_recorded_and_readable() {
        let (surface, _input) = FakePush::new();
        let lit = LedState::solid(Rgb::WHITE);

        surface
            .set_led(ControlId::Pad(pad(0)), lit)
            .await
            .expect("pads have lights");
        assert_eq!(surface.state().await.led(ControlId::Pad(pad(0))), Some(lit));
        assert_eq!(surface.state().await.lit_count(), 1);
    }

    #[tokio::test]
    async fn a_batch_containing_an_unlit_control_changes_nothing() {
        let (surface, _input) = FakePush::new();
        let lit = LedState::solid(Rgb::WHITE);

        let error = surface
            .set_leds(&[
                (ControlId::Pad(pad(0)), lit),
                (ControlId::Encoder(EncoderId::Master), lit),
            ])
            .await
            .expect_err("encoders have no light");

        assert!(matches!(error, PushSurfaceError::NotIlluminated { .. }));
        assert_eq!(
            surface.state().await.lit_count(),
            0,
            "the batch must be all or nothing"
        );
    }

    #[tokio::test]
    async fn unplugging_ends_input_as_well_as_output() {
        let (surface, mut input) = FakePush::new();
        surface.press_pad(pad(0), Instant::now()).await;
        surface.disconnect().await;

        // The event sent before the unplug still arrives; nothing after it does.
        assert!(input.next_event().await.is_some());
        assert!(
            input.next_event().await.is_none(),
            "a surface that is gone must stop producing input"
        );
    }

    #[tokio::test]
    async fn output_fails_while_disconnected_and_works_again_after_reconnect() {
        let (surface, _input) = FakePush::new();
        surface.disconnect().await;

        let error = surface
            .set_led(ControlId::Pad(pad(0)), LedState::solid(Rgb::WHITE))
            .await
            .expect_err("the surface is gone");
        assert!(error.is_disconnect());
        assert!(surface.present(&DisplayFrame::blank()).await.is_err());

        surface.reconnect().await;
        surface
            .set_led(ControlId::Pad(pad(0)), LedState::solid(Rgb::WHITE))
            .await
            .expect("the surface is back");
    }

    #[tokio::test]
    async fn a_reconnect_clears_stale_light_state() {
        let (surface, _input) = FakePush::new();
        surface
            .set_led(ControlId::Pad(pad(0)), LedState::solid(Rgb::WHITE))
            .await
            .expect("pads have lights");

        surface.disconnect().await;
        surface.reconnect().await;
        assert_eq!(
            surface.state().await.lit_count(),
            0,
            "the device comes back dark and must be redrawn"
        );
    }

    #[tokio::test]
    async fn injected_input_arrives_in_order() {
        let (surface, mut input) = FakePush::new();
        let at = Instant::now();

        surface.press_pad(pad(3), at).await;
        surface.release_pad(pad(3), at).await;

        let first = input.next_event().await.expect("a press was injected");
        let second = input.next_event().await.expect("a release was injected");
        assert_eq!(first.phase, InputPhase::Down { velocity: 100 });
        assert_eq!(second.phase, InputPhase::Up);
        assert_eq!(first.control, ControlId::Pad(pad(3)));
    }

    #[tokio::test]
    async fn presented_frames_are_kept_for_inspection() {
        let (surface, _input) = FakePush::new();
        let mut frame = DisplayFrame::blank();
        frame.fill(Rgb::WHITE);

        surface
            .present(&frame)
            .await
            .expect("the surface is connected");
        let state = surface.state().await;
        assert_eq!(state.frame_count(), 1);
        assert_eq!(state.last_frame().and_then(|f| f.pixel(0, 0)), Some(0xFFFF));
    }
}
