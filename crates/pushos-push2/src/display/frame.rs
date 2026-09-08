//! Display frame encoding.
//!
//! The Push 2 display expects each line padded to two kilobytes and every byte
//! inverted against a fixed signal-shaping pattern. This module does that
//! transformation into a buffer it owns and reuses, so presenting a frame
//! allocates nothing.

use pushos_domain::ports::{DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayFrame};

/// The fixed sixteen-byte header that precedes every frame.
pub(crate) const FRAME_HEADER: [u8; 16] =
    [0xFF, 0xCC, 0xAA, 0x88, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Bytes of real pixel data in one line.
const LINE_PIXEL_BYTES: usize = DISPLAY_WIDTH * 2;
/// Bytes sent per line, including the filler that keeps lines from straddling
/// the device's internal buffer boundaries.
const LINE_BYTES: usize = 2048;
/// Total bytes of pixel data in one frame.
pub(crate) const FRAME_BYTES: usize = LINE_BYTES * DISPLAY_HEIGHT;
/// The size of each bulk transfer. A multiple of the line length, chosen for
/// throughput rather than correctness.
pub(crate) const CHUNK_BYTES: usize = 16_384;

/// The signal-shaping pattern, in the byte order it is applied.
const SHAPING: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];

/// Converts frames into the bytes the display expects.
///
/// Owns its output buffer so that the render loop reuses one allocation for the
/// lifetime of the connection.
pub(crate) struct FrameEncoder {
    // A boxed slice rather than a boxed array: at 320 kilobytes this must be
    // heap-allocated directly, never built on the stack and moved.
    buffer: Box<[u8]>,
}

impl FrameEncoder {
    /// Allocates an encoder with a zeroed buffer.
    pub(crate) fn new() -> Self {
        Self {
            buffer: vec![0u8; FRAME_BYTES].into_boxed_slice(),
        }
    }

    /// Encodes a frame and returns the bytes to send.
    ///
    /// Filler bytes are written once at construction and never touched again,
    /// so each call only rewrites real pixel data.
    pub(crate) fn encode(&mut self, frame: &DisplayFrame) -> &[u8] {
        let pixels = frame.pixels();
        for row in 0..DISPLAY_HEIGHT {
            let line_start = row * LINE_BYTES;
            let source = &pixels[row * DISPLAY_WIDTH..(row + 1) * DISPLAY_WIDTH];
            debug_assert_eq!(source.len() * 2, LINE_PIXEL_BYTES);

            for (column, pixel) in source.iter().enumerate() {
                let bytes = pixel.to_le_bytes();
                let offset = line_start + column * 2;
                let shaping = (column * 2) % SHAPING.len();
                self.buffer[offset] = bytes[0] ^ SHAPING[shaping];
                self.buffer[offset + 1] = bytes[1] ^ SHAPING[(shaping + 1) % SHAPING.len()];
            }
        }
        &self.buffer
    }

    /// The encoded bytes, split into transfer-sized chunks.
    pub(crate) fn chunks(encoded: &[u8]) -> impl Iterator<Item = &[u8]> {
        encoded.chunks(CHUNK_BYTES)
    }
}

impl std::fmt::Debug for FrameEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameEncoder")
            .field("bytes", &FRAME_BYTES)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::color::Rgb;

    use super::*;

    /// Recovers a pixel from encoded bytes, undoing the shaping pattern.
    fn decode_pixel(wire: &[u8], x: usize, y: usize) -> u16 {
        let offset = y * LINE_BYTES + x * 2;
        let shaping = (x * 2) % SHAPING.len();
        let low = wire[offset] ^ SHAPING[shaping];
        let high = wire[offset + 1] ^ SHAPING[(shaping + 1) % SHAPING.len()];
        u16::from_le_bytes([low, high])
    }

    #[test]
    fn the_encoded_frame_is_the_documented_size() {
        let mut subject = FrameEncoder::new();
        let wire = subject.encode(&DisplayFrame::blank());
        assert_eq!(wire.len(), 327_680);
        assert_eq!(wire.len(), 160 * 2048);
    }

    #[test]
    fn the_first_bytes_are_inverted_against_the_documented_pattern() {
        let mut subject = FrameEncoder::new();
        let wire = subject.encode(&DisplayFrame::blank());
        // A black frame is all zeroes, so the encoded bytes are the pattern.
        assert_eq!(&wire[0..4], &[0xE7, 0xF3, 0xE7, 0xFF]);
        assert_eq!(&wire[4..8], &[0xE7, 0xF3, 0xE7, 0xFF]);
    }

    #[test]
    fn every_pixel_survives_a_round_trip_through_the_encoding() {
        let mut frame = DisplayFrame::blank();
        // A gradient, so a transposed row or column would be visible.
        for y in 0..DISPLAY_HEIGHT {
            for x in 0..DISPLAY_WIDTH {
                let across = u8::try_from(x % 256).expect("a value below 256 fits a byte");
                let down = u8::try_from(y % 256).expect("a value below 256 fits a byte");
                let color = Rgb::new(across, down, across.wrapping_add(down));
                frame.pixels_mut()[y * DISPLAY_WIDTH + x] = color.to_bgr565();
            }
        }

        let mut subject = FrameEncoder::new();
        let wire = subject.encode(&frame);

        for (x, y) in [
            (0, 0),
            (1, 0),
            (2, 0),
            (959, 0),
            (0, 159),
            (959, 159),
            (480, 80),
        ] {
            assert_eq!(
                decode_pixel(wire, x, y),
                frame.pixel(x, y).expect("coordinate is inside the frame"),
                "pixel ({x}, {y}) did not survive encoding"
            );
        }
    }

    #[test]
    fn the_filler_at_the_end_of_each_line_is_left_untouched() {
        let mut frame = DisplayFrame::blank();
        frame.fill(Rgb::WHITE);
        let mut subject = FrameEncoder::new();
        let wire = subject.encode(&frame);

        for row in 0..DISPLAY_HEIGHT {
            let filler = &wire[row * LINE_BYTES + LINE_PIXEL_BYTES..(row + 1) * LINE_BYTES];
            assert_eq!(filler.len(), 128);
            assert!(
                filler.iter().all(|&byte| byte == 0),
                "line {row} filler was written to"
            );
        }
    }

    #[test]
    fn encoding_twice_reuses_the_buffer_and_leaves_no_residue() {
        let mut subject = FrameEncoder::new();
        let mut bright = DisplayFrame::blank();
        bright.fill(Rgb::WHITE);

        let first: Vec<u8> = subject.encode(&bright).to_vec();
        let second: Vec<u8> = subject.encode(&DisplayFrame::blank()).to_vec();
        let third: Vec<u8> = subject.encode(&bright).to_vec();

        assert_ne!(first, second);
        assert_eq!(
            first, third,
            "re-encoding the same frame must be deterministic"
        );
    }

    #[test]
    fn transfers_divide_evenly_and_cover_the_whole_frame() {
        let mut subject = FrameEncoder::new();
        let wire = subject.encode(&DisplayFrame::blank());
        let chunks: Vec<_> = FrameEncoder::chunks(wire).collect();

        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            FRAME_BYTES
        );
        assert!(chunks.iter().all(|chunk| chunk.len() == CHUNK_BYTES));
        // Each transfer holds whole lines, so a partial write never splits one.
        assert_eq!(CHUNK_BYTES % LINE_BYTES, 0);
    }
}
