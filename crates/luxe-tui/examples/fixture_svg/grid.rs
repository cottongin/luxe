//! Grid input — a terminal grid handed in on stdin, read into the buffer the exporter draws.

use super::fail;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// One cell of a grid handed in on stdin.
#[derive(serde::Deserialize)]
struct GridCell {
    /// The character. Empty for the second half of a wide glyph.
    c: String,
    /// `#rrggbb`, or absent for the terminal's default.
    #[serde(default)]
    fg: Option<String>,
    #[serde(default)]
    bg: Option<String>,
    #[serde(default)]
    bold: bool,
}

#[derive(serde::Deserialize)]
struct Grid {
    width: u16,
    height: u16,
    /// Row-major, `width * height` of them.
    cells: Vec<GridCell>,
}

/// Reads a grid from stdin into the buffer the exporter already knows how to draw.
pub(super) fn buffer_from_grid() -> Buffer {
    let grid: Grid = serde_json::from_reader(std::io::stdin().lock())
        .unwrap_or_else(|error| fail(&format!("could not read the grid: {error}")));
    let expected = usize::from(grid.width) * usize::from(grid.height);
    if grid.cells.len() != expected {
        fail(&format!(
            "grid says {}x{} but carries {} cells",
            grid.width,
            grid.height,
            grid.cells.len()
        ));
    }

    let area = ratatui::layout::Rect::new(0, 0, grid.width, grid.height);
    let mut buffer = Buffer::empty(area);
    for (index, cell) in grid.cells.iter().enumerate() {
        let (x, y) = (index % usize::from(grid.width), index / usize::from(grid.width));
        let target = &mut buffer[(x as u16, y as u16)];
        // A wide glyph's trailing cell carries no character of its own; leaving it as a space keeps
        // the columns after it where the emulator put them.
        target.set_symbol(if cell.c.is_empty() { " " } else { &cell.c });
        if let Some(color) = cell.fg.as_deref().and_then(parse_hex) {
            target.set_fg(color);
        }
        if let Some(color) = cell.bg.as_deref().and_then(parse_hex) {
            target.set_bg(color);
        }
        if cell.bold {
            target.set_style(ratatui::style::Style::default().add_modifier(Modifier::BOLD));
        }
    }
    buffer
}

/// `#rrggbb` to a ratatui color.
pub(super) fn parse_hex(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#')?;
    // ASCII first, so the channel slices below always land on character boundaries
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Color::Rgb(channel(0)?, channel(2)?, channel(4)?))
}
