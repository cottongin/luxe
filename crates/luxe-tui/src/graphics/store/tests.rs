use super::*;
use crate::graphics::{kitty, CellSize, Fit, ImageKey, ImageOrigin};
use image::{Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([10, 120, 240, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

fn origin(label: &str) -> ImageOrigin {
    ImageOrigin { label: label.into(), path: None }
}

fn store(tier: ImageTier) -> ImageStore {
    ImageStore::new(tier, CellSize { width: 10, height: 20 })
}

fn seeded(tier: ImageTier, width: u32, height: u32) -> (ImageStore, ImageKey) {
    let mut store = store(tier);
    let key = ImageKey::new("k");
    assert!(store.insert(key.clone(), png(width, height), origin("shot.png")));
    (store, key)
}

/// The header is the gate: an oversized or unreadable image is refused before it is decoded,
/// so a decompression bomb costs a parse instead of a gigabyte.
#[test]
fn an_image_is_judged_on_its_header() {
    let mut store = store(ImageTier::Blocks);
    assert!(!store.insert(ImageKey::new("empty"), Vec::new(), origin("x")));
    assert!(!store.insert(ImageKey::new("garbage"), vec![1, 2, 3, 4, 5, 6, 7, 8], origin("x")));
    assert!(store.insert(ImageKey::new("real"), png(32, 16), origin("x")));
    assert_eq!(store.source_size(&ImageKey::new("real")), Some((32, 16)));
    assert_eq!(store.origin(&ImageKey::new("real")).map(|o| o.label.as_str()), Some("x"));
    assert!(!store.contains(&ImageKey::new("garbage")));
}

/// Re-inserting a known key is free, which is what makes replaying a session's transcript on
/// resume cost nothing.
#[test]
fn re_inserting_a_known_image_is_a_no_op() {
    let (mut store, key) = seeded(ImageTier::Blocks, 40, 20);
    let before = store.total_source_bytes;
    assert!(store.insert(key, png(40, 20), origin("again")));
    assert_eq!(store.total_source_bytes, before, "the second copy is not held");
}

/// The pixel tier's contract with the buffer: real symbols and a real foreground color, so
/// ratatui's diff can erase them like any other text. No escape sequence goes into a cell.
#[test]
fn the_pixel_tier_paints_addressable_cells_and_nothing_else() {
    let (mut store, key) = seeded(ImageTier::Pixels, 80, 40);
    let fit = store.fit(&key, 8, 24).expect("a known image fits");
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));

    assert!(store.paint_row(&mut buf, (2, 1), &key, fit, 0, Color::Reset));
    let painted = &buf[(2, 1)];
    assert_eq!(painted.symbol(), kitty::placeholder(0, 0));
    assert_eq!(
        painted.fg,
        kitty::id_color(match store.live[&key].kind {
            LiveKind::Pixels(id) => id,
            LiveKind::Blocks(_) => panic!("pixel tier"),
        })
    );
    assert_eq!(buf[(3, 1)].symbol(), kitty::placeholder(0, 1), "columns are addressed in turn");
    for cell in buf.content() {
        assert!(!cell.symbol().contains('\u{1b}'), "no escape sequence reaches the buffer");
    }
}

/// Rows are painted independently, which is what makes a half-scrolled image correct: the
/// caller simply paints fewer of them and the terminal crops to what it was given.
#[test]
fn rows_are_independent_so_a_clipped_image_needs_no_special_case() {
    let (mut store, key) = seeded(ImageTier::Pixels, 80, 160);
    let fit = store.fit(&key, 8, 24).expect("fits");
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));

    assert!(store.paint_row(&mut buf, (0, 0), &key, fit, 3, Color::Reset));
    assert!(store.paint_row(&mut buf, (0, 1), &key, fit, 4, Color::Reset));
    assert_eq!(buf[(0, 0)].symbol(), kitty::placeholder(3, 0));
    assert_eq!(buf[(0, 1)].symbol(), kitty::placeholder(4, 0));
    // A row the image does not have is refused rather than drawn wrong.
    assert!(!store.paint_row(&mut buf, (0, 2), &key, fit, fit.rows, Color::Reset));
}

