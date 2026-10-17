use super::*;
use tokio::sync::mpsc;

/// `text` with every mention shown as `chip`'s answer for it.
fn shown(text: &str, chip: impl Fn(&MentionRef) -> String) -> (String, Vec<Range<usize>>) {
    display_with_chips(text, &crate::mentions::parse_mentions(text), chip)
}

/// Each chip covers exactly its label, wherever the chips before it moved it.
#[test]
fn adjacent_mentions_each_get_their_own_chip() {
    let (display, chips) = shown("@[file:a.rs]@[file:b.rs] done", |r| format!("[{}]", r.target));
    assert_eq!(display, "[a.rs][b.rs] done");
    let labels: Vec<&str> = chips.iter().map(|chip| &display[chip.clone()]).collect();
    assert_eq!(labels, ["[a.rs]", "[b.rs]"]);
}

/// Chip ranges are bytes, so a chip with a wide, multi-byte glyph still moves the next one by its
/// whole length.
#[test]
fn a_wide_chip_moves_the_next_one_by_its_bytes() {
    let (display, chips) = shown("see @[file:x] and @[file:y]", |r| format!("📄 {}", r.target));
    assert_eq!(display, "see 📄 x and 📄 y");
    let labels: Vec<&str> = chips.iter().map(|chip| &display[chip.clone()]).collect();
    assert_eq!(labels, ["📄 x", "📄 y"]);
}

/// Whitespace trimmed off the front of the message takes the chips with it.
#[test]
fn leading_whitespace_is_trimmed_from_under_the_chips() {
    let (display, chips) = shown("  \n @[file:a] tail ", |r| format!("<{}>", r.target));
    assert_eq!(display, "<a> tail");
    let labels: Vec<&str> = chips.iter().map(|chip| &display[chip.clone()]).collect();
    assert_eq!(labels, ["<a>"]);
}

/// A message the engine never took is marked never sent, as a steer or a queued follow-up that
/// failed already is.
///
/// The break this catches: the message sat in the transcript looking delivered, beside a notice
/// that the engine was gone.
#[tokio::test]
async fn a_message_the_engine_never_took_is_marked_never_sent() {
    let (tx, rx) = mpsc::channel(8);
    drop(rx);
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), config);
    app.status.model = "gpt-5.6".into();
    let image = AttachedImage {
        data: "aW1hZ2U=".into(),
        mime: "image/png".into(),
        filename: "trace.png".into(),
        width: 4,
        height: 4,
    };
    app.editor.prepend_messages([("explain the stall".to_string(), vec![image])]);

    submit_prompt(&mut app);

    let marked = |kind: fn(&ConvoBlock) -> bool| {
        let item = app.convo.iter().find(|item| kind(item.block())).expect("in the transcript");
        app.was_never_sent(&item.id)
    };
    assert!(marked(|block| matches!(block, ConvoBlock::User { .. })), "the message is marked");
    assert!(!marked(|block| matches!(block, ConvoBlock::Image { .. })), "its image is not");
}

/// A draft whose mentions nest still places its chips instead of crashing on submit.
#[test]
fn nested_mention_text_places_chips() {
    let text = "@[file:@[file:x] done";
    let (display, chips) = shown(text, |r| format!("[{}]", r.target));
    assert_eq!(chips.len(), 1, "{display:?}");
    assert!(display.ends_with(" done"), "{display:?}");
}
