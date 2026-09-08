//! The 8x8 pad grid.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::ParseControlError;

/// Number of pad columns on the Push 2 grid.
pub const PAD_COLUMNS: u8 = 8;
/// Number of pad rows on the Push 2 grid.
pub const PAD_ROWS: u8 = 8;
/// Total addressable pads.
pub const PAD_COUNT: usize = (PAD_COLUMNS as usize) * (PAD_ROWS as usize);

/// A pad position, numbered left-to-right then top-to-bottom.
///
/// Index `0` is the top-left pad, matching how a page layout reads on screen.
/// The hardware's own bottom-left origin is an adapter concern.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct PadIndex(u8);

impl PadIndex {
    /// Builds a pad index, rejecting values outside the grid.
    pub const fn new(index: u8) -> Result<Self, ParseControlError> {
        if (index as usize) < PAD_COUNT {
            Ok(Self(index))
        } else {
            Err(ParseControlError::PadOutOfRange { index })
        }
    }

    /// Builds a pad index from a row and column, both zero-based from the top-left.
    pub const fn from_row_column(row: u8, column: u8) -> Result<Self, ParseControlError> {
        if row >= PAD_ROWS || column >= PAD_COLUMNS {
            return Err(ParseControlError::PadOutOfRange {
                index: row.saturating_mul(PAD_COLUMNS).saturating_add(column),
            });
        }
        Ok(Self(row * PAD_COLUMNS + column))
    }

    /// The flat grid index.
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// The flat grid index as a slice offset.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Zero-based row, counted from the top.
    pub const fn row(self) -> u8 {
        self.0 / PAD_COLUMNS
    }

    /// Zero-based column, counted from the left.
    pub const fn column(self) -> u8 {
        self.0 % PAD_COLUMNS
    }

    /// Iterates every pad in reading order.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..PAD_ROWS * PAD_COLUMNS).map(Self)
    }
}

impl fmt::Display for PadIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pad.{}", self.0)
    }
}

impl fmt::Debug for PadIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PadIndex({})", self.0)
    }
}

impl From<PadIndex> for u8 {
    fn from(pad: PadIndex) -> Self {
        pad.0
    }
}

impl TryFrom<u8> for PadIndex {
    type Error = ParseControlError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for PadIndex {
    type Err = ParseControlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let index = s
            .parse::<u8>()
            .map_err(|_| ParseControlError::unknown_pad(s))?;
        Self::new(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_beyond_the_grid_are_rejected() {
        assert!(PadIndex::new(63).is_ok());
        assert!(PadIndex::new(64).is_err());
    }

    #[test]
    fn row_and_column_round_trip() {
        for pad in PadIndex::all() {
            let rebuilt = PadIndex::from_row_column(pad.row(), pad.column())
                .expect("row and column derived from a valid pad stay in range");
            assert_eq!(pad, rebuilt);
        }
    }

    #[test]
    fn index_zero_is_the_top_left_pad() {
        let pad = PadIndex::new(0).expect("0 is in range");
        assert_eq!((pad.row(), pad.column()), (0, 0));
        assert_eq!(pad.to_string(), "pad.0");
    }
}
