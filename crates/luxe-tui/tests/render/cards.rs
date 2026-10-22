//! Cards — tool cards, their diffs and what a rejected edit says.

use super::*;

/// Tool cards read as a sentence, not a payload, and edit results render as a line-numbered diff
/// with elision instead of `@@` hunk headers.
#[test]
fn tool_cards_show_human_labels_and_line_numbered_diffs() {
    let (width, height) = (100, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();

    // A tool with no path/command/query key used to fall back to its raw argument JSON.
    let arguments = serde_json::json!({"action": "add", "subject": "wire the dock"});
    let label = luxe_core::describe_call("todo", &arguments, "/project");
    let mut todo = ToolActivity::running("session-a", "call-todo", "todo", label, 1);
    todo.state = ToolActivityState::Succeeded;
    todo.summary = Some("Completed".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: todo,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let mut edit = ToolActivity::running("session-a", "call-edit", "edit", "src/lib.rs", 1);
    edit.state = ToolActivityState::Succeeded;
    edit.summary = Some("Completed".into());
    edit.output_preview = Some(
        "Applied 1 edit(s) to 'src/lib.rs'\n     ...\n 11 fn main() {\n-12     old();\n+12     new();\n 13 }\n     ..."
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: edit,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("add · wire the dock"), "human tool label: {content}");
    assert!(!content.contains("\"action\""), "no raw argument JSON: {content}");
    assert!(!content.contains("@@"), "no unified hunk headers on the card: {content}");
    assert!(content.contains("-12"), "removed line carries its number: {content}");
    assert!(content.contains("+12"), "added line carries its number: {content}");
    assert!(content.contains('⋯'), "elided context is marked: {content}");
    assert!(!content.contains("Completed"), "a green card already says it completed: {content}");
    assert!(
        !content.contains("Applied 1 edit"),
        "the header already names the file it edited: {content}"
    );
}

/// Builds a succeeded card for `tool`, with a body taken from the call's arguments the way the live
/// projection does.
pub(super) fn card_with_arguments(
    tool: &str,
    label: &str,
    arguments: serde_json::Value,
) -> ToolItem {
    let mut activity = ToolActivity::running("session-a", "call-a", tool, label, 1);
    activity.state = ToolActivityState::Succeeded;
    activity.summary = Some("Completed".into());
    activity.card_body = luxe_core::tool_card_body(tool, &arguments, "/project");
    ToolItem { activity, expanded: false, details_toggled: false, waited_on_user_ms: 0 }
}

/// A `write` shows the file it wrote, highlighted as that language, and accounts for what it hid.
///
/// It used to render the tool's *result* — a wall of green `+` rows capped at the model's token
/// budget, with no language to highlight and no honest count of what was cut.
#[test]
fn a_write_card_shows_its_content_and_admits_what_it_hid() {
    let backend = test_backend(100, 40);
    let mut app = create_app();
    let content = (1..=30).map(|n| format!("let value_{n} = {n};")).collect::<Vec<_>>().join("\n");
    let mut card = card_with_arguments(
        "write",
        "src/generated.rs",
        serde_json::json!({"path": "src/generated.rs", "content": content}),
    );
    card.activity.output_preview = Some("Created 'src/generated.rs' (620 bytes)".into());
    app.push_convo(ConvoBlock::Tool(Box::new(card)));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("write  src/generated.rs"), "{rendered}");
    assert!(rendered.contains("Created · 620 B"), "the header says what it did: {rendered}");
    assert!(rendered.contains("1 let value_1 = 1;"), "content is numbered, not signed: {rendered}");
    assert!(!rendered.contains("+1 let value_1"), "a write is not a wall of additions: {rendered}");
    assert!(
        rendered.contains("20 more lines · 30 total · click or ctrl+o to expand"),
        "the footer accounts for the rest and names both ways in: {rendered}"
    );
    assert!(!rendered.contains("let value_11"), "the eleventh line is past the budget: {rendered}");
}

/// Ctrl+O opens every truncated card at once, and closes them again.
#[test]
fn toggling_tool_output_expands_every_card() {
    let backend = test_backend(100, 60);
    let mut app = create_app();
    let content = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    app.push_convo(ConvoBlock::Tool(Box::new(card_with_arguments(
        "write",
        "notes.txt",
        serde_json::json!({"path": "notes.txt", "content": content}),
    ))));

    let collapsed = buffer_to_string(&render_app(&mut app, &backend));
    assert!(collapsed.contains("10 more lines · 20 total"), "{collapsed}");
    assert!(!collapsed.contains("line 20"));

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)));
    let expanded = buffer_to_string(&render_app(&mut app, &backend));
    assert!(expanded.contains("line 20"), "ctrl+o opened the card: {expanded}");
    assert!(!expanded.contains("more lines"), "nothing left to account for: {expanded}");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)));
    let recollapsed = buffer_to_string(&render_app(&mut app, &backend));
    assert!(recollapsed.contains("10 more lines · 20 total"), "and closed it again: {recollapsed}");
}