/// The transmission is owed to the stream, not to the buffer — once per image, and again only
/// when the size it was prepared for changes.
#[test]
fn an_image_transmits_once_per_size() {
    let (mut store, key) = seeded(ImageTier::Pixels, 80, 40);
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));

    let fit = store.fit(&key, 8, 24).expect("fits");
    store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset);
    let first = store.drain_control();
    assert!(first.starts_with("\x1b_Gq=2,i="), "the image is transmitted: {:?}", &first[..24]);
    assert!(first.contains(&format!("c={},r={}", fit.cols, fit.rows)));

    store.paint_row(&mut buf, (0, 1), &key, fit, 1, Color::Reset);
    assert!(store.drain_control().is_empty(), "painting more rows costs nothing");

    let narrower = store.fit(&key, 4, 24).expect("fits");
    assert_ne!(narrower, fit);
    store.paint_row(&mut buf, (0, 0), &key, narrower, 0, Color::Reset);
    let resized = store.drain_control();
    assert!(resized.contains("a=d,d=I"), "the old size is released: {resized:?}");
    assert!(resized.contains("a=T,U=1"), "and the new one transmitted");
}

/// A frame that draws the same thing must cost the terminal nothing.
///
/// This is the invariant that keeps a still screen still. Break it and the store releases and
/// re-transmits on every repaint — a few hundred kilobytes and a fresh image id per frame — and
/// the picture flickers as the terminal's graphics store churns underneath it. The way to break
/// it is to ask for the *same* image at two sizes in one frame, which is exactly what a
/// full-window viewer drawn over a transcript that also draws that image does.
#[test]
fn a_steady_frame_owes_the_terminal_nothing() {
    let (mut store, key) = seeded(ImageTier::Pixels, 800, 400);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 8));
    let fit = store.fit(&key, 30, 6).expect("fits");

    let frame = |store: &mut ImageStore, buf: &mut Buffer| {
        for row in 0..fit.rows {
            store.paint_row(buf, (0, row), &key, fit, row, Color::Reset);
        }
        store.sweep();
        store.drain_control()
    };

    assert!(!frame(&mut store, &mut buf).is_empty(), "the first frame transmits");
    for index in 0..5 {
        assert!(frame(&mut store, &mut buf).is_empty(), "frame {index} re-transmitted");
    }

    // And it writes byte-identical cells, so ratatui's own diff emits nothing either.
    let settled = buf.clone();
    frame(&mut store, &mut buf);
    assert_eq!(buf, settled, "a steady frame does not even change the cells");
}

/// Every image put into the terminal is taken back out, or a long session leaves the terminal
/// holding every screenshot it ever showed.
#[test]
fn eviction_and_shutdown_both_release_the_terminals_copy() {
    let mut store = store(ImageTier::Pixels);
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
    let keys: Vec<ImageKey> = (0..LIVE_CAPACITY + 3)
        .map(|index| {
            let key = ImageKey::new(format!("k{index}"));
            store.insert(key.clone(), png(20, 10), origin("x"));
            key
        })
        .collect();
    for key in &keys {
        let fit = store.fit(key, 4, 4).expect("fits");
        store.paint_row(&mut buf, (0, 0), key, fit, 0, Color::Reset);
    }
    store.drain_control();

    store.sweep();
    assert_eq!(store.live.len(), LIVE_CAPACITY, "the store is bounded");
    assert_eq!(store.drain_control().matches("a=d,d=I").count(), 3, "and says so");
    // The most recently painted images are the ones kept.
    assert!(store.live.contains_key(keys.last().expect("keys")));

    store.release_all();
    assert_eq!(store.drain_control().matches("a=d,d=I").count(), LIVE_CAPACITY);
    assert!(store.live.is_empty());
}

/// Switching fidelity or resizing the terminal invalidates what was prepared for the old one,
/// and hands back what the terminal was holding.
#[test]
fn changing_tier_or_cell_size_releases_what_was_prepared() {
    let (mut store, key) = seeded(ImageTier::Pixels, 80, 40);
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
    let fit = store.fit(&key, 8, 24).expect("fits");
    store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset);
    store.drain_control();

    store.set_cell_size(CellSize { width: 8, height: 16 });
    assert!(store.drain_control().contains("a=d,d=I"), "the terminal's copy is handed back");
    assert!(store.live.is_empty());
    assert!(store.contains(&key), "the source survives so it can be drawn again");

    store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset);
    store.drain_control();
    store.set_tier(ImageTier::Blocks);
    assert!(store.drain_control().contains("a=d,d=I"));
    assert_eq!(store.tier(), ImageTier::Blocks);
}

