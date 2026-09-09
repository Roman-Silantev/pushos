//! The MIDI transport.
//!
//! `midir` delivers input on its own callback thread and requires exclusive
//! ownership of the output connection, so writes are funnelled through one
//! dedicated thread. Callers hand over already-encoded messages and never wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use pushos_domain::input::ControlEvent;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, error, warn};

use crate::identity::{CLIENT_NAME, PortRole};

use super::{decode, sysex};

/// How many input events may be in flight before the surface is outrunning the
/// runtime. A human cannot fill this; a fault can, and bounding it is the point.
const INPUT_CAPACITY: usize = 512;

/// How many outgoing batches may queue before writes are dropped.
///
/// Smaller than the colour palette on purpose: during operation the latest
/// state is what matters and a dropped light is corrected by the next redraw,
/// so a deep queue would only add latency. The handful of messages that are
/// said once go through [`MidiLink::send_now`] instead.
pub(crate) const OUTPUT_CAPACITY: usize = 64;

/// The MIDI real-time start message, which begins the shared animation phase.
///
/// Without it the hardware runs no blinking or pulsing at all. With it and no
/// clock messages, animations free-run at 120 beats per minute, which is
/// exactly what PushOS wants and costs no timer.
const MIDI_START: [u8; 1] = [0xFA];

/// A live MIDI connection to the surface.
pub(crate) struct MidiLink {
    // Held so the input connection stays open for the link's lifetime.
    _input: MidiInputConnection<()>,
    commands: SyncSender<MidiCommand>,
    dropped_writes: Arc<AtomicU64>,
    writer: Option<JoinHandle<()>>,
}

/// What the writer thread is asked to do.
enum MidiCommand {
    /// Send a batch of channel messages.
    Batch(Vec<[u8; 3]>),
    /// Send one system-exclusive or real-time message.
    Raw(Vec<u8>),
}

impl std::fmt::Debug for MidiCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batch(messages) => write!(f, "Batch({})", messages.len()),
            Self::Raw(bytes) => write!(f, "Raw({})", bytes.len()),
        }
    }
}

impl std::fmt::Debug for MidiLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiLink")
            .field("dropped_writes", &self.dropped_writes())
            .finish_non_exhaustive()
    }
}

impl MidiLink {
    /// Opens both directions of the given port and switches the device into the
    /// matching MIDI mode.
    ///
    /// Returns the link and the receiver that normalised input arrives on.
    pub(crate) fn open(
        role: PortRole,
    ) -> Result<(Self, tokio_mpsc::Receiver<ControlEvent>), MidiError> {
        let (events_tx, events_rx) = tokio_mpsc::channel(INPUT_CAPACITY);
        let dropped_inputs = Arc::new(AtomicU64::new(0));

        let input = Self::open_input(role, events_tx, Arc::clone(&dropped_inputs))?;
        let output = Self::open_output(role)?;

        let (commands, requests) = mpsc::sync_channel(OUTPUT_CAPACITY);
        let writer = thread::Builder::new()
            .name("pushos-midi-out".to_owned())
            .spawn(move || write_loop(output, &requests))
            .map_err(MidiError::Spawn)?;

        let link = Self {
            _input: input,
            commands,
            dropped_writes: Arc::new(AtomicU64::new(0)),
            writer: Some(writer),
        };

        // Take ownership of the surface, then start the animation phase. Both
        // are said once and never again: a dropped mode change leaves the Push
        // talking to Live, and a dropped start leaves every blink still.
        link.send_now(sysex::set_midi_mode(role));
        link.send_now(MIDI_START.to_vec());

        Ok((link, events_rx))
    }

    /// Queues a batch of channel messages.
    pub(crate) fn send_batch(&self, messages: Vec<[u8; 3]>) {
        if messages.is_empty() {
            return;
        }
        self.enqueue(MidiCommand::Batch(messages));
    }

    /// Sends one message, waiting for room rather than dropping it.
    ///
    /// Every system-exclusive message PushOS sends is like this. There is no
    /// queueing version, because there is nothing PushOS says this way that it
    /// would say again: the palette, the mode change and the start of the
    /// animation phase are each said once.
    ///
    /// A dropped light is corrected by the next redraw, but a dropped palette
    /// entry is a colour that stays wrong for as long as the Push is plugged
    /// in, and a dropped mode change leaves the surface talking to Live.
    ///
    /// Waiting here is safe: this is the connecting path, not the input path,
    /// and the writer drains steadily. A stopped writer returns rather than
    /// blocking, because a disconnected channel is never full again.
    pub(crate) fn send_now(&self, bytes: Vec<u8>) {
        if self.commands.send(MidiCommand::Raw(bytes)).is_err() {
            debug!("MIDI writer has stopped; discarding outgoing message");
        }
    }

    /// Sends a batch, waiting for room rather than dropping it.
    ///
    /// For the batch that runs at connect and at disconnect. During operation a
    /// dropped batch is corrected by the next redraw, but the one that puts the
    /// surface into a known state has nothing after it to correct it.
    pub(crate) fn send_batch_now(&self, messages: Vec<[u8; 3]>) {
        if messages.is_empty() {
            return;
        }
        if self.commands.send(MidiCommand::Batch(messages)).is_err() {
            debug!("MIDI writer has stopped; discarding outgoing batch");
        }
    }

