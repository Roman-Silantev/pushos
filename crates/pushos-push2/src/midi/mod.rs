//! The MIDI half of the Push 2 adapter.
//!
//! Everything that knows a note or controller number lives here. Decoding and
//! encoding are pure functions over bytes, so they are tested without hardware.

pub(crate) mod decode;
pub(crate) mod encode;
pub(crate) mod map;
pub(crate) mod palette;
pub(crate) mod sysex;
pub(crate) mod transport;
