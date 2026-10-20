use super::*;

const PLATE: [u8; 3] = [0x12, 0x10, 0x0e];
const INK: [u8; 3] = [0xdd, 0xc0, 0x84];

fn drawn(side: u32) -> RgbImage {
    let png = tile_png(side, PLATE, INK).expect("the tile encodes");
    image::load_from_memory_with_format(&png, ImageFormat::Png).expect("a PNG").to_rgb8()
}

/// The columns that hold any ink, first and last.
fn inked_columns(canvas: &RgbImage) -> (u32, u32) {
    let inked: Vec<u32> = (0..canvas.width())
        .filter(|&x| (0..canvas.height()).any(|y| canvas.get_pixel(x, y).0 == INK))
        .collect();
    (inked[0], *inked.last().expect("some ink"))
}

/// The break this catches: the size asked for not being the size drawn, which a home screen
/// scales into a blur.
#[test]
fn the_tile_is_drawn_at_the_size_asked_for() {
    for side in [180, 192, 512] {
        let canvas = drawn(side);
        assert_eq!((canvas.width(), canvas.height()), (side, side));
        assert_eq!(canvas.get_pixel(0, 0).0, PLATE, "a corner is the plate at {side}");
    }
}

/// The break this catches: the letter scaled past its padding, or set off center, so the icon
/// crops it or leans to one side.
#[test]
fn the_letter_sits_inside_its_padding_and_centered() {
    let canvas = drawn(512);
    let pad = 32;
    let row_has_ink = |y: u32| (0..512).any(|x| canvas.get_pixel(x, y).0 == INK);
    assert!(!(0..pad).any(row_has_ink), "ink in the top padding");
    assert!(!(512 - pad..512).any(row_has_ink), "ink in the bottom padding");
    let (first, last) = inked_columns(&canvas);
    let (left, right) = (first, 511 - last);
    assert!(left.abs_diff(right) <= 1, "margins {left} and {right}");
}

/// The break this catches: the letter scaled to other than the favicon's 32-of-512 padding, so
/// the home screen's L is a different size from the browser tab's.
#[test]
fn the_art_fills_the_favicons_box() {
    let canvas = drawn(512);
    let rows = Wordmark::Full.rows().len() as f64;
    let scale = (512.0 - 2.0 * 32.0) / (rows * CELL_H);
    let cells = letter_cells();
    let top = cells.iter().map(|cell| cell.y).fold(f64::INFINITY, f64::min);
    let first_inked_row = (0..512).find(|&y| (0..512).any(|x| canvas.get_pixel(x, y).0 == INK));
    assert_eq!(first_inked_row, Some((32.0 + top * scale).round() as u32));
}

/// The break this catches: a glyph the art draws with missing from the tile, a half block drawn
/// whole, or the upper and lower halves crossed. Each of the letter's cells is read in the art
/// and sampled in both halves of where it lands.
#[test]
fn every_cell_of_the_letter_is_drawn_as_the_art_has_it() {
    let canvas = drawn(512);
    let fit = Fit::new(512, &letter_cells());
    let art = Wordmark::Full;
    for (row, text) in art.rows().iter().enumerate() {
        for (column, glyph) in text.chars().enumerate() {
            if art.groups().get(column) != Some(&0) {
                continue;
            }
            let drawn = (
                inked(&canvas, &fit, column, row as f64),
                inked(&canvas, &fit, column, row as f64 + 0.5),
            );
            assert_eq!(drawn, halves(glyph), "{glyph:?} at row {row}, column {column}");
        }
    }
}

/// Which halves of its cell `glyph` inks, top then bottom.
fn halves(glyph: char) -> (bool, bool) {
    match glyph {
        '\u{2588}' => (true, true),
        '\u{2580}' => (true, false),
        '\u{2584}' => (false, true),
        _ => (false, false),
    }
}

/// Whether the middle of the half cell at `column` and `row` (in rows, a half step for the
/// bottom) is drawn in ink.
fn inked(canvas: &RgbImage, fit: &Fit, column: usize, row: f64) -> bool {
    let cell = Cell { x: column as f64 * CELL_W, y: row * CELL_H, height: CELL_H / 2.0 };
    let (columns, rows) = fit.pixels(&cell);
    let middle = |range: std::ops::Range<u32>| (range.start + range.end) / 2;
    canvas.get_pixel(middle(columns), middle(rows)).0 == INK
}

/// The break this catches: the L read from the wrong columns, or a kind of block skipped, so the
/// tile draws a different letter than the terminal and the favicon do.
#[test]
fn the_letter_is_every_inked_cell_of_the_first_group() {
    let art = Wordmark::Full;
    let inked = art
        .rows()
        .iter()
        .flat_map(|row| row.chars().enumerate())
        .filter(|(column, glyph)| art.groups().get(*column) == Some(&0) && *glyph != ' ')
        .count();
    let cells = letter_cells();
    assert_eq!(cells.len(), inked);
    assert!(cells.iter().any(|cell| cell.height < CELL_H), "the L has a half block");
}
