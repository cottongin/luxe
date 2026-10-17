//! The SVG document — its backgrounds, the drawn glyphs, and one selectable text row per line.

use super::color::{attributes, hex};
use super::geometry::{geometry, is_drawn, rect};
use super::{BASELINE, CELL_H, CELL_W, FONT_SIZE};
use ratatui::buffer::Buffer;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The whole document.
pub(super) fn svg(buffer: &Buffer) -> String {
    let (cols, rows) = (buffer.area.width, buffer.area.height);
    let (width, height) = (f64::from(cols) * CELL_W, f64::from(rows) * CELL_H);
    let canvas = canvas_color(buffer);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.0} {height:.0}\" \
         width=\"{width:.0}\" height=\"{height:.0}\" font-family=\"'JetBrainsMono Nerd Font Mono', \
         'AdwaitaMono Nerd Font Mono', 'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, \
         Consolas, monospace\" \
         font-size=\"{FONT_SIZE:.2}\">"
    );
    let _ =
        writeln!(out, "  <rect width=\"{width:.0}\" height=\"{height:.0}\" fill=\"{canvas}\"/>");

    out.push_str("  <g shape-rendering=\"crispEdges\">\n");
    out.push_str(&backgrounds(buffer, &canvas));
    out.push_str(&geometry(buffer));
    out.push_str("  </g>\n");

    out.push_str(&text(buffer));
    out.push_str("</svg>\n");
    out
}

/// The color behind everything, taken from the cell the app paints most of.
///
/// Reading it off the render rather than the theme keeps the export honest about surfaces that
/// bring their own canvas, and gives `Color::Reset` something real to resolve to.
pub(super) fn canvas_color(buffer: &Buffer) -> String {
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for cell in buffer.content() {
        if let Some(hex) = hex(cell.bg) {
            *tally.entry(hex).or_default() += 1;
        }
    }
    tally.into_iter().max_by_key(|(_, count)| *count).map(|(hex, _)| hex).unwrap_or_else(|| {
        // Nothing painted a background, so the terminal's own shows through. Dark is the safer
        // guess: it is what a terminal defaults to, and light text on it stays readable.
        "#000000".to_string()
    })
}

/// Merged background rectangles, skipping whatever already matches the canvas.
fn backgrounds(buffer: &Buffer, canvas: &str) -> String {
    let mut out = String::new();
    for row in 0..buffer.area.height {
        let mut run: Option<(u16, u16, String)> = None;
        let flush = |run: &mut Option<(u16, u16, String)>, out: &mut String| {
            if let Some((start, end, fill)) = run.take() {
                let _ = writeln!(out, "    {}", rect(start, row, end - start + 1, 1, &fill));
            }
        };
        for column in 0..buffer.area.width {
            let fill = hex(buffer[(column, row)].bg).filter(|fill| fill != canvas);
            match (&mut run, fill) {
                (Some((_, end, current)), Some(fill)) if *current == fill && *end + 1 == column => {
                    *end = column;
                }
                (_, Some(fill)) => {
                    flush(&mut run, &mut out);
                    run = Some((column, column, fill));
                }
                (_, None) => flush(&mut run, &mut out),
            }
        }
        flush(&mut run, &mut out);
    }
    out
}

/// One `<text>` per row, so selecting a line of the picture yields the line.
pub(super) fn text(buffer: &Buffer) -> String {
    let width = f64::from(buffer.area.width) * CELL_W;
    let mut out = String::new();

    for row in 0..buffer.area.height {
        let mut spans = String::new();
        let mut pending = String::new();
        let mut pending_style: Option<(String, String)> = None;

        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            let symbol = cell.symbol();
            let glyph = symbol.chars().next().unwrap_or(' ');
            // A drawn glyph leaves a space behind so the columns after it still line up, and so the
            // row does not paint the same mark twice. The whole symbol otherwise: an accent or a
            // joined emoji is several scalars in one cell, and an empty cell still takes a column.
            let typeset = if is_drawn(glyph) || symbol.is_empty() { " " } else { symbol };
            let style = (hex(cell.fg).unwrap_or_default(), attributes(cell.modifier));

            if pending_style.as_ref() != Some(&style) {
                flush_span(&mut spans, &mut pending, &pending_style);
                pending_style = Some(style);
            }
            pending.push_str(typeset);
        }
        flush_span(&mut spans, &mut pending, &pending_style);

        if spans.trim().is_empty() {
            continue;
        }
        let baseline = (f64::from(row) + BASELINE) * CELL_H;
        let _ = writeln!(
            out,
            "  <text x=\"0\" y=\"{baseline:.1}\" textLength=\"{width:.0}\" \
             lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\">{spans}</text>"
        );
    }
    out
}

fn flush_span(spans: &mut String, pending: &mut String, style: &Option<(String, String)>) {
    if pending.is_empty() {
        return;
    }
    let text = std::mem::take(pending);
    // A run of pure whitespace still has to occupy its columns, but needs no fill or attributes.
    if text.trim().is_empty() {
        let _ = write!(spans, "<tspan>{}</tspan>", escape(&text));
        return;
    }
    let (fill, attrs) = style.clone().unwrap_or_default();
    let fill = if fill.is_empty() { String::new() } else { format!(" fill=\"{fill}\"") };
    let _ = write!(spans, "<tspan{fill}{attrs}>{}</tspan>", escape(&text));
}

/// Glyphs still reaching the font, most common first — the input to deciding what to draw next.
pub(super) fn text_census(buffer: &Buffer) -> Vec<(char, usize)> {
    let mut tally: BTreeMap<char, usize> = BTreeMap::new();
    for cell in buffer.content() {
        let glyph = cell.symbol().chars().next().unwrap_or(' ');
        if glyph != ' ' && !glyph.is_ascii() && !is_drawn(glyph) {
            *tally.entry(glyph).or_default() += 1;
        }
    }
    let mut census: Vec<(char, usize)> = tally.into_iter().collect();
    census.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    census
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
