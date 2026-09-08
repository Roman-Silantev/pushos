//! The Ableton Push 2 hardware adapter.
//!
//! This crate is the only place in PushOS that knows about MIDI notes,
//! controller numbers, USB endpoints or the display's wire format. Everything
//! above it works with [`pushos_domain::controls::ControlId`] and normalised
//! [`pushos_domain::input::ControlEvent`]s.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod device;
mod display;
pub mod identity;
mod midi;

pub use device::{ConnectError, Push2Device, Push2Input};
pub use display::usb::DisplayError;
pub use identity::{PortRole, USB_PRODUCT_ID, USB_VENDOR_ID};
pub use midi::transport::MidiError;