/// A multi-line `bash` command is shown in full, and the header stops trying to preview it.
#[test]
fn a_multiline_command_is_shown_under_its_card() {
    let backend = test_backend(100, 30);
    let mut app = create_app();
    let command = "cd /tmp && python3 - <<'PY'\nimport json\nprint(json.dumps({'ok': True}))\nPY";
    app.push_convo(ConvoBlock::Tool(Box::new(card_with_arguments(
        "bash",
        &luxe_core::describe_call("bash", &serde_json::json!({"command": command}), "/project"),
        serde_json::json!({"command": command}),
    ))));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("print(json.dumps"), "the heredoc is no longer hidden: {rendered}");
    assert!(
        !rendered.contains("cd /tmp && python3 - <<'PY' …"),
        "the header's ellipsized peek would sit directly above the real thing: {rendered}"
    );
    assert!(rendered.contains("cd /tmp && python3 - <<'PY'"), "which the body draws: {rendered}");
    // A command is not a file, so it gets no line-number gutter.
    assert!(!rendered.contains("2 import json"), "{rendered}");

    // A one-liner says everything on its header already.
    let mut single = create_app();
    single.push_convo(ConvoBlock::Tool(Box::new(card_with_arguments(
        "bash",
        "cargo build",
        serde_json::json!({"command": "cargo build"}),
    ))));
    let one_line = buffer_to_string(&render_app(&mut single, &backend));
    assert_eq!(one_line.matches("cargo build").count(), 1, "said once, not twice: {one_line}");
}

/// A `cd` into the directory the shell already starts in never reaches the card.
///
/// Most of what a model writes opens with one, and on a real project path it is fifty characters of
/// nothing in front of the command the reader came for — wide enough on a narrow card to push the
/// command off the end of the line. The sibling above proves the other half: a `cd` that actually
/// goes somewhere is kept.
#[test]
fn a_card_drops_a_cd_into_the_directory_the_command_already_ran_in() {
    let backend = test_backend(100, 30);
    let root = "/home/dev/project";
    let command = format!("cd {root} && cargo build --workspace");

    let mut app = create_app();
    app.push_convo(ConvoBlock::Tool(Box::new(card_with_arguments(
        "bash",
        &luxe_core::describe_call("bash", &serde_json::json!({ "command": command }), root),
        serde_json::json!({ "command": command }),
    ))));

    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("cargo build --workspace"), "the command survives: {rendered}");
    assert!(!rendered.contains("cd "), "and the no-op does not: {rendered}");
}

/// A wide card draws its diff side by side; a narrow one stacks. Same rows, same numbers.
#[test]
fn a_card_diff_splits_when_wide_and_stacks_when_narrow() {
    let preview = "Applied 1 edit(s) to 'src/lib.rs'\n     ...\n 11 fn main() {\n\
                   -12     let label: String = category.to_string();\n\
                   +12     let label = category.display_name();\n 13 }";
    let card = |width: u16| {
        let mut app = create_app();
        let mut edit = ToolActivity::running("session-a", "call-edit", "edit", "src/lib.rs", 1);
        edit.state = ToolActivityState::Succeeded;
        edit.output_preview = Some(preview.into());
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity: edit,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
        buffer_to_string(&render_app(&mut app, &test_backend(width, 24)))
    };

    // 197 columns is a full-screen terminal; the rule puts old beside new on one row.
    let wide = card(197);
    let paired =
        wide.lines().find(|line| line.contains("category.to_string")).expect("the replacement row");
    assert!(paired.contains('│'), "a rule divides the columns: {paired:?}");
    assert!(
        paired.contains("category.display_name"),
        "old and new share one row when wide: {paired:?}"
    );

    // At 100 columns each side would hold half a line of Rust, so it stacks instead.
    let narrow = card(100);
    let removed =
        narrow.lines().find(|line| line.contains("category.to_string")).expect("the removal");
    assert!(!removed.contains("display_name"), "a narrow card stacks: {removed:?}");
    assert!(narrow.contains("-12") && narrow.contains("+12"), "both rows are there: {narrow}");
}

