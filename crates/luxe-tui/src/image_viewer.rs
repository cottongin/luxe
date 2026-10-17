//! image_viewer — one image, as large as the terminal will draw it.
//!
//! The transcript caps an image at a fraction of the viewport so a conversation stays a
//! conversation, which makes every inline image a thumbnail. This is the other half of that trade:
//! double-click one (or press `Alt+I`) and it fills the window at whatever resolution the terminal
//! can manage, `Esc` puts it back.
//!
//! Rendered as a full-area overlay rather than a view mode, because it is a look rather than a
//! place: nothing about the session changes while it is open, and closing it returns to exactly the
//! transcript that was underneath.

use crate::glyphs::Icon;
use crate::graphics::{ImageKey, ImageStore};
use crate::theme::styles::ThemeStyles;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

/// Draws `key` over the whole of `area`.
///
/// Painted row by row through the same store the transcript uses, so an image already transmitted
/// for the transcript is re-fitted rather than re-sent, and the viewer inherits every bound the
/// store enforces.
pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    key: &ImageKey,
    images: &mut ImageStore,
    styles: &ThemeStyles,
) {
    frame.render_widget(Clear, area);
    let label = images.origin(key).map_or("image", |origin| origin.label.as_str()).to_string();
    let frame_block = crate::widgets::titled_panel(Icon::Image.glyph(), &label, styles);
    let inner = frame_block.inner(area);
    frame.render_widget(frame_block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let (stage, footer) = crate::widgets::split_footer(inner);

    let drawn = images
        .tier()
        .draws_pixels()
        .then(|| images.fit(key, stage.width, stage.height))
        .flatten()
        .filter(|fit| stage.height >= fit.rows);
    if let Some(fit) = drawn {
        // Centered: an image letterboxed to one corner of an otherwise empty window looks like a
        // layout bug rather than a picture.
        let left = stage.x + (stage.width.saturating_sub(fit.cols)) / 2;
        let top = stage.y + (stage.height.saturating_sub(fit.rows)) / 2;
        let canvas = styles.background.bg.unwrap_or(ratatui::style::Color::Reset);
        for row in 0..fit.rows {
            images.paint_row(frame.buffer_mut(), (left, top + row), key, fit, row, canvas);
        }
    }

    if drawn.is_none() {
        // The one case worth explaining in place: the viewer is open and empty, which without a
        // word reads as a failure rather than as a terminal that cannot draw pictures
        let message = "Images unavailable in this terminal · /images";
        let top = stage.y + stage.height / 2;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message, styles.muted)))
                .alignment(ratatui::layout::Alignment::Center),
            Rect { y: top, height: 1.min(stage.height), ..stage },
        );
    }
    crate::widgets::render_footer(frame, footer, "esc back", Some(&about(key, images)), styles);
}

/// What the footer says about the image: how big it really is, and where it came from.
fn about(key: &ImageKey, images: &ImageStore) -> String {
    let size = images
        .source_size(key)
        .map(|(width, height)| format!("{width}\u{d7}{height}"))
        .unwrap_or_default();
    let path = images
        .origin(key)
        .and_then(|origin| origin.path.as_deref())
        .map(luxe_core::paths::home_relative)
        .unwrap_or_default();
    [size, path].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{CellSize, ImageOrigin, ImageTier};
    use ratatui::{backend::TestBackend, Terminal};

    fn styles() -> ThemeStyles {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("dark").expect("the dark theme exists");
        ThemeStyles::from_resolved(&resolved, "dark")
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([30, 90, 200, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode");
        out.into_inner()
    }

    fn seeded(tier: ImageTier) -> (ImageStore, ImageKey) {
        let mut store = ImageStore::new(tier, CellSize { width: 10, height: 20 });
        let key = ImageKey::new("viewed");
        store.insert(
            key.clone(),
            png(640, 320),
            ImageOrigin { label: "screenshot".into(), path: None },
        );
        (store, key)
    }

    fn draw(tier: ImageTier) -> ratatui::buffer::Buffer {
        let (mut store, key) = seeded(tier);
        let styles = styles();
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &key, &mut store, &styles);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content.iter().map(ratatui::buffer::Cell::symbol).collect()
    }

    /// The viewer's whole reason to exist: the image, bigger than the transcript would allow, with
    /// a caption naming it and the way out.
    #[test]
    fn the_viewer_fills_the_window_and_says_how_to_leave() {
        let buffer = draw(ImageTier::Pixels);
        let painted =
            buffer.content.iter().filter(|cell| cell.symbol().starts_with('\u{10EEEE}')).count();
        assert!(painted > 200, "the image takes most of the window: {painted} cells");

        let rendered = text(&buffer);
        assert!(rendered.contains("screenshot"), "the caption names it");
        assert!(rendered.contains("640") && rendered.contains("320"), "and its real size");
        assert!(rendered.contains("esc back"));
    }

    /// Opened where nothing can be drawn, an empty box reads as a bug. It has to say what happened.
    #[test]
    fn a_viewer_that_cannot_draw_explains_itself() {
        let rendered = text(&draw(ImageTier::Text));
        assert!(rendered.contains("Images unavailable"), "{rendered}");
        assert!(rendered.contains("/images"), "and where the reason is: {rendered}");
        assert!(rendered.contains("screenshot"), "the caption is still there");
    }
}
