//! The Push 2 device, as the rest of PushOS sees it.
//!
//! [`Push2Device`] implements the domain's output port and hands back an input
//! stream of normalised events. Everything below it is MIDI and USB; everything
//! above it is semantic.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::color::{LedState, Rgb};
use pushos_domain::controls::ControlId;
use pushos_domain::input::ControlEvent;
use pushos_domain::ports::{DisplayFrame, PushInput, PushOutput, PushSurfaceError, SurfaceKind};
use tokio::sync::mpsc;
use tracing::info;

use crate::display::usb::{DisplayError, DisplayLink};
use crate::identity::PortRole;
use crate::midi::transport::{MidiError, MidiLink};
use crate::midi::{encode, palette, sysex};

/// A connected Push 2.
///
/// Dropping the device restores the surface: every light goes out and control
/// traffic is handed back to Live.
#[derive(Debug)]
pub struct Push2Device {
    midi: MidiLink,
    display: DisplayLink,
    role: PortRole,
}

impl Push2Device {
    /// Connects to an attached Push 2 and prepares it for PushOS.
    ///
    /// Preparation installs the PushOS colour palette, so that every subsequent
    /// light update is a single palette index rather than a colour negotiation.
    pub fn connect(role: PortRole) -> Result<(Self, Push2Input), ConnectError> {
        let (midi, events) = MidiLink::open(role).map_err(ConnectError::Midi)?;
        let display = DisplayLink::open().map_err(ConnectError::Display)?;

        let device = Self {
            midi,
            display,
            role,
        };
        device.install_palette();
        device.blank_all_leds();

        info!(port = role.name_suffix(), "Push 2 connected");
        Ok((device, Push2Input { events }))
    }

    /// Whether both transports are still working.
    pub fn is_healthy(&self) -> bool {
        self.midi.is_healthy() && self.display.is_healthy()
    }

    /// How many outgoing MIDI batches were dropped because the writer fell behind.
    pub fn dropped_writes(&self) -> u64 {
        self.midi.dropped_writes()
    }

    /// Replaces the factory palette with the PushOS colour cube.
    ///
    /// Sent once, at connect, and never sent again, so every entry has to
    /// arrive. There are more entries than the writer's queue holds; dropping
    /// the overflow would leave colours wrong for as long as the Push is
    /// plugged in, and nothing would ever correct them.
    fn install_palette(&self) {
        for index in 0..palette::PALETTE_ENTRIES {
            let color = palette::entry(index);
            self.midi.send_now(sysex::set_palette_entry(
                index,
                color,
                palette::white_value(color),
            ));
        }
        self.midi.send_now(sysex::reapply_palette());
    }

    /// Puts every light out, so the surface starts from a known state.
    ///
    /// Waited on rather than queued, and for the same reason as the palette:
    /// this runs at connect, straight after a hundred and twenty-nine palette
    /// messages, and at disconnect, when there is nothing after it. A dropped
    /// light during operation is corrected by the next redraw. These two are
    /// not.
    fn blank_all_leds(&self) {
        let messages: Vec<_> = ControlId::all().filter_map(encode::led_off).collect();
        self.midi.send_batch_now(messages);
    }
}

impl Drop for Push2Device {
    fn drop(&mut self) {
        self.blank_all_leds();
        // Hand the surface back so Live behaves normally once PushOS exits.
        // Waited on rather than queued: this is the last thing PushOS says, and
        // a dropped one leaves the Push in a mode nobody chose.
        if self.role != PortRole::Live {
            self.midi.send_now(sysex::set_midi_mode(PortRole::Live));
        }
    }
}

#[async_trait]
impl PushOutput for Push2Device {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Hardware
    }

    async fn set_led(&self, control: ControlId, state: LedState) -> Result<(), PushSurfaceError> {
        let mut messages = Vec::with_capacity(encode::MAX_MESSAGES_PER_LED);
        encode::led_messages(control, state, &mut messages)?;
        self.midi.send_batch(messages);
        Ok(())
    }

    async fn set_leds(&self, states: &[(ControlId, LedState)]) -> Result<(), PushSurfaceError> {
        let mut messages = Vec::with_capacity(states.len() * encode::MAX_MESSAGES_PER_LED);
        // Encode everything before sending, so a rejected control cannot leave
        // the surface half-updated.
        for &(control, state) in states {
            encode::led_messages(control, state, &mut messages)?;
        }
        self.midi.send_batch(messages);
        Ok(())
    }

    async fn present(&self, frame: &DisplayFrame) -> Result<(), PushSurfaceError> {
        if self.display.present(Arc::new(frame.clone())) {
            Ok(())
        } else {
            Err(PushSurfaceError::Disconnected)
        }
    }

    async fn clear(&self) -> Result<(), PushSurfaceError> {
        self.blank_all_leds();
        let mut blank = DisplayFrame::blank();
        blank.fill(Rgb::BLACK);
        self.present(&blank).await
    }
}

/// The input half of a connected Push 2.
#[derive(Debug)]
pub struct Push2Input {
    events: mpsc::Receiver<ControlEvent>,
}

#[async_trait]
impl PushInput for Push2Input {
    async fn next_event(&mut self) -> Option<ControlEvent> {
        self.events.recv().await
    }
}

/// Why connecting to a Push 2 failed.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The MIDI ports could not be opened.
    #[error("Push 2 MIDI is unavailable")]
    Midi(#[source] MidiError),
    /// The USB display could not be opened.
    #[error("Push 2 display is unavailable")]
    Display(#[source] DisplayError),
}

impl ConnectError {
    /// Whether the failure simply means no Push 2 is attached.
    ///
    /// The supervisor treats this as "wait and retry" rather than a fault,
    /// because PushOS is expected to run with the hardware unplugged.
    pub const fn is_absent(&self) -> bool {
        matches!(
            self,
            Self::Midi(MidiError::PortNotFound { .. }) | Self::Display(DisplayError::NotFound)
        )
    }
}
