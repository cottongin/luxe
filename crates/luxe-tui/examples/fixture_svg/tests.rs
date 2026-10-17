use super::geometry::{block, is_drawn};
use super::grid::parse_hex;
use super::palette::with_variables;
use super::svg::text;
use super::*;
use luxe_tui::fixture::FixtureScenario;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

fn shot() -> Buffer {
    render("luxe", FixtureScenario::ChatTools, None, 120, 30).0
}

/// One palette picture stands in for every theme: no placeholder survives into the document,
/// the fallbacks are the first theme's own colors, and every theme gets a value for every
/// variable the picture uses.
#[test]
fn a_palette_picture_names_every_color_it_draws() {
    let themes = ["luxe".to_string(), "nord".to_string()];
    let (document, values) = palette_svg(&themes, FixtureScenario::ChatTools, 120, 30);
    assert!(!document.contains("=\"#0f"), "a placeholder leaked into the picture");
    assert!(document.contains("fill=\"var(--p0, #"), "{}", &document[..400]);
    let used = document.matches("var(--p").count();
    assert!(used > 20, "the picture is drawn in variables: {used}");
    let (luxe, nord) = (&values["luxe"], &values["nord"]);
    assert_eq!(luxe.len(), nord.len());
    assert!(luxe.iter().any(|(key, value)| nord[key] != *value), "the themes differ");
    for (key, value) in luxe {
        assert!(document.contains(&format!("var(--{key}, {value})")), "{key} unused");
    }
}

/// Only attributes are rewritten: a hex color in the terminal's *text* is somebody's code.
#[test]
fn a_placeholder_in_the_text_is_left_alone() {
    let document = "<rect fill=\"#0f0001\"/><text>#0f0001</text>";
    let out = with_variables(document, &["#aaaaaa", "#bbbbbb"]);
    assert_eq!(out, "<rect fill=\"var(--p1, #bbbbbb)\"/><text>#0f0001</text>");
}

/// The export is a real render, not a blank grid — the cheapest way to catch a fixture that
/// stopped seeding itself.
#[test]
fn the_export_carries_the_render() {
    let svg = svg(&shot());
    assert!(svg.starts_with("<svg xmlns="));
    assert!(svg.trim_end().ends_with("</svg>"));
    assert!(svg.contains("<text "), "no typeset rows reached the document");
    assert!(svg.matches("<text ").count() > 5, "suspiciously few rows");
}

/// Text stays text. The whole reason not to ship a PNG is that a reader can select a command
/// out of the picture, so a run of real characters has to survive the trip.
#[test]
fn commands_in_the_render_are_still_selectable_text() {
    let svg = svg(&shot());
    assert!(svg.contains("cargo test"), "a command the fixture renders went missing");
    assert!(svg.contains("xml:space=\"preserve\""), "leading columns would collapse");
    assert!(svg.contains("lengthAdjust=\"spacingAndGlyphs\""), "the grid needs pinning");
}

/// Every glyph defined as an exact cell fraction is drawn rather than typeset. These are the
/// ones a font approximates, which is what makes a terminal picture look wrong.
#[test]
fn cell_fraction_glyphs_are_drawn_not_typeset() {
    for glyph in [
        '\u{2588}', '\u{2580}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2581}', '\u{2587}',
        '\u{258f}', '\u{2589}', '\u{2594}', '\u{2595}', '\u{2596}', '\u{259f}', '\u{2591}',
        '\u{2593}',
    ] {
        assert!(is_drawn(glyph), "U+{:04X} should be geometry", glyph as u32);
    }
    for glyph in ['\u{2500}', '\u{2502}', '\u{250c}', '\u{253c}', '\u{256d}', '\u{2570}'] {
        assert!(is_drawn(glyph), "U+{:04X} should be geometry", glyph as u32);
    }
    for glyph in ['\u{e0b8}', '\u{e0ba}', '\u{e0bc}', '\u{e0be}'] {
        assert!(is_drawn(glyph), "U+{:04X} should be geometry", glyph as u32);
    }
    // Letters and the geometric shapes Luxe uses as marks stay text: any font has them, and
    // they carry meaning that should remain selectable.
    for glyph in ['a', '\u{276f}', '\u{25cf}', '\u{25c6}'] {
        assert!(!is_drawn(glyph), "U+{:04X} should be typeset", glyph as u32);
    }
}

/// A drawn glyph leaves a space in its row, so the columns after it still land on the grid and
/// the mark is not painted twice.
#[test]
fn a_drawn_glyph_holds_its_column_in_the_text_layer() {
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 3, 1));
    buffer[(0, 0)].set_symbol("\u{2588}");
    buffer[(1, 0)].set_symbol("A");
    buffer[(2, 0)].set_symbol("B");

    let text = text(&buffer);
    assert!(text.contains("AB"), "typeset glyphs survive: {text}");
    assert!(!text.contains('\u{2588}'), "the block is drawn, not typeset: {text}");
    assert!(text.contains(' '), "the block still occupies its column: {text}");
}

