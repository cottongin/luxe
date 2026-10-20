//! Tile — the mark's square tile as a bitmap, for a phone's home screen.
//!
//! The same tile `examples/wordmark_svg.rs` writes as the website's favicon, from the same art: a
//! plate with the L on it, fit by height inside a sixteenth of padding and centered across. A home
//! screen takes only bitmaps, so this one is drawn to pixels at the size asked for.

use super::Wordmark;
use image::{ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

/// A cell's width and height in drawing units, at the 1:2.2 a terminal draws, as the SVG twin has
/// them: the mark is recognized at those proportions.
const CELL_W: f64 = 10.0;
const CELL_H: f64 = 22.0;

/// The padding around the letter, as a share of the side: 32 of the favicon's 512.
const PAD_SHARE: f64 = 1.0 / 16.0;

/// The tile, `side` pixels square, with the L in `ink` on a `plate`, as PNG bytes.
pub fn tile_png(side: u32, plate: [u8; 3], ink: [u8; 3]) -> Result<Vec<u8>, image::ImageError> {
    let mut canvas = RgbImage::from_pixel(side, side, Rgb(plate));
    let cells = letter_cells();
    let fit = Fit::new(side, &cells);
    for cell in &cells {
        paint(&mut canvas, fit.pixels(cell), Rgb(ink));
    }
    let mut png = Vec::new();
    canvas.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(png)
}

/// One inked cell of the letter, in drawing units: where it starts and how tall its ink is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    x: f64,
    y: f64,
    height: f64,
}

/// The L's cells: a full block inks its cell, a half block the top or bottom half of it.
fn letter_cells() -> Vec<Cell> {
    let art = Wordmark::Full;
    let in_letter = |column: usize| art.groups().get(column) == Some(&0);
    let mut cells = Vec::new();
    for (row, text) in art.rows().iter().enumerate() {
        for (column, glyph) in text.chars().enumerate().filter(|(column, _)| in_letter(*column)) {
            let (top, share) = match glyph {
                '\u{2588}' => (0.0, 1.0),
                '\u{2580}' => (0.0, 0.5),
                '\u{2584}' => (0.5, 0.5),
                _ => continue,
            };
            let (x, y) = (column as f64 * CELL_W, (row as f64 + top) * CELL_H);
            cells.push(Cell { x, y, height: share * CELL_H });
        }
    }
    cells
}

/// How the letter's drawing units land on the tile's pixels.
struct Fit {
    scale: f64,
    left: f64,
    top: f64,
}

impl Fit {
    /// Fit the art's height, the letter's taller axis, then center the ink across.
    fn new(side: u32, cells: &[Cell]) -> Self {
        let side = f64::from(side);
        let pad = side * PAD_SHARE;
        let height = Wordmark::Full.rows().len() as f64 * CELL_H;
        let left = cells.iter().map(|cell| cell.x).fold(f64::INFINITY, f64::min);
        let right = cells.iter().map(|cell| cell.x + CELL_W).fold(0.0, f64::max);
        let scale = (side - pad * 2.0) / height;
        Self { scale, left: (side - (right - left) * scale) / 2.0 - left * scale, top: pad }
    }

    /// The pixel columns and rows `cell` covers. Edges round the same way on both sides of a
    /// boundary, so neighbors meet with no seam between them.
    fn pixels(&self, cell: &Cell) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
        let at = |units: f64, origin: f64| (origin + units * self.scale).round() as u32;
        let columns = at(cell.x, self.left)..at(cell.x + CELL_W, self.left);
        let rows = at(cell.y, self.top)..at(cell.y + cell.height, self.top);
        (columns, rows)
    }
}

fn paint(
    canvas: &mut RgbImage,
    (columns, rows): (std::ops::Range<u32>, std::ops::Range<u32>),
    ink: Rgb<u8>,
) {
    for y in rows {
        for x in columns.clone() {
            if x < canvas.width() && y < canvas.height() {
                canvas.put_pixel(x, y, ink);
            }
        }
    }
}

#[cfg(test)]
mod tests;
