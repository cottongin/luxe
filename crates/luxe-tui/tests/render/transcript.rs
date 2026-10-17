//! Transcript — messages, cards and system notices drawn in the conversation, and how a scrolled view holds its place.

use super::*;

/// An empty conversation still draws the composer, where typing will land.
#[test]
fn an_empty_conversation_draws_the_composer() {
    let backend = test_backend(80, 24);
    let mut app = create_app();

    let buffer = render_app(&mut app, &backend);

    let (x, y) = composer_corner(&buffer);
    assert!(y > 0, "the composer sits under the conversation, not at the top: ({x}, {y})");
    assert_eq!(buffer[(x, y)].symbol(), "\u{256d}", "a rounded corner opens the box");
}

/// Messages draw in the order they were said, the reader's first.
#[test]
fn messages_draw_in_the_order_they_were_said() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    app.push_convo(ConvoBlock::User {
        text: "Hello".to_string(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant("Hi there!".to_string()));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));

    let hello =
        rendered.find("Hello").unwrap_or_else(|| panic!("the message is missing: {rendered}"));
    let reply =
        rendered.find("Hi there!").unwrap_or_else(|| panic!("the reply is missing: {rendered}"));
    assert!(hello < reply, "the reader's message comes first: {rendered}");
}

/// Test rendering with a tool call.
#[test]
fn test_render_with_tool_call() {
    let backend = test_backend(80, 24);
    let mut app = create_app();

    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: ToolActivity::running("session-a", "call-a", "bash", "ls -la", 1),
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("ls -la"), "running command missing: {rendered}");
}

#[test]
fn expanded_tool_details_render_and_wrap_inside_the_conversation() {
    let (width, height) = (48, 18);
    let backend = test_backend(width, height);
    let mut app = create_app();
    let mut activity = ToolActivity::running("session-a", "call-a", "bash", "cargo test", 1);
    activity.state = ToolActivityState::Failed;
    activity.input_preview = Some("--package luxe-tui --features detailed-tools".into());
    activity.output_preview =
        Some("tool output must wrap inside the visible conversation pane tail".into());
    activity.summary = Some("validation failed".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: true,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);

    assert!(content.contains("arguments:"), "missing arguments detail: {content}");
    assert!(content.contains("output:"), "missing output detail: {content}");
    assert!(content.contains("pane tail"), "wrapped output tail was clipped: {content}");
    // The outcome rides on the header line — a `result:` row underneath would say it twice.
    assert!(content.contains("validation failed"), "the outcome is on the card: {content}");
    assert!(!content.contains("result:"), "no redundant result row: {content}");
}

/// A question card is one exchange in two rows, so the answer has to land on the *header's*
/// columns rather than under the body indent every other card's payload uses. Measured off the
/// rendered buffer, because the columns are what a reader sees: nothing in the source says `12`.
#[test]
fn a_question_cards_answer_lines_up_with_the_question_it_answers() {
    let (width, height) = (150u16, 34u16);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::ChatQuestion).expect("the fixture");
    app.update(Msg::TerminalResized { width, height });

    let buffer = render_app(&mut app, &backend);
    let rows: Vec<String> = (0..height).map(|y| row_text(&buffer, y, width)).collect();
    let asked = rows
        .iter()
        .find(|row| row.contains("Where should the ingest worker live?"))
        .expect("the question card's header");
    let answered =
        rows.iter().find(|row| row.contains("its own crate")).expect("the answer under it");

    assert_eq!(
        column_of(asked, "question"),
        column_of(answered, "answer"),
        "the labels share a column:\n{asked}\n{answered}"
    );
    assert_eq!(
        column_of(asked, "Where should"),
        column_of(answered, "its own crate"),
        "and so do the words they label:\n{asked}\n{answered}"
    );
}

/// The reader's own decision is drawn in the reader's own color, the one a user turn takes — not in
/// the body color that would leave it indistinguishable from the question above it.
#[test]
fn a_question_cards_answer_is_drawn_in_the_color_a_user_turn_is() {
    let (width, height) = (150u16, 34u16);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::ChatQuestion).expect("the fixture");
    app.update(Msg::TerminalResized { width, height });
    let accent = app.theme_styles.accent.fg.expect("an accent color");
    let text = app.theme_styles.text.fg.expect("a text color");

    let buffer = render_app(&mut app, &backend);
    let (x, y) = marker_position(&buffer, "its own crate", width, height);
    assert_eq!(buffer[(x, y)].fg, accent, "the answer takes the user's color");
    assert_ne!(accent, text, "precondition: the theme draws them differently");
}

