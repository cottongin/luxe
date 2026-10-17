//! Changes pane — the peeked diff, its columns, footer and stepping.

use super::*;

/// The pane takes the conversation column and gives it back — the dock and the composer never
/// move, which is what makes the dock usable as the pane's navigation.
#[test]
fn the_changes_pane_replaces_the_transcript_and_esc_returns_it() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("changes-pane", 3));
    app.push_convo(ConvoBlock::Assistant("a distinctive transcript line".into()));

    let transcript = buffer_to_string(&render_app(&mut app, &backend));
    assert!(transcript.contains("a distinctive transcript line"));

    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let pane = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!pane.contains("a distinctive transcript line"), "the pane owns the column: {pane}");
    assert!(pane.contains("file00.rs"), "and names the file: {pane}");
    assert!(pane.contains("FILES"), "the dock stays: {pane}");
    assert!(pane.contains("esc close"), "and says how to leave: {pane}");

    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    let back = buffer_to_string(&render_app(&mut app, &backend));
    assert!(back.contains("a distinctive transcript line"), "Esc gives the column back: {back}");
}

/// The pane draws the file's actual hunks, through the review's own content model.
#[test]
fn the_changes_pane_draws_the_hunks_it_was_given() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("changes-hunks", 2));

    app.update(Msg::OpenChanges("src/module00/file00.rs".into()));
    let loading = buffer_to_string(&render_app(&mut app, &backend));
    assert!(loading.contains("Reading the change"), "it says it is working: {loading}");

    let path = "src/module00/file00.rs";
    // Re-open through the helper so the request the pane is waiting on is the one answered.
    app.update(Msg::CloseChanges);
    while rx.try_recv().is_ok() {}
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(shown.contains("@@ -1,2 +1,2 @@"), "the hunk header orients the reader: {shown}");
    assert!(shown.contains("let total = a + b;"), "the old line: {shown}");
    assert!(shown.contains("let total = a - b;"), "and the new one: {shown}");
    assert!(shown.contains("1 of 2"), "and where this file sits in the change: {shown}");
}

/// A file that matches its base is an answer, not a failure — and never an empty surface.
#[test]
fn an_unchanged_file_says_so_rather_than_showing_nothing() {
    let (width, height) = (120, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("changes-clean", 2));
    open_changes_with(&mut app, &mut rx, "src/module00/file00.rs", None);

    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(shown.contains("Nothing has changed in this file"), "{shown}");
}

/// A failure is shown as a sentence a person can act on, in the pane they are looking at.
#[test]
fn a_failed_capture_is_reported_in_the_pane() {
    let (width, height) = (120, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("changes-failure", 2));

    let path = "src/module00/file00.rs";
    app.update(Msg::OpenChanges(path.to_string()));
    let luxe_core::engine::Action::LoadFileDiff { request_id, .. } =
        rx.try_recv().expect("a capture was requested")
    else {
        panic!("expected a file-diff request");
    };
    app.update(Msg::Engine(luxe_core::event::Event::FileDiffFailed {
        request_id,
        path: path.to_string(),
        error: luxe_core::file_diff::FileDiffFailure::TooLarge,
    }));

    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(shown.contains("This change is too large"), "capitalized like prose: {shown}");
    assert!(shown.contains("open the full review"), "and says where to go instead: {shown}");
}

/// Escalating leaves the pane and asks for a review — the expensive surface, deliberately.
///
/// Asking is all it does: the review opens once there is a whole one to open, and until then the
/// tray above the composer carries the wait.
#[test]
fn escalating_from_the_pane_opens_the_review() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("changes-escalate", 2));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let _ = render_app(&mut app, &backend);

    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewOpen));
    let asked = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|action| matches!(action, luxe_core::engine::Action::LoadGuidedReview { .. }));
    assert!(asked, "a review capture was requested");
    assert_eq!(app.current_view, ViewMode::Chat, "and the screen is not taken until it is ready");
    let waiting = buffer_to_string(&render_app(&mut app, &backend));
    assert!(waiting.contains("review"), "with the wait visible where the reader is: {waiting}");
}

/// The bar's changes module earns its cells by carrying the way in, not just the number.
#[test]
fn the_statusline_changes_module_says_the_count_and_the_key() {
    let (width, height) = (150, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("statusline-changes", 4));
    app.dock.hide();
    app.statusline_state.show();

    let buffer = render_app(&mut app, &backend);
    let bar = row_text(&buffer, height - 1, width);
    assert!(bar.contains("4 files"), "the count: {bar}");
    assert!(bar.contains("ctrl+p"), "and how to see them: {bar}");

    // Clicking it is the same affordance for the pointer already sitting on it.
    let column = column_of(&bar, "4 files");
    match left_click(&mut app, column, height - 1) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::DispatchUiAction(luxe_core::keymap::UiAction::ToggleChanges)),
            "clicking the module opens the pane: {message:?}"
        ),
        other => panic!("expected the module to be clickable, got {other:?}"),
    }
}