/// A tab-indented file draws on the same columns as a space-indented one, down to the buffer.
///
/// `unicode-width` scores a tab nothing and the terminal advances it to a stop, so a card that let
/// one through laid its columns out against a width the screen did not agree with. The rule stepped
/// right once per level of indentation, and the surplus pushed the row past the card — where the
/// transcript's word wrap broke it at a space, dropped the leading padding, and stranded the tail at
/// the left margin: an addition drawn inside the column reserved for removals, with no gutter and no
/// continuation marker to say what it was.
///
/// Through the real render path, because that is where the two measurements meet: the card lays a
/// row out, and the transcript wraps whatever the card handed it.
#[test]
fn a_tab_indented_card_diff_draws_on_the_columns_it_was_laid_out_for() {
    // Svelte markup, tab-indented the way prettier writes it. Every tab starts on a stop, so the
    // space-indented twin below is the same file once the tabs are spent.
    const TABBED: &str = concat!(
        "Applied 1 edit(s) to 'TopbarProfileMenu.svelte'\n",
        " 415 \t\t\t<GettingStartedChecklist onCloseDrawer={closeDrawer} />\n",
        " 416 \t\t\t</div>\n",
        "-417 \t\t\t<!-- Tour launcher (desktop only, mobile needs its own tour) -->\n",
        "-418 \t\t\t<div class=\"shrink-0 px-2.5 pb-1 max-sm:hidden\">\n",
        "-419 \t\t\t\t<button type=\"button\" onclick={() => tourStore.startTour()}>\n",
        "+417 \t\t\t<!-- Tour launcher (desktop only, mobile needs its own tour). -->\n",
        "+418 \t\t\t<!-- Hidden while tourStore.isEnabled is false (see tour-store.svelte.ts). -->\n",
        "+419 \t\t\t{#if tourStore.isEnabled}",
    );

    let card = |preview: &str| {
        let mut app = create_app();
        let mut edit =
            ToolActivity::running("session-a", "call-edit", "edit", "TopbarProfileMenu.svelte", 1);
        edit.state = ToolActivityState::Succeeded;
        edit.output_preview = Some(preview.to_string());
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity: edit,
            expanded: true,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
        buffer_to_string(&render_app(&mut app, &test_backend(180, 30)))
    };

    let tabbed = card(TABBED);
    assert_eq!(tabbed, card(&TABBED.replace('\t', "    ")), "a tab is four cells or it is a bug");

    // The card body: everything under the header, down to the blank line that ends the block.
    let rows: Vec<&str> = tabbed.lines().collect();
    let header = rows
        .iter()
        .position(|row| row.contains("TopbarProfileMenu.svelte"))
        .expect("the card header");
    let body: Vec<&str> =
        rows[header + 1..].iter().copied().take_while(|row| !row.trim().is_empty()).collect();
    assert!(body.len() >= 5, "the diff drew its rows: {tabbed}");

    // Every row of a side-by-side body carries the rule, wrapped rows included — both columns are
    // padded so it cannot be otherwise. A row without one is a row the transcript re-wrapped, which
    // is the tail of one column left sitting at the margin in front of the other.
    for row in &body {
        assert!(row.contains('\u{2502}'), "a row escaped its column: {row:?}");
    }
    let columns: std::collections::HashSet<u16> =
        body.iter().map(|row| column_of(row, "\u{2502}")).collect();
    assert_eq!(columns.len(), 1, "the rule stepped with the indentation: {columns:?}");
}

