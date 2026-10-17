//! fixture_svg — a production render, exported as vector.
//!
//! The site needs pictures of Luxe, and a screenshot is the worst way to get one: it is a raster at
//! somebody's DPI, its text cannot be selected or searched, it weighs ten times what it should, and
//! it puts a terminal and a font in the pipeline where neither belongs. The render is already a
//! grid of glyphs and colors, so it can be drawn directly.
//!
//! A scenario from [`luxe_tui::fixture`] renders through the **production view path** into a
//! `TestBackend` buffer, and each cell becomes geometry or text:
//!
//! - **Block Elements** (full, halves, eighths, quadrants, shades) become rectangles. These are
//!   defined as exact cell fractions, which is precisely what a font renderer approximates and a
//!   rectangle does not.
//! - **Box drawing** (light lines and rounded corners) becomes stroked paths, for the same reason
//!   plus one more: a fallback font leaves gaps between border segments, and a panel drawn with
//!   gaps in it is worse than no panel.
//! - **Powerline Extra triangles** become triangles — the range terminals disagree about most.
//! - **Everything else** stays real text, one `<text>` per row, so a reader can select a command
//!   out of the picture and run it.
//!
//! Every text row carries `textLength`, so the grid holds under whatever monospace font the reader
//! actually has rather than only under ours.
//!
//! ```sh
//! cargo run -p luxe-tui --example fixture_svg -- --theme luxe --scenario chat-tools > shot.svg
//! cargo run -p luxe-tui --example fixture_svg -- --census        # what is still drawn as text
//! ```

mod capture;
mod color;
mod geometry;
mod grid;
mod palette;
mod svg;

use capture::render;
use grid::buffer_from_grid;
use luxe_tui::fixture::FixtureScenario;
use palette::palette_svg;
use svg::{svg, text_census};

/// Cell size in user units, at the 1:2.2 aspect a terminal draws — the same grid the wordmark's
/// vector twin uses, so a logotype exported here lines up with the standalone one.
const CELL_W: f64 = 10.0;
const CELL_H: f64 = 22.0;

/// The face's advance width, as a fraction of its type size.
///
/// This has to be the *real* figure for the first font in the stack below, because every row is
/// stretched to `cols × CELL_W` by `textLength`: guess low and every glyph is squeezed, guess high
/// and they are pulled apart, in both cases silently and across every capture at once. JetBrains
/// Mono advances 0.6em. `scripts/site-smoke.mjs` measures what the website's own face actually
/// advances and fails on the difference.
const ADVANCE: f64 = 0.6;

/// Type size that fills the cell at [`ADVANCE`]. `textLength` corrects the rest.
const FONT_SIZE: f64 = CELL_W / ADVANCE;
/// The face's ascender, as a fraction of its type size.
///
/// JetBrains Mono's line box is 1.02em up and 0.30em down, 1.32em in all, which at [`FONT_SIZE`]
/// is exactly [`CELL_H`] — so the baseline sits where a terminal drawing this face puts it.
const ASCENT: f64 = 1.02;
/// Baseline as a fraction of cell height.
const BASELINE: f64 = ASCENT * FONT_SIZE / CELL_H;
/// Stroke width of a light box-drawing line.
const HAIRLINE: f64 = CELL_W / 8.0;

fn main() {
    let mut theme = "luxe".to_string();
    let mut scenario = "chat-tools".to_string();
    let (mut cols, mut rows) = (150_u16, 40_u16);
    let mut census = false;
    let mut grid = false;
    let mut palette: Option<Vec<String>> = None;
    let mut palette_json: Option<String> = None;
    let mut overlay: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| fail(&format!("{flag} needs a value")));
        match flag.as_str() {
            "--theme" => theme = value(),
            "--scenario" => scenario = value(),
            "--cols" => cols = value().parse().unwrap_or_else(|_| fail("--cols wants a number")),
            "--rows" => rows = value().parse().unwrap_or_else(|_| fail("--rows wants a number")),
            "--census" => census = true,
            "--grid" => grid = true,
            "--palette" => {
                palette = Some(value().split(',').map(str::trim).map(str::to_owned).collect());
            }
            "--palette-json" => palette_json = Some(value()),
            "--overlay" => overlay = Some(value()),
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => fail(&format!("unknown flag {other}\n\n{USAGE}")),
        }
    }

    // The braille sweep, as the live app draws it wherever a font can: the site's webfonts carry all
    // ten frames, and a still of a running card in the four-corner fallback reads as broken
    luxe_tui::glyphs::set_braille(true);

    // A screen somebody else emulated, drawn by the same hand as a fixture.
    //
    // `scripts/cast-still.py` replays a recording with a real terminal emulator and sends the
    // resulting grid here rather than writing its own SVG. Box-drawing and block characters are
    // *geometry* in this exporter, not glyphs — which is the only way a vertical rule tiles down a
    // column instead of breaking into dashes at every row boundary — and a second implementation
    // of that in another language would drift from this one within a release.
    if grid {
        print!("{}", svg(&buffer_from_grid()));
        return;
    }

    let scenario: FixtureScenario = scenario.parse().unwrap_or_else(|error: String| fail(&error));

    if let Some(themes) = palette {
        let Some(json) = palette_json else { fail("--palette needs --palette-json FILE") };
        let (document, values) = palette_svg(&themes, scenario, cols, rows);
        let written = serde_json::to_string_pretty(&values).expect("a palette is plain strings");
        std::fs::write(&json, written + "\n")
            .unwrap_or_else(|error| fail(&format!("could not write {json}: {error}")));
        print!("{document}");
        return;
    }

    let buffer = render(&theme, scenario, overlay.as_deref(), cols, rows).0;

    if census {
        for (glyph, count) in text_census(&buffer) {
            println!("U+{:04X} {glyph}  {count}", glyph as u32);
        }
        return;
    }
    print!("{}", svg(&buffer));
}

const USAGE: &str = "usage: fixture_svg [--theme ID] [--scenario NAME] [--cols N] [--rows N] \
     [--overlay STATE] [--census] [--grid] [--palette ID,ID,... --palette-json FILE]";

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2)
}

#[cfg(test)]
mod tests;
