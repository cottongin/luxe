//! Selection — thinking output, drags, double clicks and folding by click.

use super::*;

/// Reasoning renders muted + italic: the body carries `Modifier::ITALIC` in the `thinkingText`
/// color, and the collapsed block shows a one-line peek rather than the full body. The styling is
/// the whole cue — no "thinking" banner is printed above it.
#[test]
fn thinking_output_is_muted_italic_with_a_collapsed_peek() {
    use ratatui::style::Modifier;
    let (width, height) = (120, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::ChatThinking).unwrap();
    let thinking_fg = app.theme_styles.thinking_text.fg.expect("thinkingText color");
    app.update(Msg::TerminalResized { width, height });
    let buffer = render_app(&mut app, &backend);

    assert!(
        !buffer_to_string(&buffer).contains("thinking"),
        "the redundant reasoning label is gone"
    );

    // A word only the reasoning body says is italic in the muted color.
    let body_row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("partial"))
        .expect("the reasoning body renders");
    let text = row_text(&buffer, body_row, width);
    let col = text.find("partial").expect("the reasoning token") as u16;
    assert!(
        buffer[(col, body_row)].modifier.contains(Modifier::ITALIC),
        "reasoning renders in italic"
    );
    assert_eq!(buffer[(col, body_row)].fg, thinking_fg, "reasoning uses the thinkingText color");

    // The collapsed block peeks at its rendered first line and says what it is standing in for.
    let content = buffer_to_string(&buffer);
    assert!(
        content.contains("Planning the test · +1 more line"),
        "the collapsed peek shows the section title and the count: {content}"
    );
    assert!(
        !content.contains("single case"),
        "the collapsed block hides the rest of the reasoning body"
    );
}

/// Reasoning is markdown in practice, so it renders as markdown: the `**title**` a GPT model leads
/// each section with becomes a bold title in the reasoning color, and a backticked symbol takes the
/// theme's code hue. The syntax that produced either one never reaches the screen.
#[test]
fn thinking_output_renders_its_markdown_without_showing_the_syntax() {
    use ratatui::style::Modifier;
    let (width, height) = (120, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::ChatThinking).unwrap();
    let thinking_fg = app.theme_styles.thinking_text.fg.expect("thinkingText color");
    let code_fg = app.theme_styles.md_code.fg.expect("mdCode color");
    app.update(Msg::TerminalResized { width, height });
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);

    assert!(content.contains("Tracing the buffering"), "the section title renders: {content}");
    assert!(
        !content.contains("**") && !content.contains('`'),
        "no markdown syntax survives into the transcript: {content}"
    );

    let cell = |needle: &str| {
        let row = (0..height)
            .find(|&y| row_text(&buffer, y, width).contains(needle))
            .unwrap_or_else(|| panic!("{needle} renders: {content}"));
        let col = row_text(&buffer, row, width).find(needle).expect("the column") as u16;
        &buffer[(col, row)]
    };

    let title = cell("Tracing the buffering");
    assert!(title.modifier.contains(Modifier::BOLD), "a reasoning title is bold");
    assert!(title.modifier.contains(Modifier::ITALIC), "and stays in the reasoning voice");
    assert_eq!(title.fg, thinking_fg, "a title spends weight rather than a color");

    // `Parser::feed` appears in both an answer and the reasoning above it, so pick the reasoning
    // line by something only it says.
    let code_row = (0..height)
        .find(|&y| {
            let text = row_text(&buffer, y, width);
            text.contains("Parser::feed") && text.contains("whole input")
        })
        .expect("the reasoning line naming the symbol");
    let code_col = row_text(&buffer, code_row, width).find("Parser::feed").expect("the column");
    let code = &buffer[(code_col as u16, code_row)];
    assert_eq!(code.fg, code_fg, "a backticked symbol keeps the code hue inside reasoning");
    assert!(code.modifier.contains(Modifier::ITALIC), "and is still italic");

    // A list the model wrote renders as one, in the reasoning color.
    let bullet = cell("• a multi-byte token");
    assert_eq!(bullet.fg, thinking_fg, "a reasoning list marker takes the reasoning color");
}