/// A clean worktree spends no cells saying so.
#[test]
fn the_statusline_changes_module_is_absent_with_nothing_changed() {
    let (width, height) = (150, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("statusline-clean"));
    app.dock.hide();
    app.statusline_state.show();

    let bar = row_text(&render_app(&mut app, &backend), height - 1, width);
    assert!(!bar.contains("files"), "no change, no module: {bar}");
    assert!(!bar.contains("ctrl+p"), "and no key to advertise: {bar}");
}

/// A peeked diff keeps the conversation beside it where the column can seat both — and the whole
/// chat stack, composer included, narrows with it.
///
/// A prompt spanning both columns says the text goes to both, which is not what happens: the
/// message lands in the transcript. And a diff stopping short of the composer's rows would waste
/// them for nothing.
#[test]
fn a_peeked_diff_keeps_the_conversation_and_the_composer_beside_it() {
    let path = "src/module00/file00.rs";
    let (width, height) = (240u16, 30);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("peek-three-pane", 2));
    app.push_convo(ConvoBlock::Assistant("A reply that stays readable beside the code.".into()));
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let buffer = render_app(&mut app, &backend);
    let diff_width = luxe_tui::view::peek_diff_column(&app, width).expect("the column seats both");
    let whole = buffer_to_string(&buffer);
    assert!(whole.contains("fn sum() {"), "the diff is on screen: {whole}");
    assert!(whole.contains("stays readable beside the code"), "and so is the exchange: {whole}");

    // The composer belongs to the conversation column: its box starts to the right of the diff.
    let composer_row = row_containing(&buffer, "\u{256d}", width, height);
    let left_of_rule = row_text(&buffer, composer_row, diff_width);
    assert!(
        !left_of_rule.contains('\u{256d}'),
        "the prompt does not reach under the diff: {left_of_rule:?}"
    );

    // And the diff runs the full height beside it rather than stopping where the composer starts.
    let footer_row = row_containing(&buffer, "esc close", width, height);
    assert!(footer_row > composer_row, "the diff outlasts the prompt: {whole}");
}

/// On a wide terminal the diff takes what code needs and the conversation keeps the rest.
///
/// It used to be the other way round: the conversation was capped at eighty cells and a unified
/// diff took everything else, so a wide screen spent most of its rows on empty diff.
#[test]
fn a_wide_peek_gives_the_conversation_what_the_diff_does_not_need() {
    use luxe_tui::view::peek_diff_width;
    let unified = peek_diff_width(300, false).expect("seats both");
    let split = peek_diff_width(300, true).expect("seats both");
    assert!(unified < 140, "a unified diff stops at a code measure: {unified}");
    assert!(split > unified && split < 230, "two columns get two measures: {split}");
    assert_eq!(peek_diff_width(300, false), peek_diff_width(400, false), "and stops growing");
    // Narrower than the caps, nothing changes: the transcript keeps its floor and the diff the rest
    assert_eq!(peek_diff_width(140, false), Some(140 - 57));
    assert_eq!(peek_diff_width(140, true), Some(140 - 57));
    // Whether the two share never depends on the layout, so a layout toggle never moves the chat
    for width in [128u16, 129, 200] {
        assert_eq!(peek_diff_width(width, false).is_some(), peek_diff_width(width, true).is_some());
    }

    // On screen: a unified peek on a wide terminal, the transcript wide beside it
    let path = "src/module00/file00.rs";
    let (width, height) = (300u16, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("peek-wide", 2));
    app.push_convo(ConvoBlock::Assistant("word ".repeat(60)));
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    render_app(&mut app, &backend);
    assert_eq!(luxe_tui::view::peek_diff_column(&app, width), Some(split), "starts split");
    // The toggle flips what the last frame drew
    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewToggleSplit));
    let buffer = render_app(&mut app, &backend);
    let diff = luxe_tui::view::peek_diff_column(&app, width).expect("shared");
    assert_eq!(diff, unified, "drawn unified, sized unified");
    let prose = row_containing(&buffer, "word word", width, height);
    let words = row_text(&buffer, prose, width).matches("word").count();
    assert!(words > 30, "the transcript has room for a line of prose: {words} words");
}

/// Below that width the diff takes the column alone, because a transcript in forty cells is a
/// column of two-word lines.
#[test]
fn a_narrow_peek_gives_the_diff_the_whole_column() {
    let path = "src/module00/file00.rs";
    let (width, height) = (110u16, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("peek-two-pane", 2));
    app.push_convo(ConvoBlock::Assistant("A reply that has to wait its turn.".into()));
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    assert!(luxe_tui::view::peek_diff_width(width, false).is_none(), "too narrow for both");
    let whole = buffer_to_string(&render_app(&mut app, &backend));
    assert!(whole.contains("fn sum() {"), "the diff is on screen: {whole}");
    assert!(!whole.contains("has to wait its turn"), "and the transcript waits: {whole}");
}

