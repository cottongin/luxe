use super::*;
use crate::app::{App, ConversationItem, ConvoBlock, ToolItem};
use crate::graphics::{CellSize, ImageKey, ImageOrigin};

fn signature(first: usize, revision: BlockRevision) -> UnitSignature {
    UnitSignature {
        first,
        len: 1,
        revision,
        leading_gap: first > 0,
        folded: false,
        arriving: false,
        selected: false,
        never_sent: false,
        expanded: false,
        images: None,
    }
}

fn row(text: &str) -> RenderedConversationLine {
    RenderedConversationLine {
        item_index: 0,
        item_id: Rc::new(ConversationItemId::Provisional {
            session_id: String::new(),
            turn_id: 0,
            block_index: 0,
        }),
        visual_line_within_item: 0,
        line: Rc::new(Line::from(text.to_string())),
        code_copy: None,
        image: None,
        opens_image: None,
        expands: false,
    }
}

fn context() -> LayoutContext {
    LayoutContext {
        canvas: super::super::conversation_canvas_widths(120),
        card_width: 100,
        image_bounds: (100, 20),
        diff_layout: luxe_core::config::DiffLayout::Unified,
        tool_output_expanded: false,
        expand_chord: None,
        styles: crate::theme::styles::ThemeStyles::from_resolved(
            &luxe_core::theme::ThemeRegistry::new().resolve("dark").expect("the dark theme"),
            "dark",
        ),
        glyphs: crate::glyphs::GlyphTier::Ascii,
    }
}

/// Editing a block has to be visible to the cache, and the only way to edit one bumps it.
#[test]
fn an_edit_is_what_changes_a_revision() {
    let id = ConversationItemId::Provisional { session_id: "s".into(), turn_id: 0, block_index: 0 };
    let mut item = ConversationItem::new(id, ConvoBlock::Assistant("one".into()));
    let before = item.revision();
    assert_eq!(item.revision(), before, "reading does not count as a change");
    assert!(matches!(item.block(), ConvoBlock::Assistant(_)));
    if let ConvoBlock::Assistant(text) = item.block_mut() {
        text.push_str(" two");
    }
    assert!(item.revision() > before, "editing does");

    let other = ConversationItem::new(
        ConversationItemId::Provisional { session_id: "s".into(), turn_id: 1, block_index: 0 },
        ConvoBlock::Assistant("one two".into()),
    );
    assert_ne!(
        other.revision(),
        item.revision(),
        "identical content in a fresh item is still a different revision"
    );
}

/// The point of the whole module: a frame that changes nothing reuses everything.
#[test]
fn an_unchanged_frame_relays_out_nothing() {
    let mut layout = TranscriptLayout::default();
    let context = context();
    let first = signature(0, BlockRevision::for_test(1));
    let second = signature(1, BlockRevision::for_test(2));

    layout.begin(&context);
    layout.replace(first, vec![row("a"), row("b")], 2, true);
    layout.replace(second, vec![row("c")], 1, true);
    layout.finish();
    assert_eq!(layout.rows().len(), 3);

    layout.begin(&context);
    assert_eq!(layout.reusable(&first), Some(2));
    layout.keep();
    assert_eq!(layout.reusable(&second), Some(1));
    layout.keep();
    layout.finish();
    assert_eq!(layout.rows().len(), 3, "and the row list is the same one");
}

/// A changed item invalidates itself and nothing else — the tail of a stream must not cost the
/// history above it.
#[test]
fn a_changed_item_invalidates_only_itself() {
    let mut layout = TranscriptLayout::default();
    let context = context();
    let first = signature(0, BlockRevision::for_test(1));
    let second = signature(1, BlockRevision::for_test(2));

    layout.begin(&context);
    layout.replace(first, vec![row("a")], 1, true);
    layout.replace(second, vec![row("b")], 1, true);
    layout.finish();

    let grown = signature(1, BlockRevision::for_test(9));
    layout.begin(&context);
    assert_eq!(layout.reusable(&first), Some(1), "the history is untouched");
    layout.keep();
    assert_eq!(layout.reusable(&grown), None, "the tail is not");
    layout.replace(grown, vec![row("b"), row("b2")], 2, true);
    layout.finish();

    let text = layout.rows().iter().map(|r| r.line.to_string()).collect::<Vec<_>>();
    assert_eq!(text, ["a", "b", "b2"]);
}

/// A unit that redraws by itself — a spinner, a row the reveal is still typing —
/// must never be served from the cache, however unchanged its signature looks.
#[test]
fn a_self_animating_unit_is_never_reused() {
    let mut layout = TranscriptLayout::default();
    let context = context();
    let running = signature(0, BlockRevision::for_test(1));

    layout.begin(&context);
    layout.replace(running, vec![row("spinner")], 1, false);
    layout.finish();

    layout.begin(&context);
    assert_eq!(layout.reusable(&running), None);
}

/// Any frame-wide change — a resize, a theme, a different diff layout — drops everything,
/// because every row was measured against it.
#[test]
fn a_frame_wide_change_drops_the_whole_cache() {
    let mut layout = TranscriptLayout::default();
    let context = context();
    let only = signature(0, BlockRevision::for_test(1));

    layout.begin(&context);
    layout.replace(only, vec![row("a")], 1, true);
    layout.finish();

    let narrower =
        LayoutContext { canvas: super::super::conversation_canvas_widths(80), ..context };
    layout.begin(&narrower);
    assert_eq!(layout.reusable(&only), None, "a resize re-wraps every row");
}

