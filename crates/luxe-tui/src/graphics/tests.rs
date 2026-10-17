use super::*;

/// A terminal that reports zero pixels gets the assumption rather than a division by zero.
#[test]
fn cell_geometry_survives_a_terminal_that_will_not_say() {
    assert_eq!(CellSize::from_window(1220, 1344, 122, 64), CellSize { width: 10, height: 21 });
    assert_eq!(CellSize::from_window(0, 0, 122, 64), CellSize::ASSUMED);
    assert_eq!(CellSize::from_window(1220, 1344, 0, 0), CellSize::ASSUMED);
    // Sub-pixel cells round to at least one, so nothing downstream divides by zero.
    assert_eq!(CellSize::from_window(10, 10, 400, 400), CellSize { width: 1, height: 1 });
}

/// Width is the given and height follows from the aspect ratio — corrected for cells being
/// about twice as tall as they are wide, which is the step that, left out, squashes every
/// image by half.
#[test]
fn a_fit_preserves_the_aspect_ratio_through_the_cell_shape() {
    let cell = CellSize { width: 10, height: 20 };
    // A 2:1 image across 40 columns is 400x200 px, which is 10 rows of 20px cells.
    assert_eq!(fit_within((800, 400), 40, 100, cell), Fit { cols: 40, rows: 10 });
    // A square image across 40 columns is 400px tall: 20 rows.
    assert_eq!(fit_within((500, 500), 40, 100, cell), Fit { cols: 40, rows: 20 });
}

/// Too tall to fit means smaller, never cropped: the height cap pulls the width down with it
/// so the whole picture is still on screen.
#[test]
fn a_row_cap_shrinks_the_image_rather_than_cutting_it() {
    let cell = CellSize { width: 10, height: 20 };
    let fit = fit_within((500, 1000), 40, 12, cell);
    assert_eq!(fit.rows, 12, "capped");
    assert!(fit.cols < 40, "and narrowed to match: {fit:?}");
    // Same aspect ratio as the source, within a cell of rounding.
    let drawn = (f64::from(fit.cols) * 10.0, f64::from(fit.rows) * 20.0);
    assert!((drawn.1 / drawn.0 - 2.0).abs() < 0.25, "aspect preserved: {drawn:?}");
}

/// Degenerate inputs still produce something drawable, and nothing exceeds the protocol's own
/// coordinate limit.
#[test]
fn a_fit_is_always_drawable() {
    let cell = CellSize::ASSUMED;
    assert_eq!(fit_within((0, 0), 40, 10, cell), Fit { cols: 1, rows: 1 });
    assert_eq!(fit_within((100, 100), 0, 0, cell), Fit { cols: 1, rows: 1 });
    let huge = fit_within((10_000, 10), 5000, 5000, cell);
    assert!(huge.cols <= kitty::MAX_CELLS && huge.rows >= 1, "{huge:?}");
}
