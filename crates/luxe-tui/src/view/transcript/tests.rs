use super::*;
use crate::app::{ConversationItem, ConvoBlock};

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

/// A unit that redraws by itself — a spinner, an image, a row the reveal is still typing —
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

/// Every field of the signature has to be part of the comparison, or the cache serves a stale
/// row the moment that one thing changes on its own. Flipping each in turn is the cheap way to
/// keep a future field from being added to the struct and forgotten in the equality.
#[test]
fn every_field_of_the_signature_is_part_of_the_comparison() {
    let base = signature(3, BlockRevision::for_test(1));
    type Mutation = (&'static str, fn(&mut UnitSignature));
    let mutations: [Mutation; 9] = [
        ("first", |s| s.first += 1),
        ("len", |s| s.len += 1),
        ("revision", |s| s.revision = BlockRevision::for_test(2)),
        ("leading_gap", |s| s.leading_gap = !s.leading_gap),
        ("folded", |s| s.folded = !s.folded),
        ("arriving", |s| s.arriving = !s.arriving),
        ("selected", |s| s.selected = !s.selected),
        ("never_sent", |s| s.never_sent = !s.never_sent),
        ("expanded", |s| s.expanded = !s.expanded),
    ];
    for (field, mutate) in mutations {
        let mut changed = base;
        mutate(&mut changed);
        assert_ne!(base, changed, "changing {field} has to change the signature");
    }
}
