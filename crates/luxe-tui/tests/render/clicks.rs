//! Clicks — copy affordances, the composer cursor and prose that is not a button.

use super::*;

/// Each fenced code block renders a muted "copy" caption at its **top right**, where every editor
/// and every chat client puts one; clicking it copies the block's source text. It sits on
/// selectable text, so like every transcript click it acts on the release.
#[test]
fn clicking_a_code_block_copy_affordance_copies_the_code() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.push_convo(ConvoBlock::Assistant(
        "Here is the answer:\n\n```rust\nlet answer = 42;\n```\n".into(),
    ));

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("copy"))
        .expect("the code-block copy affordance renders");
    let code = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("let answer = 42;"))
        .expect("the code renders");
    assert!(row < code, "the affordance is above the block, not a stray word under it");

    // The block is framed: a rule above carrying the language and the copy affordance, a rule
    // below, and the two ending on the same column so they read as one box. The frame may be
    // *wider* than the code — a box around three words is still a box — but never narrower.
    let last_column = |y: u16| {
        (0..width).rev().find(|&x| buffer[(x, y)].symbol().trim() != "").expect("a drawn row")
    };
    let closing = (code + 1..height)
        .find(|&y| row_text(&buffer, y, width).trim().starts_with('\u{2500}'))
        .expect("the block is closed by a rule");
    assert_eq!(
        last_column(row),
        last_column(closing),
        "the frame's rules must line up: {:?} against {:?}",
        row_text(&buffer, row, width).trim_end(),
        row_text(&buffer, closing, width).trim_end(),
    );
    assert!(
        last_column(row) >= last_column(code),
        "the frame is at least as wide as what it frames: {:?}",
        row_text(&buffer, row, width).trim_end(),
    );
    assert!(
        row_text(&buffer, row, width).contains("rust"),
        "the opening rule names the language: {:?}",
        row_text(&buffer, row, width).trim_end(),
    );

    let column = last_column(row) - 1;
    let event = |kind| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    // The affordance sits on selectable text, so it acts on the release — a press there could
    // still be the start of a drag.
    assert!(matches!(
        route_mouse(&mut app, &event(MouseEventKind::Down(MouseButton::Left))),
        RouteAction::None
    ));
    match route_mouse(&mut app, &event(MouseEventKind::Up(MouseButton::Left))) {
        RouteAction::Msg(message) => match *message {
            Msg::CopyText(text) => {
                assert!(text.contains("let answer = 42;"), "copies the code text, got {text:?}")
            }
            other => panic!("expected a CopyText message, got {other:?}"),
        },
        other => panic!("expected a copy message from the click, got {other:?}"),
    }
}

/// Editor click-to-cursor: clicking a cell in the composer places the cursor at that visual
/// position (the inverse of `visual_cursor`) — anchoring a selection there is what moves it.
#[test]
fn clicking_the_composer_places_the_cursor() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.editor.insert_text("hello world");
    assert_eq!(app.editor.cursor(), 11, "cursor starts at the end of the inserted text");

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("hello world"))
        .expect("the composer text renders");
    // Click the 'w' of "world" (offset 6 in "hello world").
    let wx = (0..width).find(|&x| buffer[(x, row)].symbol() == "w").expect("the 'w' cell renders");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: wx,
        row,
        modifiers: KeyModifiers::NONE,
    };
    // The press anchors a selection at that cell, which is what places the caret.
    assert!(matches!(route_mouse(&mut app, &click), RouteAction::None));
    assert_eq!(app.editor.cursor(), 6, "clicking the 'w' cell moves the cursor to offset 6");
}

/// A click on plain transcript prose neither selects nor copies the message: prose is there to be
/// read and dragged across, and a click that copies a whole message behind the reader's back was
/// the single least predictable thing the mouse did.
#[test]
fn clicking_transcript_prose_neither_selects_nor_copies_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.push_convo(ConvoBlock::User {
        text: "first message".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant("second message reply".into()));
    app.push_convo(ConvoBlock::User {
        text: "third message".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("second message"))
        .expect("the second block renders");
    let event = |kind| MouseEvent { kind, column: 10, row, modifiers: KeyModifiers::NONE };
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        let action = route_mouse(&mut app, &event(kind));
        apply(&mut app, action);
    }
    assert_eq!(app.selected_block, None, "a click leaves the keyboard selection alone");
    assert!(app.outbox.copy.is_none(), "and copies nothing");
    render_app(&mut app, &backend);
    assert!(app.outbox.copy.is_none(), "still nothing on the next frame");
}