/// Items removed from the transcript take their rows with them.
#[test]
fn a_shortened_transcript_drops_the_rows_it_lost() {
    let mut layout = TranscriptLayout::default();
    let context = context();
    let first = signature(0, BlockRevision::for_test(1));
    let second = signature(1, BlockRevision::for_test(2));

    layout.begin(&context);
    layout.replace(first, vec![row("a")], 1, true);
    layout.replace(second, vec![row("b")], 1, true);
    layout.finish();
    assert_eq!(layout.rows().len(), 2);

    layout.begin(&context);
    layout.keep();
    layout.finish();
    assert_eq!(layout.rows().len(), 1, "the popped item's row is gone");
}

/// An app with nothing in its transcript, and `key` in its image store.
fn app_holding(key: &ImageKey) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(32, 16).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let origin = ImageOrigin { label: "screenshot".into(), path: None };
    assert!(app.images.insert(key.clone(), png.into_inner(), origin));
    app
}

/// An app whose transcript is one finished screenshot call, its image in the store.
fn app_with_screenshot() -> App {
    let key = ImageKey::tool_result("s", "call-shot", 1);
    let mut app = app_holding(&key);
    let mut shot = luxe_core::ToolActivity::running("s", "call-shot", "screenshot", "url", 1);
    shot.state = luxe_core::ToolActivityState::Succeeded;
    let card =
        ToolItem { activity: shot, expanded: false, details_toggled: false, waited_on_user_ms: 0 };
    app.push_convo(ConvoBlock::Tool(Box::new(card)));
    app.tool_images.insert(("s".into(), "call-shot".into()), vec![key]);
    app
}

/// An app whose transcript is one attached image.
fn app_with_attached_image() -> App {
    let key = ImageKey::new("attached");
    let mut app = app_holding(&key);
    app.push_convo(ConvoBlock::Image { key, width: 32, height: 16 });
    app
}

/// One frame's layout of `app`'s transcript, as the rows' shared lines.
fn lay_out_frame(layout: &mut TranscriptLayout, app: &mut App) -> Vec<Rc<Line<'static>>> {
    super::layout::lay_out_transcript(
        layout,
        app,
        super::super::conversation_canvas_widths(120),
        40,
    );
    layout.rows().iter().map(|row| Rc::clone(&row.line)).collect()
}

/// The break this catches: a card carrying an image rebuilt every frame, which in a session with a
/// hundred screenshots lays out a hundred cards a frame wherever they sit; or kept after the store
/// changes under it, drawing rows sized for the old geometry.
#[test]
fn an_image_card_is_reused_until_the_image_store_changes() {
    assert_reused_until_the_store_changes(app_with_screenshot());
}

/// The break this catches: an attached image left out of the store's revision, so its rows
/// outlive a change of cell size or a cleared store.
#[test]
fn an_attached_image_is_reused_until_the_image_store_changes() {
    assert_reused_until_the_store_changes(app_with_attached_image());
}

/// `app`'s only item is laid out once across unchanged frames, and again once the store moves.
fn assert_reused_until_the_store_changes(mut app: App) {
    let mut layout = TranscriptLayout::default();
    let first = lay_out_frame(&mut layout, &mut app);
    let second = lay_out_frame(&mut layout, &mut app);
    assert!(!first.is_empty(), "the card draws");
    let reused = first.iter().zip(&second).all(|(a, b)| Rc::ptr_eq(a, b));
    assert!(reused && first.len() == second.len(), "an unchanged frame reuses the card");

    app.images.set_cell_size(CellSize { width: 7, height: 15 });
    let third = lay_out_frame(&mut layout, &mut app);
    assert!(!Rc::ptr_eq(&second[0], &third[0]), "a store change lays it out again");
}

/// Every field of the signature has to be part of the comparison, or the cache serves a stale
/// row the moment that one thing changes on its own. Flipping each in turn is the cheap way to
/// keep a future field from being added to the struct and forgotten in the equality.
#[test]
fn every_field_of_the_signature_is_part_of_the_comparison() {
    let base = signature(3, BlockRevision::for_test(1));
    type Mutation = (&'static str, fn(&mut UnitSignature));
    let mutations: [Mutation; 10] = [
        ("first", |s| s.first += 1),
        ("len", |s| s.len += 1),
        ("revision", |s| s.revision = BlockRevision::for_test(2)),
        ("leading_gap", |s| s.leading_gap = !s.leading_gap),
        ("folded", |s| s.folded = !s.folded),
        ("arriving", |s| s.arriving = !s.arriving),
        ("selected", |s| s.selected = !s.selected),
        ("never_sent", |s| s.never_sent = !s.never_sent),
        ("expanded", |s| s.expanded = !s.expanded),
        ("images", |s| s.images = Some(1)),
    ];
    for (field, mutate) in mutations {
        let mut changed = base;
        mutate(&mut changed);
        assert_ne!(base, changed, "changing {field} has to change the signature");
    }
}