/// `unified` is a real override: a wide card honors it.
#[test]
fn the_unified_preference_survives_a_wide_card() {
    let config = Config {
        diff: luxe_core::config::DiffConfig {
            layout: Some(luxe_core::config::DiffLayout::Unified),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = create_app_with_config(config);
    let mut edit = ToolActivity::running("session-a", "call-edit", "edit", "src/lib.rs", 1);
    edit.state = ToolActivityState::Succeeded;
    edit.output_preview = Some(" 11 fn main() {\n-12     old();\n+12     new();\n 13 }".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: edit,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let rendered = buffer_to_string(&render_app(&mut app, &test_backend(197, 24)));
    let removed = rendered.lines().find(|line| line.contains("old();")).expect("the removal");
    assert!(!removed.contains("new();"), "unified means unified, even at 197 columns: {removed:?}");
}

/// The footer advertises expansion, so the advertised path has to work: select a card, open it.
///
/// Goes through the production event, because a card's identity comes from the activity projection —
/// and per-card expansion keys off exactly that identity.
#[test]
fn selecting_a_card_by_keyboard_and_opening_it_works() {
    let backend = test_backend(100, 40);
    let mut app = create_app();
    app.update(Msg::Engine(luxe_core::event::Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: Vec::new(),
    }));
    let content = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let card = card_with_arguments(
        "write",
        "notes.txt",
        serde_json::json!({"path": "notes.txt", "content": content}),
    );
    app.update(Msg::Engine(luxe_core::event::Event::ToolActivityUpdated(Box::new(card.activity))));

    // Alt+K was bound to "select block up" and dispatched by nobody, so this was unreachable.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT)));
    assert_eq!(app.selected_block, Some(0), "the card is selected");

    // A selected card is still a card: the band it is drawn in survives the highlight, which is
    // what tells a reader it is a tool call at all.
    let buffer = render_app(&mut app, &backend);
    let selected_bg = app.theme_styles.selected_bg.bg.expect("a selection background");
    let header = (0..40)
        .find(|&y| row_text(&buffer, y, 100).contains("write  notes.txt"))
        .expect("the card header renders");
    assert_eq!(buffer[(2, header)].bg, selected_bg, "the band takes the selection tint");
    assert_eq!(
        buffer[(90, header)].bg,
        selected_bg,
        "and still runs the full width of the card, inset and all"
    );
    let pad = header.checked_sub(1).expect("a padding row above the header");
    assert_eq!(buffer[(2, pad)].bg, selected_bg, "including its padding rows");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains("line 20"), "the card opened: {rendered}");
}

/// A rejected edit produced no diff, so the card has to fall back to what the tool said — the
/// guard's own message — instead of rendering nothing but a header and a duration.
#[test]
fn a_rejected_edit_says_why_on_its_card() {
    let (width, height) = (100, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();

    let mut edit = ToolActivity::running("session-a", "call-edit", "edit", "src/lib.rs", 1);
    edit.state = ToolActivityState::Failed;
    edit.summary = Some("edited before reading the file".into());
    // A guard rejection is faster than the millisecond clock, which is exactly the case that used
    // to read as "nothing happened".
    edit.elapsed_ms = Some(0);
    edit.output_preview = Some(
        "Error: read 'src/lib.rs' before editing it (read-before-edit guard). Use the `read` \
         tool first, then retry."
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: edit,
        expanded: true,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("edited before reading"), "the card names the reason: {content}");
    assert!(content.contains("read-before-edit guard"), "and shows the tool's words: {content}");
    assert!(content.contains("<1ms"), "a sub-millisecond call says so: {content}");
    assert!(!content.contains("0ms"), "never `0ms`, which reads as 'never ran': {content}");
}

/// A cancelled or failed turn must not silently start the queued follow-ups, and must not leave
/// them stuck behind a pending marker either — they come back to the composer instead.
///
/// Feeding a failing provider more turns compounds the failure, and a queue the user cannot see or
/// reach is worse still: the message is handed back where one key re-sends it.
#[test]
fn cancelling_or_failing_a_turn_returns_queued_messages_instead_of_running_them() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);

    for stop in ["cancel", "error"] {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
        let mut app = App::new(tx, KeyMap::defaults(), Config::default());
        app.update(Msg::Engine(luxe_core::event::Event::TokenDelta {
            block: 0,
            delta: "working".into(),
        }));
        app.editor.insert_text("follow-up work");
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
        assert_eq!(app.queued_count(), 1, "{stop}: queued");
        let _ = rx.try_recv(); // drain nothing-expected

        match stop {
            "cancel" => app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::Cancel)),
            _ => app.update(Msg::Engine(luxe_core::event::Event::Error("provider down".into()))),
        }

        assert_eq!(app.queued_count(), 0, "{stop}: nothing is left waiting");
        assert!(
            !matches!(rx.try_recv(), Ok(luxe_core::engine::Action::SubmitUserMessage { .. })),
            "{stop}: nothing was promoted"
        );
        assert_eq!(
            app.editor.current_input(),
            "follow-up work",
            "{stop}: the message is back in the composer"
        );
        let buffer = render_app(&mut app, &backend);
        let content = buffer_to_string(&buffer);
        assert!(!content.contains("queued #"), "{stop}: and the tray row is gone: {content}");
    }
}