/// A pane with room for two readable columns draws two; one without draws the unified list. Nobody
/// has to ask for it, which is why the width test has to be conservative rather than eager.
#[test]
fn the_pane_draws_two_columns_only_where_they_would_read() {
    let path = "src/module00/file00.rs";
    // 260 rather than 200: with the conversation kept beside it, a diff needs the wider terminal
    // before its own two columns fit. At 90 the pane has the main column to itself and still
    // cannot seat two readable ones.
    for (width, expect_split) in [(260u16, true), (90u16, false)] {
        let backend = test_backend(width, 24);
        let (mut app, mut rx) = create_app_with_engine();
        app.update(Msg::TerminalResized { width, height: 24 });
        app.dock.hide();
        app.workspace = Some(changed_workspace("changes-split", 2));
        open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

        let buffer = render_app(&mut app, &backend);
        assert_eq!(
            split_row(&buffer, "fn sum() {", &app, width, 24),
            expect_split,
            "at {width} columns, split should be {expect_split}: {}",
            buffer_to_string(&buffer)
        );
        // Either way the change itself is legible.
        let shown = buffer_to_string(&buffer);
        assert!(shown.contains("let total = a - b;"), "the new line is shown: {shown}");
    }
}

/// One key, both diff surfaces: the action names a layout rather than a surface.
#[test]
fn the_layout_toggle_switches_the_pane_between_columns_and_the_list() {
    // Wide enough for the diff to keep two columns beside the conversation the peek now keeps.
    let (width, height) = (260, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("changes-toggle-split", 2));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let buffer = render_app(&mut app, &backend);
    assert!(
        split_row(&buffer, "fn sum() {", &app, width, height),
        "wide enough, so it starts split"
    );
    let split = buffer_to_string(&buffer);
    assert!(split.contains("ctrl+b unified diff"), "and offers the other layout: {split}");

    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewToggleSplit));
    let buffer = render_app(&mut app, &backend);
    assert!(
        !split_row(&buffer, "fn sum() {", &app, width, height),
        "one press switches to the list"
    );
    let unified = buffer_to_string(&buffer);
    assert!(unified.contains("ctrl+b side-by-side diff"), "and offers the way back: {unified}");
}

/// Stepping between files does not make the layout shortcut blink.
///
/// `ChangesView::show` drops the loaded diff and the next capture takes a frame, so anything that
/// asks "can this draw two columns?" through `view.file()` gets `None` in between. Answering
/// `false` there dropped the shortcut out of the footer and put it back a frame later — every hint
/// to its right shifted twice per file switch. The last resolved layout carries across the gap
/// instead, so the row holds still.
#[test]
fn stepping_between_files_holds_the_footer_still() {
    let (width, height) = (200, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("changes-footer-still", 2));
    let first = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, first, Some(one_hunk_file(first)));

    let footer_of = |app: &mut App| -> String {
        let buffer = render_app(app, &backend);
        let row = row_containing(&buffer, "esc close", width, height);
        peek_diff_text(&buffer, row, app, width).trim().to_string()
    };
    let settled = footer_of(&mut app);
    assert!(settled.contains("diff"), "precondition: the layout shortcut is on the row: {settled}");

    // Mid-switch: the next diff has been asked for but has not arrived.
    app.update(Msg::ChangesStepFile(true));
    let loading = footer_of(&mut app);
    assert_eq!(loading, settled, "the footer must not reflow while the next diff loads");
}

/// The left column is the old file and the right is the new one, so each numbers its own side.
/// Printing the old number beside new content is a false statement about where that line lives.
#[test]
fn each_split_column_numbers_its_own_side() {
    // Wide enough for the diff to keep two columns beside the conversation the peek now keeps.
    let (width, height) = (260, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("changes-split-numbers", 2));
    let path = "src/module00/file00.rs";
    // Context at old 9 / new 4, so the two sides cannot be confused for one another.
    let mut file = one_hunk_file(path);
    file.hunks[0].lines[0].old_line = Some(9);
    file.hunks[0].lines[0].new_line = Some(4);
    open_changes_with(&mut app, &mut rx, path, Some(file));

    let buffer = render_app(&mut app, &backend);
    let row = row_containing(&buffer, "fn sum() {", width, height);
    let text = peek_diff_text(&buffer, row, &app, width);
    let (left, right) = text.split_once(" │ ").expect("a split row has two columns");
    assert!(left.contains('9'), "the old column carries the old number: {left:?}");
    assert!(right.contains('4'), "the new column carries the new one: {right:?}");
    assert!(!right.contains('9'), "and not the old one: {right:?}");
}

