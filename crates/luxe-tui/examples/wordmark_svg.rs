//! wordmark_svg — the logotype as vector, generated from the art that ships.
//!
//! The web needs the same mark the terminal draws, and the only honest way to get one is to derive
//! it from the same rows rather than trace it: `art.rs` is the single source, and a traced copy is
//! a second drawing that silently disagrees the first time either moves.
//!
//! Each cell becomes geometry — the full block a rectangle, the half blocks the half of the cell
//! they ink — and horizontal runs merge so a row of blocks is one rectangle rather than fourteen.
//! Output is one `<path>` per ink group, so the brass steps the TUI paints have somewhere to land in
//! CSS.
//!
//! ```sh
//! cargo run -p luxe-tui --example wordmark_svg -- wordmark > web/src/assets/wordmark.svg
//! cargo run -p luxe-tui --example wordmark_svg -- mark     > web/src/assets/mark.svg
//! cargo run -p luxe-tui --example wordmark_svg -- tile     > web/src/assets/tile.svg
//! ```

use luxe_tui::widgets::Wordmark;
use std::env;

/// Cell size in user units, at the 1:2.2 aspect a terminal actually draws — the mark is recognized
/// at those proportions, so an idealised 1:2 grid would ship a subtly different logotype.
const CELL_W: u32 = 10;
const CELL_H: u32 = 22;

fn main() {
    let mut args = env::args().skip(1);
    let target = args.next().unwrap_or_else(|| "wordmark".into());
    let rows = Wordmark::Full.rows();

    match target.as_str() {
        "wordmark" => print!("{}", svg(rows, 0..Wordmark::Full.group_count())),
        "mark" => print!("{}", svg(rows, 0..1)),
        "app" => print!("{}", app_svg(rows)),
        "tile" => print!("{}", tile(rows, tile_colors())),
        other => {
            eprintln!("usage: wordmark_svg [wordmark|mark|app|tile]  (got {other:?})");
            std::process::exit(2);
        }
    }
}

/// Side of the square tile, in user units. 512 exports cleanly to every favicon size.
const TILE: u32 = 512;
/// Air between the letter and the tile's edge. Little of it: a thin letter on a dark tile has to
/// fill the tile to stay a letter at 16px, and more margin left a blur in the tab strip.
const TILE_PAD: u32 = 32;

/// The tile's plate and letter, as colors written into the file: the luxe theme's canvas, and the
/// letter in its accent, the brass the wordmark is lit in.
///
/// Written in rather than left to page variables. A favicon is drawn as an image, where no page CSS
/// reaches, so `var(--luxe-wordmark-4, currentColor)` fell back to black under a near-black letter
/// and the tab showed an empty dark square.
fn tile_colors() -> (String, String) {
    let theme = luxe_core::theme::ThemeRegistry::new().resolve("luxe").expect("a built-in theme");
    let hex = |token: &str| match theme.get(token) {
        Some(luxe_core::theme::ColorValue::Rgb(red, green, blue)) => {
            format!("#{red:02x}{green:02x}{blue:02x}")
        }
        other => panic!("the luxe theme's {token} is a color, not {other:?}"),
    };
    (hex("background"), hex("accent"))
}

/// The favicon: a square dark tile with the L in brass.
///
/// The wordmark does not survive a 16px tab strip and neither does one bare letter — the L alone is
/// thin, tall and mostly air. What *is* legible at that size is a solid shape, so the mark becomes
/// a plain square, the same flat block the product draws its labels on. The letter is there for
/// anyone looking at it larger than a tab.
fn tile(rows: &[&str], (plate_color, ink): (String, String)) -> String {
    let side = TILE;
    let plate = format!("M0 0H{side}V{side}H0z");

    // Fit the L by height, then center its ink: it is the taller axis, so height sets the scale.
    // The letter's first column is blank, so the ink, not the column span, is what gets centered.
    let ink_left = first_inked_column(rows, 0) * CELL_W;
    let letter_w = ink_right(rows, 0..1) * CELL_W - ink_left;
    let letter_h = rows.len() as u32 * CELL_H;
    let scale = f64::from(side - TILE_PAD * 2) / f64::from(letter_h);
    let offset_x =
        (f64::from(side) - f64::from(letter_w) * scale) / 2.0 - f64::from(ink_left) * scale;

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
         width=\"{side}\" height=\"{side}\" role=\"img\" aria-label=\"luxe\">\n  \
         <title>luxe</title>\n  \
         <path fill=\"{plate_color}\" d=\"{plate}\"/>\n  \
         <g transform=\"translate({offset_x:.2} {TILE_PAD}) scale({scale:.4})\">\n    \
         <path fill=\"{ink}\" d=\"{letter}\"/>\n  \
         </g>\n</svg>\n",
        letter = letter_path(rows, 0)
    )
}