/// Capturing the mouse takes the terminal's own click-drag selection away, so a drag over the
/// transcript must select, highlight, and copy in-app.
#[test]
fn dragging_over_the_transcript_selects_highlights_and_copies() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (60, 12);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("alpha bravo charlie".into()));
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("alpha bravo charlie"))
        .expect("the reply renders");
    let start_col = column_of(&row_text(&buffer, row, width), "bravo");

    let event = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let press = event(MouseEventKind::Down(MouseButton::Left), start_col, row);
    let action = route_mouse(&mut app, &press);
    apply(&mut app, action);
    assert!(app.pointer.selection_drag.is_some(), "a press on the transcript anchors a selection");

    let drag = event(MouseEventKind::Drag(MouseButton::Left), start_col + 5, row);
    let action = route_mouse(&mut app, &drag);
    apply(&mut app, action);
    let selection = app.pointer.selection.clone().expect("the drag records a selection");
    assert!(!selection.grid.is_empty());

    // The dragged cells render with the selection background.
    let buffer = render_app(&mut app, &backend);
    let selected_bg = app.theme_styles.selected_bg.bg.expect("selection background");
    assert_eq!(buffer[(start_col, row)].bg, selected_bg, "the first selected cell is highlighted");
    assert_ne!(
        buffer[(start_col + 6, row)].bg,
        selected_bg,
        "cells past the selection stay unstyled"
    );

    // Releasing copies, exactly like a terminal's copy-on-select, and says so — the clipboard
    // itself is invisible.
    let release = event(MouseEventKind::Up(MouseButton::Left), start_col + 5, row);
    let action = route_mouse(&mut app, &release);
    apply(&mut app, action);
    let buffer = render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"));
    assert!(app.pointer.selection_drag.is_none(), "the drag ends on release");
    assert!(
        buffer_to_string(&buffer).contains("Copied to clipboard"),
        "the row above the prompt should confirm the copy: {}",
        buffer_to_string(&buffer)
    );

    // Esc drops the selection instead of reaching the composer.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.pointer.selection.is_none(), "Esc clears the selection");
}

/// An overlay drawn over the transcript takes it out of reach: a press on the overlay's own padding
/// must not start a selection in the message hidden behind it, and a drag there must not copy it.
#[test]
fn an_open_overlay_stops_the_transcript_offering_text_and_clicks() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.push_convo(ConvoBlock::Assistant("alpha bravo charlie".into()));
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("alpha bravo charlie"))
        .expect("the reply renders");

    // Ctrl+K opens the command palette over the chat surface.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    render_app(&mut app, &backend);

    let event = |kind| MouseEvent { kind, column: 4, row, modifiers: KeyModifiers::NONE };
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        let action = route_mouse(&mut app, &event(kind));
        apply(&mut app, action);
    }
    assert!(app.pointer.selection.is_none(), "no selection starts behind the overlay");
    render_app(&mut app, &backend);
    assert!(app.outbox.copy.is_none(), "and nothing hidden is copied");
}

/// A selection has to survive the transcript moving under it. Output arriving, a card opening — the
/// line indices it was made from are stale within a frame, so each end is pinned to the transcript
/// line it landed on and re-found every frame. Dropping the selection instead made it impossible to
/// copy anything out of a session that was still streaming.
#[test]
fn a_transcript_selection_survives_new_output_above_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (60, 20);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("alpha bravo charlie".into()));
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("alpha bravo charlie"))
        .expect("the reply renders");
    let start_col = column_of(&row_text(&buffer, row, width), "bravo");

    let event = |kind, column| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let action = route_mouse(&mut app, &event(MouseEventKind::Down(MouseButton::Left), start_col));
    apply(&mut app, action);
    let action =
        route_mouse(&mut app, &event(MouseEventKind::Drag(MouseButton::Left), start_col + 5));
    apply(&mut app, action);
    let before = app.pointer.selection.clone().expect("the drag records a selection");

    // Two new blocks land above nothing and below everything, and the *first* block grows — which
    // moves every rendered line under the selection.
    app.convo[0]
        .set_block(ConvoBlock::Assistant("alpha bravo charlie\n\nand a second paragraph".into()));
    app.push_convo(ConvoBlock::System("a later line".into()));

    let buffer = render_app(&mut app, &backend);
    let after = app.pointer.selection.clone().expect("the selection survives the reflow");
    assert_eq!(after.pins, before.pins, "it is pinned to the same transcript lines");
    assert_eq!(
        after.grid.bounds().0.column,
        before.grid.bounds().0.column,
        "and covers the same columns"
    );

    // Still the same cells on screen, and still what the release copies.
    let moved_row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("alpha bravo charlie"))
        .expect("the reply still renders");
    let selected_bg = app.theme_styles.selected_bg.bg.expect("selection background");
    assert_eq!(buffer[(start_col, moved_row)].bg, selected_bg, "the same word stays highlighted");

    let action =
        route_mouse(&mut app, &event(MouseEventKind::Up(MouseButton::Left), start_col + 5));
    apply(&mut app, action);
    render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"));
}