/// Quadrant masks are transcribed correctly. Getting one wrong is invisible in review and
/// obvious on the page, so it is checked against the halves they must add up to.
#[test]
fn quadrant_masks_agree_with_the_halves_they_compose() {
    let area = |glyph: char| {
        block(glyph).expect("a block").0.iter().map(|(_, _, w, h)| w * h).sum::<f64>()
    };
    assert_eq!(area('\u{2596}'), 0.25, "one quadrant");
    assert_eq!(area('\u{259f}'), 0.75, "three quadrants");
    // ▘ upper-left plus ▗ lower-right is the same ink as ▚.
    assert_eq!(area('\u{2598}') + area('\u{2597}'), area('\u{259a}'));
    // Every eighth block covers the fraction it is named for.
    assert!((area('\u{2581}') - 0.125).abs() < f64::EPSILON, "one lower eighth");
    assert!((area('\u{2587}') - 0.875).abs() < f64::EPSILON, "seven lower eighths");
    assert!((area('\u{258f}') - 0.125).abs() < f64::EPSILON, "one left eighth");
    assert!((area('\u{2589}') - 0.875).abs() < f64::EPSILON, "seven left eighths");
}

/// Markup that would break a parser is escaped. A diff of HTML is an ordinary thing to render.
#[test]
fn markup_in_the_render_is_escaped() {
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 5, 1));
    for (index, glyph) in "<a&b".chars().enumerate() {
        buffer[(index as u16, 0)].set_symbol(&glyph.to_string());
    }
    let text = text(&buffer);
    assert!(text.contains("&lt;a&amp;b"), "unescaped markup would break the document: {text}");
}

/// A cell's whole symbol is typeset, and a wide glyph's trailing cell keeps its column, so the
/// marker after each lands where the terminal put it.
///
/// The break this catches: the text layer kept only a cell's first scalar, so a decomposed
/// accent lost its mark and a joined emoji lost everything after its first person.
#[test]
fn a_cell_keeps_its_whole_symbol_in_the_text_layer() {
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 8, 1));
    for (column, symbol) in
        ["e\u{301}", "X", "\u{1f469}\u{200d}\u{1f4bb}", " ", "Y", "\u{4e2d}", "", "Z"]
            .into_iter()
            .enumerate()
    {
        buffer[(column as u16, 0)].set_symbol(symbol);
    }
    let text = text(&buffer);
    assert!(
        text.contains("e\u{301}X\u{1f469}\u{200d}\u{1f4bb} Y\u{4e2d} Z"),
        "every scalar of every cell, one column each: {text}"
    );
}

/// A color that is not ASCII hex is refused, not sliced.
///
/// The break this catches: six bytes passed the length check and the channel slices cut a
/// multi-byte character in half, which panics on a grid handed in on stdin.
#[test]
fn non_ascii_grid_colors_are_rejected_without_panicking() {
    // six bytes each, with a channel boundary inside a three-byte character
    assert_eq!(parse_hex("#a\u{20ac}bc"), None);
    assert_eq!(parse_hex("#\u{e9}\u{20ac}a"), None);
    assert_eq!(parse_hex("#0a0B0c"), Some(Color::Rgb(0x0a, 0x0b, 0x0c)));
}

/// Each quadrant sits where its name says, not merely with the right amount of ink.
#[test]
fn quadrants_sit_in_their_own_corners() {
    let rects = |glyph: char| block(glyph).expect("a block").0;
    let (upper_left, upper_right) = ((0.0, 0.0, 0.5, 0.5), (0.5, 0.0, 0.5, 0.5));
    let (lower_left, lower_right) = ((0.0, 0.5, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5));
    assert_eq!(rects('\u{2598}'), vec![upper_left]);
    assert_eq!(rects('\u{259d}'), vec![upper_right]);
    assert_eq!(rects('\u{2596}'), vec![lower_left]);
    assert_eq!(rects('\u{2597}'), vec![lower_right]);
    assert_eq!(rects('\u{259a}'), vec![upper_left, lower_right]);
    assert_eq!(rects('\u{259e}'), vec![upper_right, lower_left]);
    assert_eq!(rects('\u{2599}'), vec![upper_left, lower_left, lower_right]);
    assert_eq!(rects('\u{259b}'), vec![upper_left, upper_right, lower_left]);
    assert_eq!(rects('\u{259c}'), vec![upper_left, upper_right, lower_right]);
    assert_eq!(rects('\u{259f}'), vec![upper_right, lower_left, lower_right]);
}
