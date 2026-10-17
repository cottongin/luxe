//! Images — inline captures at each fidelity tier.

use super::*;

/// A PNG whose halves are different colors, so a rasterized cell is checkable.
pub(super) fn split_png(width: u32, height: u32) -> String {
    use base64::Engine as _;
    let mut pixels = image::RgbaImage::new(width, height);
    let midpoint = height / 2;
    for (_, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = if y < midpoint {
            image::Rgba([220, 40, 40, 255])
        } else {
            image::Rgba([40, 60, 220, 255])
        };
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("encode png");
    base64::engine::general_purpose::STANDARD.encode(png.into_inner())
}

/// A store drawing at `tier` against cells of a known size, so the geometry is not the host's.
pub(super) fn pinned_store(tier: luxe_tui::graphics::ImageTier) -> luxe_tui::graphics::ImageStore {
    luxe_tui::graphics::ImageStore::new(
        tier,
        luxe_tui::graphics::CellSize { width: 10, height: 20 },
    )
}

/// An app holding one pasted image, drawn at `tier`.
pub(super) fn app_with_image(tier: luxe_tui::graphics::ImageTier) -> App {
    let mut app = create_app();
    app.images = pinned_store(tier);
    let key = app.attachment_image_key();
    let origin = app.image_origin("pasted image", None);
    assert!(app.intern_image(&key, &split_png(320, 160), origin));
    app.push_convo(ConvoBlock::Image { key, width: 320, height: 160 });
    app
}

pub(super) fn rows_of(buffer: &Buffer) -> Vec<String> {
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol().to_string()).collect())
        .collect()
}

/// With images off — or on a terminal that cannot draw them — the reader still learns that there
/// was one and how big it was. An image nobody can show is a fact about the conversation.
#[test]
fn an_undrawable_image_is_still_named_in_words() {
    let backend = test_backend(80, 24);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Text);

    let buffer = render_app(&mut app, &backend);
    let line = rows_of(&buffer)
        .into_iter()
        .find(|row| row.contains("pasted image"))
        .expect("the image is named");
    assert!(line.contains("320") && line.contains("160"), "with its size: {line:?}");
}

/// An image the store refused — unreadable, or past the size caps — must not render as a blank gap
/// the reader has no way to interpret.
#[test]
fn an_image_that_could_not_be_taken_in_says_so() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    let key = app.attachment_image_key();
    let origin = app.image_origin("pasted image", None);
    assert!(!app.intern_image(&key, "not-base64-at-all!!", origin));
    app.push_convo(ConvoBlock::Image { key, width: 320, height: 160 });

    let buffer = render_app(&mut app, &backend);
    assert!(
        rows_of(&buffer).iter().any(|row| row.contains("image unavailable")),
        "an image that could not be decoded is reported, not silently dropped"
    );
}

/// The blocks tier draws the picture as ordinary colored cells: `▀` with the upper scanline as
/// foreground and the lower as background. Nothing else in the transcript can tell the difference,
/// which is exactly why it works in every terminal.
#[test]
fn the_blocks_tier_draws_the_image_as_colored_cells() {
    let backend = test_backend(80, 24);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Blocks);

    let buffer = render_app(&mut app, &backend);
    let painted: Vec<_> = buffer.content.iter().filter(|cell| cell.symbol() == "▀").collect();
    assert!(painted.len() > 40, "the image occupies real cells: {}", painted.len());
    assert!(
        painted.iter().any(|cell| matches!(cell.fg, ratatui::style::Color::Rgb(r, _, b) if r > b)),
        "the red half of the source is on screen"
    );
    assert!(
        painted.iter().any(|cell| matches!(cell.bg, ratatui::style::Color::Rgb(r, _, b) if b > r)),
        "and so is the blue half"
    );
}

/// The pixel tier writes placeholder cells the terminal resolves into an image: a real symbol and
/// a real foreground color, so ratatui's diff erases them like any other text. What this guards is
/// that no escape sequence is ever smuggled into a cell — cells the diff cannot reason about are
/// how an image gets left behind on screen after it scrolls away.
#[test]
fn the_pixel_tier_writes_only_addressable_cells() {
    let backend = test_backend(80, 24);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Pixels);

    let buffer = render_app(&mut app, &backend);
    let placeholders: Vec<_> =
        buffer.content.iter().filter(|cell| cell.symbol().starts_with('\u{10EEEE}')).collect();
    assert!(placeholders.len() > 40, "the image occupies real cells: {}", placeholders.len());
    assert!(
        placeholders.iter().all(|cell| matches!(cell.fg, ratatui::style::Color::Rgb(..))),
        "every image cell names its image through a foreground color"
    );
    for cell in buffer.content.iter() {
        assert!(!cell.symbol().contains('\u{1b}'), "no escape sequence reaches the cell buffer");
    }
    assert!(
        app.images.drain_control().contains("a=T,U=1"),
        "and the image itself is owed to the stream, not to the buffer"
    );
}

/// A screenshot's picture belongs to the call that produced it, and it is never behind the expand
/// toggle: for `screenshot` the image *is* the result.
#[test]
fn a_tool_cards_image_renders_without_being_expanded() {
    use luxe_core::message::{ContentBlock, Message, Role, ToolOutput};

    let backend = test_backend(100, 40);
    let mut app = create_app();
    app.images = pinned_store(luxe_tui::graphics::ImageTier::Blocks);
    app.update(Msg::Engine(luxe_core::Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![],
    }));
    app.update(Msg::Engine(luxe_core::Event::ToolActivityUpdated(Box::new(
        luxe_core::ToolActivity::running("session-a", "call-1", "screenshot", "mode=url", 1),
    ))));
    app.update(Msg::Engine(luxe_core::Event::MessageCommitted {
        session_id: "session-a".into(),
        entry_id: luxe_core::session::EntryId("entry-1".into()),
        message: Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "call-1".into(),
                content: vec![
                    ToolOutput::Text("Captured url (320x160)".into()),
                    ToolOutput::Image {
                        data: split_png(320, 160),
                        mime: "image/png".into(),
                        name: Some(".luxe/screenshots/shot-1.png".into()),
                    },
                ],
                is_error: false,
            }],
        },
    }));

    assert!(!app.tool_output_expanded, "the card is collapsed");
    let buffer = render_app(&mut app, &backend);
    let painted = buffer.content.iter().filter(|cell| cell.symbol() == "▀").count();
    assert!(painted > 40, "the screenshot is on screen anyway: {painted} cells");
}