/// Double-click grabs the word under the pointer, triple-click the whole line — the two gestures
/// mouse capture takes away from the terminal. Both copy on release, like any other selection.
#[test]
fn a_double_click_takes_the_word_and_a_triple_click_the_line() {
    let (width, height) = (60, 12);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("call Parser::feed on it".into()));
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Parser::feed"))
        .expect("the reply renders");
    let column = column_of(&row_text(&buffer, row, width), "Parser") + 3;
    let now = std::time::Instant::now();

    // Chained presses on one cell escalate: the first is a caret, the second the word.
    click_cell(&mut app, column, row, now);
    assert!(app.outbox.copy.is_none(), "one click selects nothing to copy");
    click_cell(&mut app, column, row, now);
    render_app(&mut app, &backend);
    assert_eq!(
        app.outbox.copy.as_deref(),
        Some("Parser::feed"),
        "a double-click takes the whole symbol, colons included"
    );

    // A third press takes the line — padding trimmed, since nobody wants the band on the clipboard.
    app.outbox.copy = None;
    click_cell(&mut app, column, row, now);
    render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("call Parser::feed on it"));

    // Far enough apart, the chain breaks and each press is its own caret again.
    app.outbox.copy = None;
    click_cell(&mut app, column, row, now + std::time::Duration::from_secs(1));
    render_app(&mut app, &backend);
    assert!(app.outbox.copy.is_none(), "a late click is a fresh single click");
}

/// The composer answers the same two gestures, over its own buffer.
#[test]
fn a_double_click_in_the_composer_takes_the_word_under_it() {
    let (width, height) = (60, 12);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.editor.insert_text("edit crates/luxe-tui/src/view.rs now");
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("view.rs"))
        .expect("the draft renders");
    let column = column_of(&row_text(&buffer, row, width), "crates") + 2;
    let now = std::time::Instant::now();

    click_cell(&mut app, column, row, now);
    click_cell(&mut app, column, row, now);
    assert_eq!(
        app.outbox.copy.as_deref(),
        Some("crates/luxe-tui/src/view.rs"),
        "a double-click takes the whole path"
    );

    app.outbox.copy = None;
    click_cell(&mut app, column, row, now);
    assert_eq!(app.editor.selected_text().as_deref(), Some("edit crates/luxe-tui/src/view.rs now"));
}

/// A click on a block that is holding something back opens it — the mouse equivalent of the expand
/// chord, and the reason a truncated card's footer names both.
#[test]
fn clicking_a_truncated_card_expands_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let content = (1..=30).map(|n| format!("let value_{n} = {n};")).collect::<Vec<_>>().join("\n");
    let mut card = card_with_arguments(
        "write",
        "src/generated.rs",
        serde_json::json!({"path": "src/generated.rs", "content": content}),
    );
    card.activity.output_preview = Some("Created 'src/generated.rs' (620 bytes)".into());
    app.push_convo(ConvoBlock::Tool(Box::new(card)));

    let buffer = render_app(&mut app, &backend);
    assert!(!buffer_to_string(&buffer).contains("let value_11"), "the card starts truncated");
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("write  src/generated.rs"))
        .expect("the card header renders");

    let press = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let now = std::time::Instant::now();
    let action = route_mouse_at(&mut app, &press, now);
    assert!(matches!(action, RouteAction::None), "the press only anchors a selection");
    apply(&mut app, action);
    let release = MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..press };
    let action = route_mouse_at(&mut app, &release, now);
    apply(&mut app, action);

    let buffer = render_app(&mut app, &backend);
    assert!(
        buffer_to_string(&buffer).contains("let value_11"),
        "the click opened the card: {}",
        buffer_to_string(&buffer)
    );
    assert!(app.outbox.copy.is_none(), "opening a card copies nothing");

    // A later click on the header closes it again. The card grew and a short transcript grows
    // upward, so the header has to be found again each time rather than assumed to have stayed put.
    let row_of = |app: &mut App, needle: &str| {
        let buffer = render_app(app, &backend);
        (0..height)
            .find(|&y| row_text(&buffer, y, width).contains(needle))
            .unwrap_or_else(|| panic!("{needle} renders: {}", buffer_to_string(&buffer)))
    };

    let header = row_of(&mut app, "write  src/generated.rs");
    click_cell(&mut app, 10, header, now + std::time::Duration::from_secs(1));
    let buffer = render_app(&mut app, &backend);
    assert!(!buffer_to_string(&buffer).contains("let value_11"), "a second click re-collapses it");

    // A click on the card's *body* selects text instead of folding it away: collapsing the card out
    // from under a double-click is how you make a path in it impossible to grab.
    let header = row_of(&mut app, "write  src/generated.rs");
    click_cell(&mut app, 10, header, now + std::time::Duration::from_secs(2));
    let body = row_of(&mut app, "let value_2 = 2;");
    click_cell(&mut app, 10, body, now + std::time::Duration::from_secs(3));
    let buffer = render_app(&mut app, &backend);
    assert!(buffer_to_string(&buffer).contains("let value_11"), "the card stays open");
}