#[test]
fn a_running_tool_card_animates_its_spinner_and_the_thumb_takes_the_accent() {
    let backend = test_backend(60, 12);
    let mut app = create_app();
    for index in 0..30 {
        app.push_convo(ConvoBlock::System(format!("filler {index}")));
    }
    let activity = ToolActivity::running("session-a", "call-a", "bash", "cargo test", 1);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let frame_zero = render_app(&mut app, &backend);
    let row_zero: String = frame_zero.content.iter().map(|cell| cell.symbol()).collect();
    app.status.spinner_frame = 4;
    let frame_four = render_app(&mut app, &backend);
    let row_four: String = frame_four.content.iter().map(|cell| cell.symbol()).collect();
    assert_ne!(row_zero, row_four, "the running glyph is frozen across spinner frames");

    // The scrollbar thumb is the positional indicator, so it wears the accent, not the border.
    let accent = app.theme_styles.accent.fg.expect("theme accent has a foreground");
    let border = app.theme_styles.border.fg.expect("theme border has a foreground");
    assert_ne!(accent, border, "fixture theme must distinguish accent from border");
    let thumbs =
        frame_four.content.iter().filter(|cell| cell.symbol() == "\u{2590}").collect::<Vec<_>>();
    assert!(!thumbs.is_empty(), "a taller-than-viewport transcript draws a thumb");
    assert!(
        thumbs.iter().all(|cell| cell.fg == accent),
        "scrollbar thumb should use the theme accent"
    );
}

#[test]
fn scrolling_up_between_renders_is_not_undone_by_anchor_restore() {
    // Every frame used to re-derive the offset from the *previous* frame's anchor whenever follow
    // was off, so each manual scroll was reverted before it could be seen. Drive the real
    // two-frame sequence: render, scroll, render.
    let backend = test_backend(60, 10);
    let mut app = create_app();
    for index in 0..40 {
        app.push_convo(ConvoBlock::System(format!("transcript line {index}")));
    }

    render_app(&mut app, &backend);
    assert!(app.scroll_state.follow, "a fresh transcript follows the tail");
    let bottom = app.scroll_state.offset;

    for _ in 0..5 {
        app.scroll_state.line_up();
    }
    assert_eq!(app.scroll_state.offset, bottom - 5);

    render_app(&mut app, &backend);
    assert_eq!(app.scroll_state.offset, bottom - 5, "the render pass clobbered a manual scroll");

    // And it keeps moving on subsequent scrolls rather than sticking at the first position.
    app.scroll_state.page_up();
    let after_page = app.scroll_state.offset;
    render_app(&mut app, &backend);
    assert_eq!(app.scroll_state.offset, after_page);

    // Scrolling back to the bottom re-arms follow, and new content sticks to the tail again.
    app.scroll_state.to_bottom();
    render_app(&mut app, &backend);
    app.push_convo(ConvoBlock::System("fresh tail".into()));
    render_app(&mut app, &backend);
    assert!(app.scroll_state.follow);
    assert_eq!(app.scroll_state.offset, app.scroll_state.total - app.scroll_state.viewport);
}

#[test]
fn content_growing_above_a_scrolled_viewport_keeps_the_anchored_line() {
    // The other half of the contract: while scrolled up, a block *above* the viewport growing
    // (tool output streaming in) must not shove the reader's position around.
    let backend = test_backend(60, 10);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("first reply".into()));
    for index in 0..30 {
        app.push_convo(ConvoBlock::System(format!("item {index}")));
    }

    render_app(&mut app, &backend);
    for _ in 0..8 {
        app.scroll_state.line_up();
    }
    render_app(&mut app, &backend);
    let anchored = app.conversation_anchor.clone().expect("anchor after scrolling up");

    // Grow the very first block, which sits above the viewport.
    app.convo[0].set_block(ConvoBlock::Assistant("first reply\n\nwith\n\nmore\n\nlines".into()));
    render_app(&mut app, &backend);

    let after = app.conversation_anchor.as_ref().expect("anchor after growth");
    assert_eq!(after.item_id, anchored.item_id, "same item still at the top of the viewport");
    assert_eq!(after.visual_line_within_item, anchored.visual_line_within_item);
}

