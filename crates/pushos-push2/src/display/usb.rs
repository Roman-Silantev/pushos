//! The USB bulk transport for the display.
//!
//! `rusb` is blocking, so the transfer loop owns a dedicated thread and takes
//! frames through a single-slot mailbox. The renderer therefore never waits on
//! USB, and a slow bus drops stale frames instead of queueing them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pushos_domain::ports::DisplayFrame;
use tracing::{debug, warn};

use crate::identity::{DISPLAY_ENDPOINT, USB_PRODUCT_ID, USB_VENDOR_ID};

use super::frame::{FRAME_HEADER, FrameEncoder};
use super::slot::{LatestSlot, SlotEvent};

/// How long a single bulk transfer may take before it is treated as a fault.
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(1_000);

/// The display blanks itself after two seconds without a frame, so the loop
/// re-sends the last one well inside that window.
const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(700);

/// How long the loop waits between frames while the picture is black.
///
/// Long enough to be no wake-ups at all in practice. A new frame wakes it at
/// once; this only bounds how long an idle thread sleeps between checks.
const DARK_WAIT: Duration = Duration::from_secs(600);

/// A live connection to the display.
#[derive(Debug)]
pub(crate) struct DisplayLink {
    frames: Arc<LatestSlot<Arc<DisplayFrame>>>,
    healthy: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DisplayLink {
    /// Opens the display and starts its transfer loop.
    pub(crate) fn open() -> Result<Self, DisplayError> {
        let handle = rusb::open_device_with_vid_pid(USB_VENDOR_ID, USB_PRODUCT_ID)
            .ok_or(DisplayError::NotFound)?;
        handle.claim_interface(0).map_err(DisplayError::Claim)?;

        let frames = LatestSlot::new();
        let healthy = Arc::new(AtomicBool::new(true));
        let worker = {
            let frames = Arc::clone(&frames);
            let healthy = Arc::clone(&healthy);
            thread::Builder::new()
                .name("pushos-display".to_owned())
                .spawn(move || transfer_loop(&handle, &frames, &healthy))
                .map_err(DisplayError::Spawn)?
        };

        Ok(Self {
            frames,
            healthy,
            worker: Some(worker),
        })
    }

    /// Queues a frame, replacing any frame not yet sent.
    ///
    /// Returns `false` once the transfer loop has stopped.
    pub(crate) fn present(&self, frame: Arc<DisplayFrame>) -> bool {
        self.is_healthy() && self.frames.publish(frame)
    }

    /// Whether the transfer loop is still running without errors.
    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.frames.close();
        if let Some(worker) = self.worker.take() {
            // The loop only ever waits on the slot or a bounded USB transfer,
            // so this join is short.
            if worker.join().is_err() {
                warn!("the display transfer thread ended abnormally");
            }
        }
    }
}

fn transfer_loop(
    handle: &rusb::DeviceHandle<rusb::GlobalContext>,
    frames: &LatestSlot<Arc<DisplayFrame>>,
    healthy: &AtomicBool,
) {
    let mut encoder = FrameEncoder::new();
    // The panel starts blank, and a blank panel needs nothing sent to stay so.
    let mut dark = true;

    loop {
        let wait = if dark { DARK_WAIT } else { KEEPALIVE_INTERVAL };
        match frames.take_within(wait) {
            // Encoded once, when it arrives. Keeping it alive sends the same
            // bytes again rather than working them out again.
            SlotEvent::Value(latest) => {
                dark = latest.is_black();
                encoder.encode(&latest);
            }
            // A black picture is not kept alive. The panel turns itself black
            // two seconds after its last frame, which is exactly what a black
            // frame shows, so a sleeping surface sends nothing over USB all
            // night instead of a third of a megabyte every 700 milliseconds.
            SlotEvent::Idle if dark => continue,
            // Anything else would blank without it, so it is sent again.
            SlotEvent::Idle => {}
            SlotEvent::Closed => break,
        }

        if let Err(error) = send_frame(handle, encoder.encoded()) {
            warn!(%error, "display transfer failed; treating the display as gone");
            healthy.store(false, Ordering::Release);
            break;
        }
    }

    debug!("display transfer loop stopped");
    healthy.store(false, Ordering::Release);
}

fn send_frame(
    handle: &rusb::DeviceHandle<rusb::GlobalContext>,
    wire: &[u8],
) -> Result<(), rusb::Error> {
    handle.write_bulk(DISPLAY_ENDPOINT, &FRAME_HEADER, TRANSFER_TIMEOUT)?;
    for chunk in FrameEncoder::chunks(wire) {
        handle.write_bulk(DISPLAY_ENDPOINT, chunk, TRANSFER_TIMEOUT)?;
    }
    Ok(())
}

/// Why the display could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    /// No Push 2 is attached.
    #[error("no Push 2 display found on the USB bus")]
    NotFound,
    /// Another process holds the display interface.
    #[error("could not claim the Push 2 display interface")]
    Claim(#[source] rusb::Error),
    /// The transfer thread could not be started.
    #[error("could not start the display transfer thread")]
    Spawn(#[source] std::io::Error),
}