/// The footer says "click or Ctrl+O to expand", so clicking the footer has to expand. A hint that
/// names an affordance the row itself does not offer is worse than no hint.
#[test]
fn clicking_the_truncation_footer_expands_the_card() {
    let (width, height) = (100, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let content = (1..=30).map(|n| format!("let value_{n} = {n};")).collect::<Vec<_>>().join("\n");
    app.push_convo(ConvoBlock::Tool(Box::new(card_with_arguments(
        "write",
        "src/generated.rs",
        serde_json::json!({"path": "src/generated.rs", "content": content}),
    ))));

    let buffer = render_app(&mut app, &backend);
    let footer = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("click or ctrl+o to expand"))
        .expect("the truncation footer renders");
    click_cell(&mut app, 10, footer, std::time::Instant::now());

    let buffer = render_app(&mut app, &backend);
    assert!(
        buffer_to_string(&buffer).contains("let value_30"),
        "the footer's own click opened the card: {}",
        buffer_to_string(&buffer)
    );
}

/// The same click opens a collapsed reasoning block, which is the other thing in the transcript
/// with a body behind a one-line peek.
#[test]
fn clicking_a_collapsed_reasoning_block_expands_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.push_convo(ConvoBlock::Thinking {
        text: "**Checking the config**\n\nStart with the loader, then the merge order.".into(),
        expanded: false,
    });

    let buffer = render_app(&mut app, &backend);
    assert!(!buffer_to_string(&buffer).contains("merge order"), "it starts collapsed");
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Checking the config"))
        .expect("the peek renders");

    let event = |kind| MouseEvent { kind, column: 5, row, modifiers: KeyModifiers::NONE };
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        let action = route_mouse(&mut app, &event(kind));
        apply(&mut app, action);
    }
    let buffer = render_app(&mut app, &backend);
    assert!(
        buffer_to_string(&buffer).contains("merge order"),
        "the click expanded the reasoning: {}",
        buffer_to_string(&buffer)
    );
}

/// The composer owns its own selection (buffer offsets), highlights it, and copies on release.
#[test]
fn dragging_in_the_composer_selects_and_copies_the_draft_text() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (60, 12);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.editor.insert_text("select this text");
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("select this text"))
        .expect("the draft renders");
    let start_col = column_of(&row_text(&buffer, row, width), "this");

    let event = |kind, column| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let action = route_mouse(&mut app, &event(MouseEventKind::Down(MouseButton::Left), start_col));
    apply(&mut app, action);
    let action =
        route_mouse(&mut app, &event(MouseEventKind::Drag(MouseButton::Left), start_col + 9));
    apply(&mut app, action);
    assert_eq!(app.editor.selected_text().as_deref(), Some("this text"));

    let buffer = render_app(&mut app, &backend);
    let selected_bg = app.theme_styles.selected_bg.bg.expect("selection background");
    assert_eq!(buffer[(start_col, row)].bg, selected_bg, "the composer paints its selection");

    let action =
        route_mouse(&mut app, &event(MouseEventKind::Up(MouseButton::Left), start_col + 9));
    apply(&mut app, action);
    assert_eq!(app.outbox.copy.as_deref(), Some("this text"));

    // Typing replaces the selection, the way every other editor behaves.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE)));
    assert_eq!(app.editor.current_input(), "select X");
}

/// Hovering a list row moves the same highlight the arrow keys would.
#[test]
fn hovering_a_picker_row_moves_the_keyboard_selection() {
    use crossterm::event::{MouseEvent, MouseEventKind};
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::Key(key(KeyCode::Char('/'))));
    let buffer = render_app(&mut app, &backend);

    let rows: Vec<u16> =
        (0..height).filter(|&y| row_text(&buffer, y, width).contains('❯')).collect();
    assert_eq!(rows.len(), 1, "exactly one row carries the selection cursor");
    let selected_row = rows[0];
    let other_row = selected_row + 1;

    let moved = MouseEvent {
        kind: MouseEventKind::Moved,
        column: 4,
        row: other_row,
        modifiers: KeyModifiers::NONE,
    };
    let action = route_mouse(&mut app, &moved);
    apply(&mut app, action);
    let buffer = render_app(&mut app, &backend);
    assert!(
        row_text(&buffer, other_row, width).contains('❯'),
        "the hovered row takes the selection cursor"
    );
    assert!(
        !row_text(&buffer, selected_row, width).contains('❯'),
        "the previously selected row gives it up"
    );
}
