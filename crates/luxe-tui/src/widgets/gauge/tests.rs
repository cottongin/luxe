use super::*;
use ratatui::style::Color;

/// Stand-ins for the theme's derived ramps: the value's, brightest first, and the rail's.
fn fill() -> Vec<Style> {
    [Color::Rgb(220, 230, 255), Color::Rgb(150, 170, 220), Color::Rgb(90, 100, 140)]
        .into_iter()
        .map(|color| Style::default().fg(color))
        .collect()
}

fn rail() -> Vec<Style> {
    [Color::Rgb(69, 71, 90), Color::Rgb(45, 46, 60), Color::Rgb(32, 33, 42)]
        .into_iter()
        .map(|color| Style::default().fg(color))
        .collect()
}

fn bar(percent: f32, cells: usize) -> Vec<Span<'static>> {
    fill_gauge(percent, cells, &fill(), &rail())
}

fn rendered(percent: f32, cells: usize) -> String {
    bar(percent, cells).into_iter().map(|span| span.content.into_owned()).collect()
}

/// The bar's style per cell, spans flattened back out.
fn shades(percent: f32, cells: usize) -> Vec<Style> {
    bar(percent, cells)
        .iter()
        .flat_map(|span| std::iter::repeat_n(span.style, span.content.chars().count()))
        .collect()
}

#[test]
fn the_bar_is_always_exactly_as_wide_as_it_was_asked_to_be() {
    for percent in [0.0, 0.4, 12.5, 42.0, 50.0, 99.9, 100.0, 250.0, -10.0] {
        assert_eq!(rendered(percent, 10).chars().count(), 10, "at {percent}%");
    }
}

#[test]
fn fill_tracks_the_value() {
    assert_eq!(rendered(0.0, 10), "▁▁▁▁▁▁▁▁▁▁");
    assert_eq!(rendered(50.0, 10), "█████▁▁▁▁▁");
    assert_eq!(rendered(100.0, 10), "██████████");
}

/// Sub-cell resolution is the point: eight steps per cell, so a value between cells renders as a
/// partly-filled cell rather than rounding to one side.
#[test]
fn a_partial_cell_fills_vertically() {
    // Ten cells, eight steps each: eighty steps, so each 1.25% moves the bar.
    assert_eq!(rendered(45.0, 10), "████▄▁▁▁▁▁", "four full, one half");
    assert_eq!(rendered(47.5, 10), "████▆▁▁▁▁▁", "one three-quarters");
    // The braille rail this replaced had four steps, so 42% and 45% drew the same bar. Eight
    // steps tell them apart.
    assert_eq!(rendered(42.0, 10), "████▂▁▁▁▁▁", "42% is not 45%");
    assert_eq!(rendered(1.0, 10), "▁▁▁▁▁▁▁▁▁▁", "a trace is a lit eighth, told by color");
    assert_ne!(bar(1.0, 10)[0].style, rail()[0], "and it belongs to the value");
}

/// The bar brightens toward the frontier, which is where the eye should land: that is the value.
#[test]
fn the_value_brightens_toward_its_leading_edge() {
    let shades = shades(80.0, 10);
    let fill = fill();

    assert_eq!(shades[7], fill[0], "the frontier wears the brightest shade");
    assert_eq!(shades[0], fill[fill.len() - 1], "and the start the dimmest");
    assert!(
        shades[..8].windows(2).all(|pair| pair[0] != pair[1] || fill.len() < 8),
        "with a gradient in between: {shades:?}"
    );
}

/// And the remainder fades away from it, so the bar has no hard end.
#[test]
fn the_rail_fades_away_from_the_frontier() {
    let shades = shades(20.0, 10);
    let rail = rail();

    assert_eq!(shades[2], rail[0], "the cell after the value is the rail at full");
    assert_eq!(shades[9], rail[rail.len() - 1], "the far end is all but the band");
    // A full bar has no rail to fade, and must not reach for one.
    assert!(bar(100.0, 10).iter().all(|span| !rail.contains(&span.style)));
}

/// The partial cell is spent, so it carries the value's color, not the rail's.
#[test]
fn the_partial_cell_belongs_to_the_value() {
    let shades = shades(45.0, 10);
    assert!(fill().contains(&shades[4]), "the boundary cell is spent, not rail");
    assert!(rail().contains(&shades[5]), "and the cell past it is rail");
}

#[test]
fn a_bar_with_no_room_renders_nothing() {
    assert!(fill_gauge(50.0, 0, &fill(), &rail()).is_empty());
    assert!(fill_gauge(50.0, 10, &[], &rail()).is_empty());
    assert!(fill_gauge(50.0, 10, &fill(), &[]).is_empty());
}

/// Every unit is one cell wide, whatever the value — the meter's width is a layout promise.
#[test]
fn every_glyph_is_a_lower_block() {
    for percent in (0..=100).step_by(3) {
        for glyph in rendered(percent as f32, 10).chars() {
            assert!(RAMP.contains(&glyph), "{glyph:?} at {percent}%");
        }
    }
}
