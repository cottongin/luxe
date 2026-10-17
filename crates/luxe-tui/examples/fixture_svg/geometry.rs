//! Glyph geometry — block, box and triangle glyphs drawn as exact cell fractions, not typeset.

use super::color::hex;
use super::{CELL_H, CELL_W, HAIRLINE};
use ratatui::buffer::Buffer;
use std::fmt::Write as _;

/// Every cell whose glyph is drawn rather than typeset.
pub(super) fn geometry(buffer: &Buffer) -> String {
    let mut out = String::new();
    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            let Some(glyph) = cell.symbol().chars().next() else { continue };
            let Some(fill) = hex(cell.fg) else { continue };
            if let Some(drawn) = draw(glyph, column, row, &fill) {
                let _ = writeln!(out, "    {drawn}");
            }
        }
    }
    out
}

/// Whether this glyph is drawn as geometry rather than typeset.
pub(super) fn is_drawn(glyph: char) -> bool {
    block(glyph).is_some() || arms(glyph).is_some() || triangle_points(glyph).is_some()
}

/// The SVG element for a drawn glyph, or `None` when the font should handle it.
pub(super) fn draw(glyph: char, column: u16, row: u16, fill: &str) -> Option<String> {
    let (x, y) = (f64::from(column) * CELL_W, f64::from(row) * CELL_H);
    if let Some(parts) = block(glyph) {
        return Some(block_element(&parts, x, y, fill));
    }
    if let Some(points) = triangle_points(glyph) {
        return Some(triangle(&points, x, y, fill));
    }
    arms(glyph).map(|(arms, rounded)| box_line(arms, rounded, x, y, fill))
}

/// A block glyph as `(fractional rects, opacity)`, in cell-relative units.
///
/// Rects are `(x, y, w, h)` with the cell as the unit square. The shade blocks have no geometry of
/// their own — they are the full cell at a fraction of its ink, which is what they mean.
type Block = (Vec<(f64, f64, f64, f64)>, f64);

pub(super) fn block(glyph: char) -> Option<Block> {
    let full = vec![(0.0, 0.0, 1.0, 1.0)];
    let eighth = |n: f64| n / 8.0;
    Some(match glyph {
        '\u{2588}' => (full, 1.0),
        // Shades: the same cell, thinned.
        '\u{2591}' => (full, 0.25),
        '\u{2592}' => (full, 0.5),
        '\u{2593}' => (full, 0.75),
        // Halves.
        '\u{2580}' => (vec![(0.0, 0.0, 1.0, 0.5)], 1.0),
        '\u{2584}' => (vec![(0.0, 0.5, 1.0, 0.5)], 1.0),
        '\u{258c}' => (vec![(0.0, 0.0, 0.5, 1.0)], 1.0),
        '\u{2590}' => (vec![(0.5, 0.0, 0.5, 1.0)], 1.0),
        // Lower eighths, one through seven.
        '\u{2581}'..='\u{2587}' => {
            let height = eighth(f64::from(glyph as u32 - 0x2580));
            (vec![(0.0, 1.0 - height, 1.0, height)], 1.0)
        }
        // Left eighths run the other way: U+2589 is seven eighths, U+258F is one.
        '\u{2589}'..='\u{258f}' => {
            let width = eighth(f64::from(0x2590 - glyph as u32));
            (vec![(0.0, 0.0, width, 1.0)], 1.0)
        }
        '\u{2594}' => (vec![(0.0, 0.0, 1.0, eighth(1.0))], 1.0),
        '\u{2595}' => (vec![(1.0 - eighth(1.0), 0.0, eighth(1.0), 1.0)], 1.0),
        // Quadrants, as a bitmask of the four sub-cells.
        '\u{2596}'..='\u{259f}' => {
            let mask = QUADRANTS[glyph as usize - 0x2596];
            let mut rects = Vec::new();
            for (bit, (qx, qy)) in
                [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)].into_iter().enumerate()
            {
                if mask & (1 << bit) != 0 {
                    rects.push((qx, qy, 0.5, 0.5));
                }
            }
            (rects, 1.0)
        }
        _ => return None,
    })
}

/// Quadrant fills for `U+2596..=U+259F`, as `upper-left | upper-right | lower-left | lower-right`.
const QUADRANTS: [u8; 10] = [
    0b0100, // ▖ lower left
    0b1000, // ▗ lower right
    0b0001, // ▘ upper left
    0b1101, // ▙
    0b1001, // ▚
    0b0111, // ▛
    0b1011, // ▜
    0b0010, // ▝ upper right
    0b0110, // ▞
    0b1110, // ▟
];