    /// How many outgoing batches have been dropped because the writer fell behind.
    pub(crate) fn dropped_writes(&self) -> u64 {
        self.dropped_writes.load(Ordering::Relaxed)
    }

    /// Whether the writer thread is still accepting work.
    pub(crate) fn is_healthy(&self) -> bool {
        self.commands
            .try_send(MidiCommand::Batch(Vec::new()))
            .map_or_else(
                |error| !matches!(error, TrySendError::Disconnected(_)),
                |()| true,
            )
    }

    fn enqueue(&self, command: MidiCommand) {
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(dropped)) => {
                self.dropped_writes.fetch_add(1, Ordering::Relaxed);
                warn!(?dropped, "MIDI writer is behind; dropped an outgoing batch");
            }
            Err(TrySendError::Disconnected(_)) => {
                debug!("MIDI writer has stopped; discarding outgoing batch");
            }
        }
    }

    fn open_input(
        role: PortRole,
        events: tokio_mpsc::Sender<ControlEvent>,
        dropped: Arc<AtomicU64>,
    ) -> Result<MidiInputConnection<()>, MidiError> {
        let mut input = MidiInput::new(CLIENT_NAME).map_err(|_| MidiError::ClientInit)?;
        // Clock and active sensing arrive constantly and mean nothing to PushOS.
        input.ignore(Ignore::TimeAndActiveSense);

        let port = input
            .ports()
            .into_iter()
            .find(|port| {
                input
                    .port_name(port)
                    .is_ok_and(|name| name.contains(role.name_suffix()))
            })
            .ok_or(MidiError::PortNotFound { role })?;

        input
            .connect(
                &port,
                "pushos-in",
                move |_timestamp, message, ()| {
                    let Some(event) = decode::decode(message, Instant::now()) else {
                        return;
                    };
                    if let Err(error) = events.try_send(event) {
                        record_input_overflow(&error, &dropped);
                    }
                },
                (),
            )
            .map_err(|_| MidiError::Connect { role })
    }

    fn open_output(role: PortRole) -> Result<MidiOutputConnection, MidiError> {
        let output = MidiOutput::new(CLIENT_NAME).map_err(|_| MidiError::ClientInit)?;
        let port = output
            .ports()
            .into_iter()
            .find(|port| {
                output
                    .port_name(port)
                    .is_ok_and(|name| name.contains(role.name_suffix()))
            })
            .ok_or(MidiError::PortNotFound { role })?;

        output
            .connect(&port, "pushos-out")
            .map_err(|_| MidiError::Connect { role })
    }
}

impl Drop for MidiLink {
    fn drop(&mut self) {
        // Dropping the sender ends the write loop once its queue drains.
        let (closed, _) = mpsc::sync_channel(1);
        let commands = std::mem::replace(&mut self.commands, closed);
        drop(commands);

        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            warn!("the MIDI writer thread ended abnormally");
        }
    }
}

/// Reports a lost input event, but only where losing it matters.
///
/// Continuous streams such as touch-strip position are safe to thin out. A
/// press, release or turn is a state transition and must never be lost quietly.
fn record_input_overflow(
    error: &tokio_mpsc::error::TrySendError<ControlEvent>,
    dropped: &AtomicU64,
) {
    let event = match error {
        tokio_mpsc::error::TrySendError::Full(event)
        | tokio_mpsc::error::TrySendError::Closed(event) => event,
    };
    let total = dropped.fetch_add(1, Ordering::Relaxed) + 1;
    if event.phase.is_continuous() {
        debug!(total, "thinned a continuous input stream");
    } else {
        error!(control = %event.control, total, "dropped a control transition");
    }
}

fn write_loop(mut output: MidiOutputConnection, requests: &mpsc::Receiver<MidiCommand>) {
    while let Ok(command) = requests.recv() {
        let result = match &command {
            MidiCommand::Batch(messages) => {
                messages.iter().try_for_each(|message| output.send(message))
            }
            MidiCommand::Raw(bytes) => output.send(bytes),
        };
        if let Err(error) = result {
            warn!(%error, "MIDI write failed; treating the surface as gone");
            break;
        }
    }
    debug!("MIDI write loop stopped");
    output.close();
}

/// Why a MIDI connection could not be established.
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    /// The system MIDI client could not be created.
    #[error("could not create the `{CLIENT_NAME}` MIDI client")]
    ClientInit,
    /// No matching port is present, which normally means no Push 2 is attached.
    #[error("no Push 2 {} is available", role.name_suffix())]
    PortNotFound {
        /// The port that was looked for.
        role: PortRole,
    },
    /// The port exists but could not be opened.
    #[error("could not open the Push 2 {}", role.name_suffix())]
    Connect {
        /// The port that was tried.
        role: PortRole,
    },
    /// The writer thread could not be started.
    #[error("could not start the MIDI writer thread")]
    Spawn(#[source] std::io::Error),
}
