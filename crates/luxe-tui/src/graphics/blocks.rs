//! blocks — an image as ordinary colored cells, for terminals with no graphics protocol.
//!
//! Each cell draws `▀` with the upper pixel as its foreground and the lower one as its background,
//! so one cell carries two vertical pixels and a row of cells carries two scanlines. That is a
//! coarse picture — roughly 100×48 at conversation width — but it is coarse in a useful way: a
//! collapsed layout, a panel in the wrong color, the shape of a chart all survive it.
//!
//! What it buys is that there is no protocol involved. These are cells with colors, so they scroll,
//! diff, clip and copy like any other text, in every terminal, inside tmux, and over ssh.

use image::DynamicImage;
use ratatui::style::Color;

/// The glyph that splits a cell in half. Block Elements, drawn by the terminal itself.
const UPPER_HALF: &str = "▀";

/// A rasterized image: two vertical pixels per cell, row-major over `cols` × `rows * 2`.
pub(super) struct Raster {
    cols: u16,
    pixels: Vec<[u8; 4]>,
}

impl Raster {
    /// Rasterizes `image` to exactly `cols`×`rows` cells.
    ///
    /// The caller has already sized the rectangle to the source's aspect ratio, so this fills it
    /// rather than fitting inside it. Lanczos because the result is cached and a screenshot
    /// downscaled by 15× with a cheaper filter is mush.
    pub(super) fn new(image: &DynamicImage, cols: u16, rows: u16) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let scanlines = u32::from(rows) * 2;
        let resized =
            image.resize_exact(u32::from(cols), scanlines, image::imageops::FilterType::Lanczos3);
        let rgba = resized.to_rgba8();
        Self { cols, pixels: rgba.pixels().map(|pixel| pixel.0).collect() }
    }

    pub(super) fn cols(&self) -> u16 {
        self.cols
    }

    /// Cells tall, recovered from the pixel count since only the tests ask.
    #[cfg(test)]
    fn rows(&self) -> u16 {
        (self.pixels.len() / (usize::from(self.cols) * 2)) as u16
    }

    /// The glyph every cell of a raster draws.
    pub(super) fn symbol() -> &'static str {
        UPPER_HALF
    }

    /// Foreground (upper pixel) and background (lower pixel) for one cell, over `canvas`.
    ///
    /// `canvas` is what a transparent pixel resolves to; `None` leaves alpha ignored, which is the
    /// honest answer when the theme's background is the terminal's own default and no number for it
    /// exists.
    pub(super) fn cell(&self, col: u16, row: u16, canvas: Option<[u8; 3]>) -> (Color, Color) {
        let upper = self.pixel(col, u32::from(row) * 2, canvas);
        let lower = self.pixel(col, u32::from(row) * 2 + 1, canvas);
        (upper, lower)
    }

    fn pixel(&self, col: u16, scanline: u32, canvas: Option<[u8; 3]>) -> Color {
        let index = scanline as usize * usize::from(self.cols) + usize::from(col);
        let Some([r, g, b, a]) = self.pixels.get(index).copied() else {
            return Color::Reset;
        };
        match canvas {
            Some(canvas) if a < 255 => {
                let blend = |value: u8, under: u8| {
                    let mixed =
                        u16::from(value) * u16::from(a) + u16::from(under) * u16::from(255 - a);
                    (mixed / 255) as u8
                };
                Color::Rgb(blend(r, canvas[0]), blend(g, canvas[1]), blend(b, canvas[2]))
            }
            _ => Color::Rgb(r, g, b),
        }
    }
}

/// The numeric background a transparent pixel can be composited over, when there is one.
///
/// A theme that leaves the background to the terminal has no number to blend with, and inventing
/// one paints a rectangle of not-quite-black over whatever the user actually chose.
pub(super) fn canvas_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Black => Some([0, 0, 0]),
        Color::White => Some([255, 255, 255]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn split_image() -> DynamicImage {
        // Top half red, bottom half blue — so a one-row raster must show red over blue.
        let mut image = RgbaImage::new(8, 8);
        for (_, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = if y < 4 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) };
        }
        DynamicImage::ImageRgba8(image)
    }

    /// The whole trick in one assertion: a cell's foreground is the upper pixel and its background
    /// is the lower one, so `▀` splits the cell into two scanlines.
    ///
    /// Dominance rather than exact values — a good resampling filter rings at a hard edge, and a
    /// test that pins the exact bytes is really a test of the filter.
    #[test]
    fn a_cell_carries_two_scanlines() {
        let raster = Raster::new(&split_image(), 4, 1);
        assert_eq!((raster.cols(), raster.rows()), (4, 1));
        let (Color::Rgb(ur, _, ub), Color::Rgb(lr, _, lb)) = raster.cell(0, 0, None) else {
            panic!("expected rgb colors");
        };
        assert!(ur > ub, "the upper scanline is the image's red half: {ur},{ub}");
        assert!(lb > lr, "and the lower is its blue half: {lr},{lb}");
    }

    /// A raster fills the rectangle it was asked for; the caller already matched the aspect ratio,
    /// and a raster that quietly returned a different size would shear every row under it.
    #[test]
    fn a_raster_is_exactly_the_size_it_was_asked_for() {
        let raster = Raster::new(&split_image(), 10, 3);
        assert_eq!((raster.cols(), raster.rows()), (10, 3));
        assert_eq!(raster.pixels.len(), 10 * 6);
        // Every cell inside the raster resolves; nothing reads past the end.
        for row in 0..raster.rows() {
            for col in 0..raster.cols() {
                assert_ne!(raster.cell(col, row, None).0, Color::Reset);
            }
        }
    }

    /// A degenerate rectangle still has to produce a drawable raster rather than an empty one.
    #[test]
    fn a_zero_sized_request_still_draws_one_cell() {
        let raster = Raster::new(&split_image(), 0, 0);
        assert_eq!((raster.cols(), raster.rows()), (1, 1));
    }

    /// Reading outside the raster is a caller bug, and it reports one instead of panicking.
    #[test]
    fn a_cell_outside_the_raster_is_not_a_panic() {
        let raster = Raster::new(&split_image(), 2, 1);
        assert_eq!(raster.cell(99, 99, None), (Color::Reset, Color::Reset));
    }

    /// Transparency resolves against the canvas when the theme has a real color, and is ignored
    /// when it does not — a half-transparent white over black is gray, not white.
    #[test]
    fn transparency_composites_against_the_canvas_when_there_is_one() {
        let mut image = RgbaImage::new(2, 2);
        for pixel in image.pixels_mut() {
            *pixel = Rgba([255, 255, 255, 128]);
        }
        let raster = Raster::new(&DynamicImage::ImageRgba8(image), 1, 1);

        let Color::Rgb(r, _, _) = raster.cell(0, 0, Some([0, 0, 0])).0 else {
            panic!("expected an rgb color");
        };
        assert!((120..=140).contains(&r), "half of white over black is gray, got {r}");
        assert_eq!(raster.cell(0, 0, None).0, Color::Rgb(255, 255, 255), "no canvas, no blend");
    }

    /// A theme that leaves the background to the terminal has no number to blend with.
    #[test]
    fn only_a_numeric_background_can_be_composited_over() {
        assert_eq!(canvas_rgb(Color::Rgb(1, 2, 3)), Some([1, 2, 3]));
        assert_eq!(canvas_rgb(Color::Black), Some([0, 0, 0]));
        assert_eq!(canvas_rgb(Color::Reset), None);
        assert_eq!(canvas_rgb(Color::Indexed(5)), None);
    }
}