/// Dragging past the transcript's edges scrolls it; straying sideways does not.
#[test]
fn dragging_past_the_transcript_edge_scrolls_but_sideways_does_not() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (160, 16);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("drag-edges"));
    for index in 0..40 {
        app.push_convo(ConvoBlock::System(format!("line {index}")));
    }
    let buffer = render_app(&mut app, &backend);

    // Start a drag on a transcript row (the transcript sits right of the dock rail).
    let transcript_x = width - 10;
    let first_row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("line "))
        .expect("the transcript renders");
    let event = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let action = route_mouse(
        &mut app,
        &event(MouseEventKind::Down(MouseButton::Left), transcript_x, first_row + 1),
    );
    apply(&mut app, action);
    assert!(app.pointer.selection_drag.is_some(), "the press anchored a transcript selection");

    // Sideways over the dock rail: no scroll.
    let before = app.scroll_state.offset;
    let action =
        route_mouse(&mut app, &event(MouseEventKind::Drag(MouseButton::Left), 2, first_row + 1));
    apply(&mut app, action);
    assert_eq!(app.scroll_state.offset, before, "a sideways drag must not scroll");

    // Above the top row: scrolls up.
    let action =
        route_mouse(&mut app, &event(MouseEventKind::Drag(MouseButton::Left), transcript_x, 0));
    apply(&mut app, action);
    assert!(app.scroll_state.offset < before, "dragging above the pane scrolls up");
}

/// A copy confirms itself on screen, and the terminal's copy chords reach that path when the
/// terminal passes them through (it only holds them back when *it* has a selection).
#[test]
fn a_copy_is_confirmed_above_the_prompt_and_bound_to_the_terminal_copy_chords() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (80, 14);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("alpha bravo charlie".into()));
    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("alpha bravo charlie"))
        .expect("the reply renders");
    let start_col = column_of(&row_text(&buffer, row, width), "bravo");

    // Ctrl+Insert (what Hyprland's Super+C lowers to) copies the live selection.
    let event = |kind, column| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let action = route_mouse(&mut app, &event(MouseEventKind::Down(MouseButton::Left), start_col));
    apply(&mut app, action);
    let action =
        route_mouse(&mut app, &event(MouseEventKind::Drag(MouseButton::Left), start_col + 5));
    apply(&mut app, action);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Insert, KeyModifiers::CONTROL)));
    render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"), "ctrl+insert copies the selection");

    // …and the row above the prompt says so, because a captured drag leaves the terminal nothing to
    // show. Right-aligned, on its own chip, out of the activity bar's way.
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    let toast = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Copied to clipboard"))
        .unwrap_or_else(|| panic!("the copy is confirmed on screen: {content}"));
    let text = row_text(&buffer, toast, width);
    assert!(text.trim_end().ends_with("Copied to clipboard"), "right-aligned: {text:?}");
    let chip = app.theme_styles.selected_bg.bg.expect("a chip background");
    // Flush to the composer's own right margin, and wearing a bold accent on a background so it
    // catches an eye on its way to the prompt.
    assert_eq!(buffer[(width - 2, toast)].bg, chip, "the chip runs to the inset right edge");
    // Columns, not bytes: the mode button's icon shares the row
    let start = column_of(&text, "\u{f0c5}");
    assert_eq!(buffer[(start, toast)].bg, chip);
    assert_ne!(buffer[(start - 2, toast)].bg, chip, "and stops there");
    let label = column_of(&text, "Copied");
    assert!(buffer[(label, toast)].modifier.contains(ratatui::style::Modifier::BOLD));
    assert_eq!(buffer[(label, toast)].fg, app.theme_styles.accent.fg.expect("the accent"));

    // It is transient: a few seconds later the row is its old self again.
    app.status.toast = app.status.toast.take().map(|(message, _)| {
        (message, std::time::Instant::now() - std::time::Duration::from_secs(30))
    });
    let buffer = render_app(&mut app, &backend);
    assert!(!buffer_to_string(&buffer).contains("Copied to clipboard"), "the toast expires");

    // Ctrl+Shift+C is the other conventional chord and reaches the same action.
    app.outbox.copy = None;
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"));
}