/// A rule sits over the pane's shortcuts, the way one sits under its header: straight after the
/// last line of code, the keys read as one more line of it.
#[test]
fn the_pane_rules_off_its_footer() {
    let path = "src/module00/file00.rs";
    let (width, height) = (120u16, 20u16);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("changes-rule", 3));
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let buffer = render_app(&mut app, &backend);
    let row = row_containing(&buffer, "esc close", width, height);
    let above = peek_diff_text(&buffer, row - 1, &app, width);
    assert!(above.trim().chars().all(|c| c == '─') && above.trim().len() > 20, "{above:?}");
}

/// A footer that truncates mid-label is worse than one that says less: `Alt+W no wr` teaches the
/// reader that the row cannot be trusted.
#[test]
fn the_pane_footer_drops_whole_shortcuts_rather_than_truncating_one() {
    let path = "src/module00/file00.rs";
    for width in [50u16, 60, 70, 90, 120, 200] {
        let backend = test_backend(width, 20);
        let (mut app, mut rx) = create_app_with_engine();
        app.update(Msg::TerminalResized { width, height: 20 });
        app.dock.hide();
        app.workspace = Some(changed_workspace("changes-footer", 3));
        open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

        let buffer = render_app(&mut app, &backend);
        let row = row_containing(&buffer, "esc close", width, 20);
        let footer = peek_diff_text(&buffer, row, &app, width);
        assert!(
            footer.trim_end().ends_with("esc close"),
            "the way out is never the thing that gets cut, at {width}: {footer:?}"
        );
        // Every segment on the row is a whole shortcut, so nothing was cut mid-label.
        const WHOLE: [&str; 9] = [
            "alt+m whole file",
            "alt+m changes",
            "alt+n next file",
            "alt+p previous",
            "ctrl+b side-by-side diff",
            "ctrl+b unified diff",
            "alt+o open in editor",
            "alt+w stop wrapping",
            "ctrl+g review this file",
        ];
        for segment in footer.trim().split(" · ") {
            assert!(
                segment == "esc close" || WHOLE.contains(&segment),
                "{segment:?} is not a whole shortcut at {width}: {footer:?}"
            );
        }
    }
}

/// A peek at a file outside the project says where it is, and stops short of a review.
///
/// The pane is the same pane, and the footer is where the difference has to show: a review is
/// captured from the project's own worktree, so `Ctrl+G review this file` over a file that lives
/// somewhere else names a destination that cannot exist.
#[test]
fn peeking_a_file_outside_the_project_offers_everything_but_the_review() {
    let (width, height) = (140, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(external_workspace());

    let path = "/work/sdk/src/client.ts";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("/work/sdk/src/client.ts"), "the header says where it is: {content}");
    assert!(content.contains("esc close"), "the footer is drawn: {content}");
    assert!(
        !content.contains("review this file"),
        "and offers no review of a file no review can contain: {content}"
    );
}

/// Stepping walks every file this session changed, the project's own first and the rest after —
/// the list is what changed, and work done elsewhere changed too.
#[test]
fn stepping_runs_off_the_projects_changes_into_the_work_done_elsewhere() {
    let (width, height) = (140, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    let mut workspace = changed_workspace("changes-stepping", 1);
    workspace.external = external_workspace().external;
    app.workspace = Some(workspace);

    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    app.update(Msg::ChangesStepFile(true));
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("client.ts"), "the next file is the one elsewhere: {content}");
    assert!(content.contains("2 of 2"), "and it is counted in the same list: {content}");
}

/// The pane cannot scroll sideways, so it wraps from the start: a clipped line here would put
/// content out of reach rather than one keystroke away.
#[test]
fn the_pane_wraps_long_lines_without_being_asked() {
    let (width, height) = (120, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("changes-wrap", 2));
    let path = "src/module00/file00.rs";
    let mut file = one_hunk_file(path);
    // One-sided, so the pane stays a unified list at any width: wrapping a *column* is the split
    // renderer's own business, and the continuation and clipped forms below only exist here.
    file.status = luxe_core::guided_review::ReviewFileStatus::Added;
    let tail = "the-tail-that-must-not-be-lost";
    file.hunks[0].lines[0].text = format!("let head = \"{}{tail}\";", "padding-".repeat(20));
    open_changes_with(&mut app, &mut rx, path, Some(file));

    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(shown.contains(tail), "the end of a long line is still reachable: {shown}");
    assert!(shown.contains('↳'), "and it says where it continued: {shown}");
    assert!(shown.contains("alt+w stop wrapping"), "the footer offers to stop wrapping: {shown}");

    // Turning it off clips instead, and says so.
    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ChangesToggleWordWrap));
    let clipped = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!clipped.contains(tail), "clipped now: {clipped}");
    assert!(clipped.contains('…'), "and marked as clipped: {clipped}");
}
