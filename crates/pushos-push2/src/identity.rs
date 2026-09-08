//! How PushOS recognises a Push 2 and talks to it.
//!
//! Values are taken from Ableton's published *Push 2 MIDI and Display
//! Interface Manual*. Nothing outside this crate should need them.

/// Ableton's USB vendor identifier.
pub const USB_VENDOR_ID: u16 = 0x2982;
/// The Push 2's USB product identifier.
pub const USB_PRODUCT_ID: u16 = 0x1967;
/// The bulk endpoint the display frame is written to.
pub const DISPLAY_ENDPOINT: u8 = 0x01;

/// The MIDI port PushOS drives, so that Live keeps the other one.
pub const USER_PORT_SUFFIX: &str = "User Port";
/// The MIDI port Live uses. PushOS avoids it unless explicitly told otherwise.
pub const LIVE_PORT_SUFFIX: &str = "Live Port";
/// The name PushOS registers its own virtual ports under.
pub const CLIENT_NAME: &str = "PushOS";

/// The prefix every Push 2 system-exclusive message carries: start byte,
/// Ableton's manufacturer id, device id and model id.
pub const SYSEX_PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
/// The system-exclusive terminator.
pub const SYSEX_END: u8 = 0xF7;

/// Which of the two MIDI ports a connection should use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortRole {
    /// The user port, which PushOS owns without disturbing Live.
    #[default]
    User,
    /// The Live port. Only for diagnostics.
    Live,
}

impl PortRole {
    /// The substring that identifies the port in the system's MIDI device list.
    pub const fn name_suffix(self) -> &'static str {
        match self {
            Self::User => USER_PORT_SUFFIX,
            Self::Live => LIVE_PORT_SUFFIX,
        }
    }

    /// The `Set MIDI Mode` argument that routes control traffic to this port.
    pub const fn midi_mode(self) -> u8 {
        match self {
            Self::User => 1,
            Self::Live => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sysex_prefix_matches_the_published_format() {
        assert_eq!(SYSEX_PREFIX, [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01]);
    }

    #[test]
    fn pushos_defaults_to_the_user_port_so_live_keeps_its_own() {
        assert_eq!(PortRole::default(), PortRole::User);
        assert_eq!(PortRole::default().name_suffix(), "User Port");
        assert_eq!(PortRole::default().midi_mode(), 1);
    }
}