#[test]
fn expanding_a_tool_above_the_viewport_preserves_anchor_and_selection() {
    let backend = test_backend(60, 10);
    let mut app = create_app();
    let mut activity = ToolActivity::running("session-a", "call-a", "bash", "cargo test", 1);
    activity.input_preview = Some("--package luxe-tui --features detailed-tools".into());
    activity.output_preview = Some("tool output that becomes several wrapped detail lines".into());
    activity.summary = Some("completed".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    app.push_convo(ConvoBlock::Assistant("anchored assistant reply".into()));
    for index in 0..6 {
        app.push_convo(ConvoBlock::System(format!("trailing item {index}")));
    }
    let anchor_id = app.convo[1].id.clone();
    app.selected_block = Some(1);

    render_app(&mut app, &backend);
    // The tool renders as a padded band (top pad, line, bottom pad) and the assistant adds a
    // leading gap, so its first line sits at index 3.
    app.scroll_state.offset = 3;
    app.scroll_state.follow = false;
    app.conversation_anchor = None;
    render_app(&mut app, &backend);
    let before = app.conversation_anchor.clone().expect("assistant anchor before expansion");
    assert_eq!(before.item_id, anchor_id);

    let ConvoBlock::Tool(tool) = app.convo[0].block_mut() else {
        panic!("first item must be a tool");
    };
    tool.expanded = true;
    render_app(&mut app, &backend);

    let after = app.conversation_anchor.as_ref().expect("assistant anchor after expansion");
    assert_eq!(after.item_id, anchor_id);
    assert_eq!(after.screen_row, before.screen_row);
    assert_eq!(app.selected_block, Some(1));
}

#[test]
fn expanding_a_tool_below_the_viewport_preserves_anchor_and_selection() {
    let backend = test_backend(60, 10);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("anchored assistant reply".into()));
    for index in 0..6 {
        app.push_convo(ConvoBlock::System(format!("middle item {index}")));
    }
    let mut activity = ToolActivity::running("session-a", "call-b", "bash", "cargo test", 1);
    activity.input_preview = Some("--package luxe-tui --features detailed-tools".into());
    activity.output_preview = Some("tool output that becomes several wrapped detail lines".into());
    activity.summary = Some("completed".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    let anchor_id = app.convo[0].id.clone();
    let tool_index = app.convo.len() - 1;
    app.selected_block = Some(0);

    render_app(&mut app, &backend);
    app.scroll_state.offset = 0;
    app.scroll_state.follow = false;
    app.conversation_anchor = None;
    render_app(&mut app, &backend);
    let before = app.conversation_anchor.clone().expect("assistant anchor before expansion");
    assert_eq!(before.item_id, anchor_id);

    let ConvoBlock::Tool(tool) = app.convo[tool_index].block_mut() else {
        panic!("last item must be a tool");
    };
    tool.expanded = true;
    render_app(&mut app, &backend);

    let after = app.conversation_anchor.as_ref().expect("assistant anchor after expansion");
    assert_eq!(after.item_id, anchor_id);
    assert_eq!(after.screen_row, before.screen_row);
    assert_eq!(app.selected_block, Some(0));
}

#[test]
fn successful_tool_groups_collapse_and_restore_member_order() {
    // The run the message asked for is what folds, so a card carries the place it was promised.
    fn succeeded(call_id: &str, label: &str, index: u32) -> ToolItem {
        let mut activity = ToolActivity::running("session-a", call_id, "read", label, 1);
        activity.state = ToolActivityState::Succeeded;
        activity.summary = Some("Completed".into());
        activity.run = Some(luxe_core::ToolRun { index, total: 4 });
        ToolItem { activity, expanded: false, details_toggled: false, waited_on_user_ms: 0 }
    }

    let backend = test_backend(80, 20);
    let mut app = create_app();
    for (index, (call_id, label)) in
        [("call-a", "first.rs"), ("call-b", "second.rs"), ("call-c", "third.rs")]
            .into_iter()
            .enumerate()
    {
        let index = u32::try_from(index).unwrap() + 1;
        app.push_convo(ConvoBlock::Tool(Box::new(succeeded(call_id, label, index))));
    }
    let mut failed = succeeded("call-failed", "failure.rs", 4);
    failed.activity.state = ToolActivityState::Failed;
    failed.activity.summary = Some("validation failed".into());
    app.push_convo(ConvoBlock::Tool(Box::new(failed)));

    let collapsed = buffer_to_string(&render_app(&mut app, &backend));
    assert!(collapsed.contains("3 read calls"), "missing collapsed group: {collapsed}");
    // Listed by name under the count, but as a list: no member draws its own card header
    let listed: Vec<&str> = collapsed.lines().filter(|line| line.contains("first.rs")).collect();
    assert_eq!(listed.len(), 1, "the run lists each member once: {collapsed}");
    assert!(!listed[0].contains("read"), "and not as a card of its own: {collapsed}");
    let order = ["first.rs", "second.rs", "third.rs"].map(|name| collapsed.find(name));
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "in order: {collapsed}");
    assert!(collapsed.contains("failure.rs"), "failure must remain standalone: {collapsed}");

    app.selected_block = Some(1);
    assert!(app.toggle_selected_tool_details());
    let expanded = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!expanded.contains("3 read calls"), "opened into its cards: {expanded}");
    let first = expanded.find("first.rs").expect("first group member");
    let second = expanded.find("second.rs").expect("second group member");
    let third = expanded.find("third.rs").expect("third group member");
    assert!(first < second && second < third, "group member order changed: {expanded}");
    assert!(expanded.contains("failure.rs"), "failure must remain standalone: {expanded}");
}

