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
    let mut current = Arc::new(DisplayFrame::blank());

    loop {
        match frames.take_within(KEEPALIVE_INTERVAL) {
            SlotEvent::Value(latest) => current = latest,
            // Nothing new: re-send so the panel does not time out and blank.
            SlotEvent::Idle => {}
            SlotEvent::Closed => break,
        }

        if let Err(error) = send_frame(handle, &mut encoder, &current) {
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
    encoder: &mut FrameEncoder,
    frame: &DisplayFrame,
) -> Result<(), rusb::Error> {
    handle.write_bulk(DISPLAY_ENDPOINT, &FRAME_HEADER, TRANSFER_TIMEOUT)?;
    let wire = encoder.encode(frame);
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