/// The blocks tier is ordinary colored text: two scanlines a cell, no protocol, nothing owed
/// to the terminal afterwards.
#[test]
fn the_blocks_tier_paints_colors_and_owes_the_terminal_nothing() {
    let (mut store, key) = seeded(ImageTier::Blocks, 40, 20);
    let fit = store.fit(&key, 6, 24).expect("fits");
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));

    assert!(store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset));
    assert_eq!(buf[(0, 0)].symbol(), "▀");
    assert_eq!(buf[(0, 0)].fg, Color::Rgb(10, 120, 240));
    assert_eq!(buf[(0, 0)].bg, Color::Rgb(10, 120, 240));
    assert!(store.drain_control().is_empty(), "no protocol, nothing owed");
}

/// Turning images off is a real answer, and the caller has to be told so it can say something
/// instead of drawing nothing.
#[test]
fn the_text_tier_paints_nothing_and_admits_it() {
    let (mut store, key) = seeded(ImageTier::Text, 40, 20);
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
    let fit = Fit { cols: 4, rows: 1 };
    assert!(!store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset));
    assert!(!store.paint_row(&mut buf, (0, 0), &ImageKey::new("missing"), fit, 0, Color::Reset));
}

/// Ids are global to the terminal, so two luxe windows must not hand out the same one from the
/// first frame and quietly replace each other's images.
#[test]
fn image_ids_stay_inside_the_protocols_range() {
    for _ in 0..8 {
        let seed = seed_id();
        assert!((1..=kitty::MAX_ID).contains(&seed), "{seed}");
    }
    let mut store = store(ImageTier::Pixels);
    store.next_id = kitty::MAX_ID;
    assert_eq!(store.allocate_id(), kitty::MAX_ID);
    assert_eq!(store.allocate_id(), 1, "and wraps rather than overflowing");
}

/// The break this catches: an image arriving without moving the layout revision, so a card laid
/// out before it landed keeps its old rows.
#[test]
fn a_new_image_moves_the_layout_revision_and_a_known_one_does_not() {
    let mut store = store(ImageTier::Blocks);
    let empty = store.layout_revision();
    let key = ImageKey::new("k");
    assert!(store.insert(key.clone(), png(32, 16), origin("shot.png")));
    let landed = store.layout_revision();
    assert_ne!(landed, empty, "a new source moves it");

    assert!(store.insert(key, png(32, 16), origin("shot.png")));
    assert_eq!(store.layout_revision(), landed, "a known one changes nothing a layout reads");
}

/// The break this catches: a new cell size or tier leaving image rows sized for the old one, or
/// a repeated report of the same geometry throwing every image card's layout away.
#[test]
fn only_a_real_change_of_geometry_or_tier_moves_the_layout_revision() {
    let mut store = store(ImageTier::Blocks);
    let start = store.layout_revision();
    store.set_cell_size(CellSize { width: 10, height: 20 });
    store.set_tier(ImageTier::Blocks);
    assert_eq!(store.layout_revision(), start, "the same geometry and tier change nothing");

    store.set_cell_size(CellSize { width: 8, height: 16 });
    let resized = store.layout_revision();
    assert_ne!(resized, start, "a new cell size moves it");
    store.set_tier(ImageTier::Text);
    assert_ne!(store.layout_revision(), resized, "a new tier moves it");
}

/// The break this catches: a source that is gone (cleared for another session, or dropped for not
/// decoding) leaving its card laid out as though it could still be drawn.
#[test]
fn losing_a_source_moves_the_layout_revision() {
    let (mut store, _) = seeded(ImageTier::Blocks, 32, 16);
    let seeded_at = store.layout_revision();
    store.clear();
    assert_ne!(store.layout_revision(), seeded_at, "clearing moves it");

    // A header that parses ahead of pixels that don't: the source is dropped on first paint
    let mut broken = png(32, 16);
    let pixels = broken.len() - 20;
    broken[pixels] ^= 0xff;
    let key = ImageKey::new("broken");
    assert!(store.insert(key.clone(), broken, origin("broken.png")));
    let inserted = store.layout_revision();
    let fit = store.fit(&key, 6, 24).expect("the header gives a size");
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
    assert!(!store.paint_row(&mut buf, (0, 0), &key, fit, 0, Color::Reset));
    assert!(!store.contains(&key), "the source is gone");
    assert_ne!(store.layout_revision(), inserted, "and the layout revision says so");
}
