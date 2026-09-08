//! The drawing surface.
//!
//! A canvas is allocated once per renderer and reused. Drawing happens in
//! straight RGB, and the conversion into the display's packed format is the
//! last step, so nothing upstream needs to know the panel's pixel layout.

use pushos_domain::color::Rgb;
use pushos_domain::ports::{DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayFrame};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// A rectangle in display coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    /// Distance from the left edge.
    pub x: f32,
    /// Distance from the top edge.
    pub y: f32,
    /// Horizontal extent.
    pub width: f32,
    /// Vertical extent.
    pub height: f32,
}

impl Area {
    /// Builds an area.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole display.
    ///
    /// The panel is 960 by 160, both far inside the range a `f32` represents
    /// exactly, so the conversion is lossless.
    #[allow(clippy::cast_precision_loss)]
    pub const FULL: Self = Self::new(0.0, 0.0, DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32);

    /// Shrinks the area by `amount` on every side.
    #[must_use]
    pub fn inset(self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.width - amount * 2.0).max(0.0),
            (self.height - amount * 2.0).max(0.0),
        )
    }

    /// The vertical coordinate just past the bottom edge.
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }
}

/// A reusable drawing surface for one display frame.
pub struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    /// Allocates a canvas at the display's native size.
    ///
    /// Returns `None` only if the display dimensions are not a valid pixmap
    /// size, which the constants make impossible.
    pub fn new() -> Option<Self> {
        let width = u32::try_from(DISPLAY_WIDTH).ok()?;
        let height = u32::try_from(DISPLAY_HEIGHT).ok()?;
        Pixmap::new(width, height).map(|pixmap| Self { pixmap })
    }

    /// Fills the whole canvas with one colour.
    pub fn clear(&mut self, color: Rgb) {
        self.pixmap.fill(to_skia(color));
    }

    /// Fills a rectangle.
    pub fn fill(&mut self, area: Area, color: Rgb) {
        let Some(rect) = Rect::from_xywh(area.x, area.y, area.width, area.height) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(to_skia(color));
        paint.anti_alias = false;
        self.pixmap
            .fill_rect(rect, &paint, Transform::identity(), None);
    }

    /// Fills a rectangle with rounded corners.
    pub fn fill_rounded(&mut self, area: Area, radius: f32, color: Rgb) {
        let radius = radius.min(area.width / 2.0).min(area.height / 2.0).max(0.0);
        if radius <= 0.5 {
            self.fill(area, color);
            return;
        }

        let mut builder = PathBuilder::new();
        let (l, t) = (area.x, area.y);
        let (r, b) = (area.x + area.width, area.bottom());
        builder.move_to(l + radius, t);
        builder.line_to(r - radius, t);
        builder.quad_to(r, t, r, t + radius);
        builder.line_to(r, b - radius);
        builder.quad_to(r, b, r - radius, b);
        builder.line_to(l + radius, b);
        builder.quad_to(l, b, l, b - radius);
        builder.line_to(l, t + radius);
        builder.quad_to(l, t, l + radius, t);
        builder.close();

        let Some(path) = builder.finish() else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(to_skia(color));
        paint.anti_alias = true;
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// Blends one pixel using an eight-bit coverage value.
    ///
    /// Used by text rendering, which produces coverage rather than colour.
    pub fn blend(&mut self, x: i32, y: i32, coverage: u8, color: Rgb) {
        if coverage == 0 {
            return;
        }
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return;
        };
        if x >= DISPLAY_WIDTH || y >= DISPLAY_HEIGHT {
            return;
        }

        let index = y * DISPLAY_WIDTH + x;
        let data = self.pixmap.pixels_mut();
        let existing = data[index];
        let mix = |source: u8, destination: u8| -> u8 {
            let alpha = u16::from(coverage);
            let blended =
                (u16::from(source) * alpha + u16::from(destination) * (255 - alpha)) / 255;
            u8::try_from(blended).unwrap_or(u8::MAX)
        };

        // The pixmap is premultiplied, but every pixel here is fully opaque, so
        // the premultiplied and straight values coincide.
        data[index] = tiny_skia::PremultipliedColorU8::from_rgba(
            mix(color.r, existing.red()),
            mix(color.g, existing.green()),
            mix(color.b, existing.blue()),
            255,
        )
        .unwrap_or(existing);
    }

    /// Packs the canvas into a display frame.
    pub fn present_into(&self, frame: &mut DisplayFrame) {
        for (target, pixel) in frame.pixels_mut().iter_mut().zip(self.pixmap.pixels()) {
            *target = Rgb::new(pixel.red(), pixel.green(), pixel.blue()).to_bgr565();
        }
    }
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("width", &self.pixmap.width())
            .field("height", &self.pixmap.height())
            .finish()
    }
}

fn to_skia(color: Rgb) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r, color.g, color.b, 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new().expect("the display size is always a valid pixmap size")
    }

    #[test]
    fn an_area_can_be_inset_without_going_negative() {
        let area = Area::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(area.inset(2.0), Area::new(2.0, 2.0, 6.0, 6.0));
        assert!(area.inset(100.0).width.abs() < f32::EPSILON);
    }

    #[test]
    fn clearing_then_presenting_fills_the_whole_frame() {
        let mut canvas = canvas();
        canvas.clear(Rgb::new(255, 0, 0));

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);

        let expected = Rgb::new(255, 0, 0).to_bgr565();
        assert!(frame.pixels().iter().all(|&pixel| pixel == expected));
    }

    #[test]
    fn a_filled_rectangle_covers_exactly_its_area() {
        let mut canvas = canvas();
        canvas.clear(Rgb::BLACK);
        canvas.fill(Area::new(10.0, 10.0, 20.0, 20.0), Rgb::WHITE);

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);

        assert_eq!(frame.pixel(15, 15), Some(0xFFFF));
        assert_eq!(frame.pixel(9, 15), Some(0x0000));
        assert_eq!(frame.pixel(30, 15), Some(0x0000));
    }

    #[test]
    fn drawing_outside_the_canvas_is_ignored_rather_than_wrapping() {
        let mut canvas = canvas();
        canvas.clear(Rgb::BLACK);
        canvas.blend(-1, -1, 255, Rgb::WHITE);
        canvas.blend(10_000, 0, 255, Rgb::WHITE);
        canvas.blend(0, 10_000, 255, Rgb::WHITE);

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);
        assert!(frame.pixels().iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn zero_coverage_leaves_a_pixel_untouched() {
        let mut canvas = canvas();
        canvas.clear(Rgb::BLACK);
        canvas.blend(5, 5, 0, Rgb::WHITE);

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);
        assert_eq!(frame.pixel(5, 5), Some(0x0000));
    }

    #[test]
    fn full_coverage_replaces_a_pixel_entirely() {
        let mut canvas = canvas();
        canvas.clear(Rgb::BLACK);
        canvas.blend(5, 5, 255, Rgb::WHITE);

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);
        assert_eq!(frame.pixel(5, 5), Some(0xFFFF));
    }

    #[test]
    fn a_rounded_rectangle_leaves_its_corners_clear() {
        let mut canvas = canvas();
        canvas.clear(Rgb::BLACK);
        canvas.fill_rounded(Area::new(0.0, 0.0, 40.0, 40.0), 12.0, Rgb::WHITE);

        let mut frame = DisplayFrame::blank();
        canvas.present_into(&mut frame);

        assert_eq!(frame.pixel(20, 20), Some(0xFFFF), "the middle is filled");
        assert_eq!(frame.pixel(0, 0), Some(0x0000), "the corner is not");
    }
}