/// The SVG for `letters` of the logotype, one path per letter.
fn svg(rows: &[&str], letters: std::ops::Range<usize>) -> String {
    let paths: Vec<String> = letters.clone().map(|letter| letter_path(rows, letter)).collect();

    // Crop to the ink actually emitted, so the `mark` variant is not the full logotype's box with
    // three empty letters of padding on the right.
    let left = first_column(letters.start) * CELL_W;
    let right = ink_right(rows, letters.clone()) * CELL_W;
    let (width, height) = (right - left, rows.len() as u32 * CELL_H);

    let mut out = String::new();
    // The cell rides along because the drawing is a grid, and the website's highlight is defined in
    // that grid the way the terminal's is — one cell right per row of lean. Deriving the cell from
    // the box instead means knowing how many rows the art has, which is a second copy of the art.
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{left} 0 {width} {height}\" \
         width=\"{width}\" height=\"{height}\" data-cell=\"{CELL_W} {CELL_H}\" \
         role=\"img\" aria-label=\"luxe\">\n"
    ));
    out.push_str("  <title>luxe</title>\n");
    for (index, path) in paths.iter().enumerate() {
        let step = index + letters.start + 1;
        out.push_str(&format!(
            "  <path fill=\"var(--luxe-wordmark-{step}, currentColor)\" d=\"{path}\"/>\n"
        ));
    }
    out.push_str("</svg>\n");
    out
}

/// The phone app's copy: the wordmark, plus how deep the drift swings.
///
/// The app's colors come from core's theme CSS, and the depth is the terminal's, so the file is
/// the one way it reaches the app without a second copy of the number.
fn app_svg(rows: &[&str]) -> String {
    let depth = format!(" data-drift-depth=\"{}\" role=", luxe_tui::widgets::DRIFT_DEPTH);
    svg(rows, 0..Wordmark::Full.group_count()).replacen(" role=", &depth, 1)
}

/// The first column of ink group `group`.
fn first_column(group: usize) -> u32 {
    let groups = Wordmark::Full.groups();
    groups.iter().position(|owner| *owner == group).unwrap_or(0) as u32
}

/// The columns ink group `group` spans.
fn group_span(group: usize) -> std::ops::Range<usize> {
    let groups = Wordmark::Full.groups();
    let first = groups.iter().position(|owner| *owner == group).unwrap_or(0);
    let last = groups.iter().rposition(|owner| *owner == group).unwrap_or(first);
    first..last + 1
}

/// The leftmost inked column of ink group `group`.
fn first_inked_column(rows: &[&str], group: usize) -> u32 {
    let span = group_span(group);
    rows.iter()
        .flat_map(|row| {
            row.chars()
                .enumerate()
                .filter(|(column, glyph)| *glyph != ' ' && span.contains(column))
                .map(|(column, _)| column as u32)
        })
        .min()
        .unwrap_or(0)
}

/// One column past the rightmost inked cell in `letters`.
fn ink_right(rows: &[&str], letters: std::ops::Range<usize>) -> u32 {
    let span = group_span(letters.start).start..group_span(letters.end - 1).end;
    rows.iter()
        .flat_map(|row| {
            row.chars()
                .enumerate()
                .filter(|(column, glyph)| *glyph != ' ' && span.contains(column))
                .map(|(column, _)| column as u32 + 1)
        })
        .max()
        .unwrap_or(0)
}

