//! Where things sit on a 960 by 160 panel.
//!
//! The layout is fixed rather than computed, because the display's proportions
//! never change and the eight columns must line up with the eight physical
//! encoders above the screen and buttons below it.

use crate::canvas::Area;
use crate::snapshot::SLOT_COUNT;

/// Height of the status bar along the top.
pub(super) const STATUS_HEIGHT: f32 = 24.0;
/// Height of the message line along the bottom.
pub(super) const FOOTER_HEIGHT: f32 = 34.0;
/// Gap between adjacent slots.
pub(super) const SLOT_GAP: f32 = 2.0;
/// Padding inside a slot.
pub(super) const SLOT_PADDING: f32 = 8.0;

/// The regions of the display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The bar along the top.
    pub status: Area,
    /// The band of eight columns.
    pub slots: Area,
    /// The line along the bottom.
    pub footer: Area,
}

impl Layout {
    /// The fixed layout.
    pub fn standard() -> Self {
        let full = Area::FULL;
        let slots_height = full.height - STATUS_HEIGHT - FOOTER_HEIGHT;
        Self {
            status: Area::new(0.0, 0.0, full.width, STATUS_HEIGHT),
            slots: Area::new(0.0, STATUS_HEIGHT, full.width, slots_height),
            footer: Area::new(0.0, STATUS_HEIGHT + slots_height, full.width, FOOTER_HEIGHT),
        }
    }

    /// The area of one column, counted from the left.
    pub fn slot(&self, index: usize) -> Area {
        #[allow(clippy::cast_precision_loss)]
        let column_width = self.slots.width / SLOT_COUNT as f32;
        #[allow(clippy::cast_precision_loss)]
        let x = column_width * index as f32;
        Area::new(
            x + SLOT_GAP / 2.0,
            self.slots.y + SLOT_GAP,
            column_width - SLOT_GAP,
            self.slots.height - SLOT_GAP * 2.0,
        )
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::ports::{DISPLAY_HEIGHT, DISPLAY_WIDTH};

    use super::*;

    #[test]
    fn the_three_bands_tile_the_display_exactly() {
        let layout = Layout::standard();
        let same = |a: f32, b: f32| (a - b).abs() < f32::EPSILON;
        assert!(same(layout.status.y, 0.0));
        assert!(same(layout.status.bottom(), layout.slots.y));
        assert!(same(layout.slots.bottom(), layout.footer.y));
        #[allow(clippy::cast_precision_loss)]
        let height = DISPLAY_HEIGHT as f32;
        assert!((layout.footer.bottom() - height).abs() < f32::EPSILON);
    }

    #[test]
    fn the_eight_columns_span_the_full_width_without_overlapping() {
        let layout = Layout::standard();
        for index in 0..SLOT_COUNT - 1 {
            let current = layout.slot(index);
            let next = layout.slot(index + 1);
            assert!(current.x + current.width <= next.x + f32::EPSILON);
        }

        let last = layout.slot(SLOT_COUNT - 1);
        #[allow(clippy::cast_precision_loss)]
        let width = DISPLAY_WIDTH as f32;
        assert!(last.x + last.width <= width);
        assert!(
            last.x + last.width > width - 4.0,
            "the columns should reach the right edge"
        );
    }

    #[test]
    fn every_column_is_the_same_size() {
        let layout = Layout::standard();
        let first = layout.slot(0);
        for index in 1..SLOT_COUNT {
            let slot = layout.slot(index);
            assert!((slot.width - first.width).abs() < f32::EPSILON);
            assert!((slot.height - first.height).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn a_column_is_wide_enough_for_a_short_label() {
        assert!(Layout::standard().slot(0).width > 100.0);
    }
}