fn block_element(block: &Block, x: f64, y: f64, fill: &str) -> String {
    let (parts, opacity) = block;
    let body: String = parts
        .iter()
        .map(|(rx, ry, rw, rh)| {
            format!(
                "M{:.2} {:.2}h{:.2}v{:.2}h-{:.2}z",
                x + rx * CELL_W,
                y + ry * CELL_H,
                rw * CELL_W,
                rh * CELL_H,
                rw * CELL_W
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let alpha = if *opacity < 1.0 { format!(" fill-opacity=\"{opacity}\"") } else { String::new() };
    format!("<path fill=\"{fill}\"{alpha} d=\"{body}\"/>")
}

/// Which cell edges a box-drawing glyph reaches, and whether its corner is rounded.
///
/// `(up, right, down, left)`. Only the light set: it is what Luxe draws with, and a heavy or double
/// rule would need its own stroke width rather than a flag here.
type Arms = (bool, bool, bool, bool);

fn arms(glyph: char) -> Option<(Arms, bool)> {
    let sharp = |arms: Arms| Some((arms, false));
    match glyph {
        '\u{2500}' => sharp((false, true, false, true)),
        '\u{2502}' => sharp((true, false, true, false)),
        '\u{250c}' => sharp((false, true, true, false)),
        '\u{2510}' => sharp((false, false, true, true)),
        '\u{2514}' => sharp((true, true, false, false)),
        '\u{2518}' => sharp((true, false, false, true)),
        '\u{251c}' => sharp((true, true, true, false)),
        '\u{2524}' => sharp((true, false, true, true)),
        '\u{252c}' => sharp((false, true, true, true)),
        '\u{2534}' => sharp((true, true, false, true)),
        '\u{253c}' => sharp((true, true, true, true)),
        '\u{256d}' => Some(((false, true, true, false), true)),
        '\u{256e}' => Some(((false, false, true, true), true)),
        '\u{256f}' => Some(((true, false, false, true), true)),
        '\u{2570}' => Some(((true, true, false, false), true)),
        _ => None,
    }
}

/// A box-drawing glyph as a stroked path.
///
/// A rounded corner is one quadratic curve between the two edge midpoints, with the cell's center
/// as its control point — which is exactly the quarter-round the glyph means, with no radius to
/// pick.
fn box_line((up, right, down, left): Arms, rounded: bool, x: f64, y: f64, fill: &str) -> String {
    let (cx, cy) = (x + CELL_W / 2.0, y + CELL_H / 2.0);
    let (x1, y1) = (x + CELL_W, y + CELL_H);
    let mut path = String::new();

    if rounded {
        let (from_x, from_y) = if up { (cx, y) } else { (cx, y1) };
        let (to_x, to_y) = if left { (x, cy) } else { (x1, cy) };
        let _ = write!(path, "M{from_x:.2} {from_y:.2}Q{cx:.2} {cy:.2} {to_x:.2} {to_y:.2}");
    } else {
        for (present, (ex, ey)) in
            [(up, (cx, y)), (right, (x1, cy)), (down, (cx, y1)), (left, (x, cy))]
        {
            if present {
                let _ = write!(path, "M{cx:.2} {cy:.2}L{ex:.2} {ey:.2}");
            }
        }
    }
    format!("<path fill=\"none\" stroke=\"{fill}\" stroke-width=\"{HAIRLINE:.2}\" d=\"{path}\"/>")
}

/// The half-cell a Powerline Extra triangle inks, in cell-relative units.
fn triangle_points(glyph: char) -> Option<[(f64, f64); 3]> {
    Some(match glyph {
        '\u{e0b8}' => [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)], // lower left
        '\u{e0ba}' => [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], // lower right
        '\u{e0bc}' => [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], // upper left
        '\u{e0be}' => [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], // upper right
        _ => return None,
    })
}

fn triangle(points: &[(f64, f64); 3], x: f64, y: f64, fill: &str) -> String {
    let path: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(index, (px, py))| {
            let verb = if index == 0 { 'M' } else { 'L' };
            format!("{verb}{:.2} {:.2}", x + px * CELL_W, y + py * CELL_H)
        })
        .collect();
    format!("<path fill=\"{fill}\" d=\"{}z\"/>", path.join(""))
}

pub(super) fn rect(column: u16, row: u16, cells: u16, lines: u16, fill: &str) -> String {
    format!(
        "<rect x=\"{:.0}\" y=\"{:.0}\" width=\"{:.0}\" height=\"{:.0}\" fill=\"{fill}\"/>",
        f64::from(column) * CELL_W,
        f64::from(row) * CELL_H,
        f64::from(cells) * CELL_W,
        f64::from(lines) * CELL_H
    )
}