/// Test rendering with an error state.
#[test]
fn test_render_with_error() {
    let backend = test_backend(80, 24);
    let mut app = create_app();

    let mut item = card_with_arguments("bash", "ls", serde_json::json!({"command": "ls"}));
    item.activity.state = ToolActivityState::Failed;
    item.activity.summary = Some("Something went wrong".into());
    app.push_convo(ConvoBlock::Tool(Box::new(item)));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("Something went wrong"), "failure missing: {rendered}");
}

#[test]
fn long_conversation_content_wraps_to_the_visible_width() {
    let (width, height) = (32, 16);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.push_convo(ConvoBlock::System(
        "provider error: this deliberately long message must remain inside the conversation pane END"
            .to_string(),
    ));

    let buffer = render_app(&mut app, &backend);
    let rows = (0..height).map(|y| row_text(&buffer, y, width)).collect::<Vec<_>>();
    let start = rows.iter().position(|row| row.contains("provider error"));
    let end = rows.iter().position(|row| row.contains("END"));

    assert!(start.is_some(), "wrapped message start is missing: {rows:?}");
    assert!(end.is_some(), "wrapped message tail was clipped: {rows:?}");
    assert!(end > start, "long content should occupy more than one row: {rows:?}");
    assert!(app.scroll_state.total >= 3);
}

/// A system message is drawn as written.
#[test]
fn a_system_message_is_drawn_as_written() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    app.push_convo(ConvoBlock::System("Starting session...".to_string()));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));

    assert!(rendered.contains("Starting session..."), "the notice is missing: {rendered}");
}

/// The statusline names the model it was given.
#[test]
fn the_statusline_names_the_model() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    app.status.model = "gpt-4".to_string();
    app.status.context_pct = 50.0;
    app.status.state = luxe_tui::app::AppState::Idle;

    let rendered = buffer_to_string(&render_app(&mut app, &backend));

    assert!(rendered.contains("gpt-4"), "the model is missing: {rendered}");
}

/// What is typed shows in the composer.
#[test]
fn typed_text_shows_in_the_composer() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    app.editor.insert_text("Hello, world!");

    let rendered = buffer_to_string(&render_app(&mut app, &backend));

    assert!(rendered.contains("Hello, world!"), "the draft is missing: {rendered}");
}
