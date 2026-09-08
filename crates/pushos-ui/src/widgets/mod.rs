//! The pieces the display is drawn from.
//!
//! Each widget draws into a region it is given and knows nothing about the rest
//! of the screen, so the layout can change without touching them.

mod footer;
mod layout;
mod overlay;
mod slots;
mod status;

pub(crate) use footer::draw_footer;
pub use layout::Layout;
pub(crate) use overlay::draw_overlay;
pub(crate) use slots::draw_slots;
pub(crate) use status::draw_status;
