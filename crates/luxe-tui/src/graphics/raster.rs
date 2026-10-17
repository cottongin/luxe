//! Raster — fitting an image to cells, probing its size, and scaling it to PNG.

use super::{kitty, CellSize, Fit};

/// Fits `(width, height)` pixels into a cell rectangle no larger than `max_cols`×`max_rows`.
///
/// Kept free of the store so the arithmetic — which is where an image gets stretched, squashed, or
/// silently given a zero dimension — can be tested on its own.
pub(super) fn fit_within(
    (width, height): (u32, u32),
    max_cols: u16,
    max_rows: u16,
    cell: CellSize,
) -> Fit {
    let max_cols = max_cols.clamp(1, kitty::MAX_CELLS);
    let max_rows = max_rows.clamp(1, kitty::MAX_CELLS);
    if width == 0 || height == 0 {
        return Fit { cols: 1, rows: 1 };
    }

    let aspect = f64::from(height) / f64::from(width);
    let cell_ratio = f64::from(cell.width) / f64::from(cell.height);

    let mut cols = max_cols;
    let mut rows = ((f64::from(cols) * aspect * cell_ratio).round() as u16).max(1);
    if rows > max_rows {
        rows = max_rows;
        cols = ((f64::from(rows) / (aspect * cell_ratio)).round() as u16).clamp(1, max_cols);
    }
    Fit { cols, rows }
}

/// Scales `image` to exactly the pixels `fit` occupies and encodes it as PNG.
///
/// Scaled here rather than left to the terminal because the terminal's own fit is a GPU filter over
/// whatever it was sent, and a screenshot full of text downscaled that way is mush. Fast
/// compression: the bytes are on their way to a local terminal, not to disk.
pub(super) fn scale_to_png(
    image: &image::DynamicImage,
    fit: Fit,
    cell: CellSize,
) -> Option<Vec<u8>> {
    let target_width = u32::from(fit.cols) * u32::from(cell.width);
    let target_height = u32::from(fit.rows) * u32::from(cell.height);
    if target_width == 0 || target_height == 0 {
        return None;
    }
    let scaled = image.resize(target_width, target_height, image::imageops::FilterType::Lanczos3);

    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    let rgba = scaled.to_rgba8();
    image::ImageEncoder::write_image(
        encoder,
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(png)
}

/// The image's dimensions, from its header alone.
pub(super) fn probe_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}