/// Every inked cell of one letter, as a single path's worth of subpaths.
fn letter_path(rows: &[&str], letter: usize) -> String {
    let span = group_span(letter);
    let mut parts: Vec<String> = Vec::new();

    for (row, text) in rows.iter().enumerate() {
        let cells: Vec<(usize, char)> =
            text.chars().enumerate().filter(|(column, _)| span.contains(column)).collect();

        // Merge horizontal runs of full blocks into one rectangle each; a half block breaks the run
        // because it inks only part of its cell, and emits its own.
        let mut run: Option<(usize, usize)> = None;
        let flush = |run: &mut Option<(usize, usize)>, parts: &mut Vec<String>| {
            if let Some((start, end)) = run.take() {
                parts.push(rect(start as u32, row as u32, (end - start + 1) as u32, 0, 1.0));
            }
        };
        for (column, glyph) in cells {
            match glyph {
                '\u{2588}' => match run {
                    Some((start, end)) if end + 1 == column => run = Some((start, column)),
                    _ => {
                        flush(&mut run, &mut parts);
                        run = Some((column, column));
                    }
                },
                // The half blocks: the same rectangle at half height, from one edge or the other.
                '\u{2580}' | '\u{2584}' => {
                    flush(&mut run, &mut parts);
                    let top = u32::from(glyph == '\u{2584}');
                    parts.push(rect(column as u32, row as u32, 1, top, 0.5));
                }
                _ => flush(&mut run, &mut parts),
            }
        }
        flush(&mut run, &mut parts);
    }
    parts.join(" ")
}

/// A run of `cells` cells at `(column, row)`, inked for `share` of the cell's height.
///
/// `half` shifts the rectangle down by half a cell, which is the whole difference between the upper
/// and lower half block. Fractions rather than whole cells because that is what these glyphs *are*:
/// the reason the art needs no font is that a terminal draws them at exact cell fractions, and the
/// vector twin keeps the same promise rather than rounding it.
fn rect(column: u32, row: u32, cells: u32, half: u32, share: f32) -> String {
    let x = column * CELL_W;
    let y = row * CELL_H + half * CELL_H / 2;
    let w = cells * CELL_W;
    let h = (f64::from(CELL_H) * f64::from(share)).round() as u32;
    format!("M{x} {y}h{w}v{h}h-{w}z")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The groups this generator slices by are the art's own, and they tile it without gaps or
    /// overlaps — the one way a letter can silently lose half of itself.
    #[test]
    fn the_groups_tile_the_art() {
        let rows = Wordmark::Full.rows();
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|row| row.chars().count() == width), "ragged art");

        let mut covered = 0usize;
        let mut previous_end = 0usize;
        for group in 0..Wordmark::Full.group_count() {
            let span = group_span(group);
            assert!(span.start >= previous_end, "group {group} overlaps the one before it");
            previous_end = span.end;
            covered += span.len();
        }
        assert!(covered <= width, "the groups claim more columns than the art has");
    }

    /// Every inked cell reaches the output. A dropped glyph is a hole in the mark, and the art's own
    /// guard only checks the art — not that this generator understood all of it.
    #[test]
    fn every_inked_cell_becomes_geometry() {
        let rows = Wordmark::Full.rows();
        let inked = rows.iter().flat_map(|row| row.chars()).filter(|glyph| *glyph != ' ').count();
        let emitted: usize = (0..Wordmark::Full.group_count())
            .map(|letter| {
                let path = letter_path(rows, letter);
                // Each subpath starts with `M`; a merged run stands for its whole width.
                path.split('M').skip(1).map(cells_in_subpath).sum::<usize>()
            })
            .sum();
        assert_eq!(emitted, inked, "the vector twin lost cells the art draws");
    }

    /// How many cells one subpath accounts for: a merged block run by its width, a half block as
    /// one.
    fn cells_in_subpath(part: &str) -> usize {
        match part.split_once('h') {
            Some((_, rest)) => {
                let width: u32 = rest
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(CELL_W);
                (width / CELL_W) as usize
            }
            None => 1,
        }
    }

    /// The generated document is well-formed enough to be trusted without opening a browser.
    #[test]
    fn the_document_is_shaped_like_an_svg() {
        let svg = svg(Wordmark::Full.rows(), 0..4);
        assert!(svg.starts_with("<svg xmlns="));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<path").count(), 4, "one path per letter");
        assert!(svg.contains("--luxe-wordmark-1"), "letters are themable from CSS");
        assert!(svg.contains("aria-label=\"luxe\""));
        assert!(svg.contains("data-cell=\"10 22\""), "the grid the highlight is drawn in");
    }

    /// The app's copy is the wordmark with the drift's depth on its root.
    ///
    /// The break this catches: the attribute left off, or put somewhere other than the `<svg>` tag,
    /// which leaves the app swinging by a guess.
    #[test]
    fn the_app_copy_carries_the_drift_depth() {
        let rows = Wordmark::Full.rows();
        let app = app_svg(rows);
        let root = app.lines().next().expect("a root tag");
        let depth = luxe_tui::widgets::DRIFT_DEPTH;
        assert!(root.contains(&format!("data-drift-depth=\"{depth}\"")), "{root}");
        let paths = |doc: &str| doc.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(paths(&app), paths(&svg(rows, 0..Wordmark::Full.group_count())));
    }

    /// The tile is a plain square and keeps the letter inside its own padding, which is what decides
    /// whether it survives a 16px tab strip.
    #[test]
    fn the_tile_is_a_plain_square() {
        let rows = Wordmark::Full.rows();
        let svg = tile(rows, tile_colors());
        assert!(svg.contains(&format!("viewBox=\"0 0 {TILE} {TILE}\"")), "the tile is square");
        assert_eq!(svg.matches("<path").count(), 2, "a plate and a letter");
        // Colors written in: a favicon is an image, and no page variable reaches it
        assert!(!svg.contains("var("), "{svg}");
        let (plate_color, ink) = tile_colors();
        assert!(
            svg.contains(&format!("fill=\"{plate_color}\""))
                && svg.contains(&format!("fill=\"{ink}\""))
        );
        assert_ne!(plate_color, ink);

        // Four corners and the close: no cut, no curve.
        let plate =
            svg.split("d=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("plate");
        let vertices = plate.chars().filter(|glyph| "MHVLz".contains(*glyph)).count();
        assert_eq!(vertices, 5, "a square: {plate}");
        assert!(!plate.contains('A') && !plate.contains('C'), "square, never rounded: {plate}");

        // The letter is scaled to its padding rather than overflowing the plate it sits on.
        let scale: f64 = svg
            .split("scale(")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .and_then(|raw| raw.parse().ok())
            .expect("a scale factor");
        let drawn = scale * f64::from(rows.len() as u32 * CELL_H);
        assert!(
            (drawn - f64::from(TILE - TILE_PAD * 2)).abs() < 1.0,
            "the letter fills its box exactly: {drawn}"
        );
    }

    /// The break this catches: the blank column the L's group starts with counted as part of the
    /// letter, which sets the ink half a column right of center.
    #[test]
    fn the_tile_centers_the_ink() {
        let rows = Wordmark::Full.rows();
        let svg = tile(rows, tile_colors());
        let offset: f64 = svg
            .split("translate(")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|raw| raw.parse().ok())
            .expect("an offset");
        let scale = f64::from(TILE - TILE_PAD * 2) / f64::from(rows.len() as u32 * CELL_H);
        let left = offset + f64::from(first_inked_column(rows, 0) * CELL_W) * scale;
        let right = f64::from(TILE) - (offset + f64::from(ink_right(rows, 0..1) * CELL_W) * scale);
        assert!((left - right).abs() < 0.1, "margins {left} and {right}");
    }

    /// The single-letter mark crops to its own ink instead of carrying three empty letters of
    /// padding.
    #[test]
    fn the_mark_crops_to_the_letter_it_draws() {
        let rows = Wordmark::Full.rows();
        let mark = svg(rows, 0..1);
        assert_eq!(mark.matches("<path").count(), 1);
        let full_width = (rows[0].chars().count() as u32 * CELL_W).to_string();
        assert!(!mark.contains(&format!("width=\"{full_width}\"")), "the mark is not the wordmark");
    }
}
