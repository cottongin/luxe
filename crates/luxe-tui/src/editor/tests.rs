use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Whitespace is nothing to send, and stays where it was typed.
#[test]
fn a_blank_draft_submits_nothing() {
    let mut editor = Editor::new();
    editor.insert_text("  \n\t ");
    assert!(editor.submit().is_none());
    assert_eq!(editor.current_input(), "  \n\t ");
}

#[test]
fn submit_expands_large_paste_and_clears_the_draft() {
    let mut editor = Editor::new();
    editor.insert_text("before ");
    editor.paste_text("alpha\nbeta\ngamma");
    editor.insert_text(" after");

    assert_eq!(editor.current_input(), "before [Pasted 3 lines] after");
    let (submitted, images) = editor.submit().unwrap();
    assert_eq!(submitted, "before alpha\nbeta\ngamma after");
    assert!(images.is_empty());
    assert!(editor.current_input().is_empty());
}

/// A one-line field is the shape an API key arrives in, and the newline a console copy brings
/// with it must not survive to be typed, echoed, or sent.
#[test]
fn a_one_line_field_takes_a_paste_without_its_line_breaks() {
    assert_eq!(single_line_paste("sk-ant-abc\n"), "sk-ant-abc");
    assert_eq!(single_line_paste("  sk-ant-abc  "), "sk-ant-abc");
    assert_eq!(single_line_paste("sk-ant\r\n-abc"), "sk-ant-abc", "a wrapped key is one key");
    assert_eq!(single_line_paste("a\tb"), "ab", "a tab would move focus, not type");
    assert_eq!(single_line_paste("\n\n"), "", "and a paste of nothing types nothing");
}

#[test]
fn short_paste_stays_editable_inline() {
    let mut editor = Editor::new();
    editor.paste_text("alpha\nbeta");
    assert_eq!(editor.current_input(), "alpha\nbeta");
    assert_eq!(editor.expanded_input(), "alpha\nbeta");
}

#[test]
fn tracked_ranges_expand_only_their_own_duplicate_marker() {
    let mut editor = Editor::new();
    editor.insert_text("literal [Pasted 3 lines] ");
    editor.paste_text("one\ntwo\nthree");
    assert_eq!(editor.expanded_input(), "literal [Pasted 3 lines] one\ntwo\nthree");
}

#[test]
fn pasted_ranges_stay_exact_after_wide_characters() {
    let mut editor = Editor::new();
    editor.insert_text("你好 ");
    editor.paste_text("one\ntwo\nthree");
    editor.insert_text(" 结束");

    assert_eq!(editor.expanded_input(), "你好 one\ntwo\nthree 结束");
}

#[test]
fn paste_tokens_are_atomic_for_movement_and_deletion() {
    let mut editor = Editor::new();
    editor.paste_text("one\ntwo\nthree");
    let end = editor.cursor;

    editor.handle_key(key(KeyCode::Left));
    assert_eq!(editor.cursor, 0);
    editor.handle_key(key(KeyCode::Right));
    assert_eq!(editor.cursor, end);
    editor.handle_key(key(KeyCode::Backspace));
    assert!(editor.current_input().is_empty());
    assert!(editor.expanded_input().is_empty());
}

/// A reference is one token like a paste: stepped over, deleted whole, drawn as a chip, and
/// sent exactly as written — never expanded into anything else.
#[test]
fn a_reference_is_one_token_sent_as_written() {
    let mut editor = Editor::new();
    editor.insert_text("Review ");
    editor.insert_reference("@[file:docs/plan.md]");
    editor.insert_text(" today");
    let token_end = "Review @[file:docs/plan.md]".len();
    editor.cursor = token_end;
    editor.handle_key(key(KeyCode::Left));
    assert_eq!(editor.cursor, "Review ".len(), "stepped over");
    let layout = editor.layout(80);
    let chips: Vec<&str> = layout.rows()[0]
        .iter()
        .filter(|segment| segment.is_chip)
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(chips, ["@[file:docs/plan.md]"]);
    assert_eq!(editor.expanded_input(), "Review @[file:docs/plan.md] today");
    editor.select_all();
    assert_eq!(editor.selected_text().as_deref(), Some("Review @[file:docs/plan.md] today"));
    editor.clear_selection();
    editor.cursor = token_end;
    editor.handle_key(key(KeyCode::Backspace));
    assert_eq!(editor.current_input(), "Review  today", "deleted whole");
    assert_eq!(
        editor.submit().map(|(text, images)| (text, images.len())),
        Some(("Review  today".into(), 0))
    );
}

#[test]
fn arrows_navigate_explicit_lines_before_history() {
    let mut editor = Editor::new();
    editor.insert_text("alpha\nbeta\ngamma");

    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.cursor, "alpha\nbeta".len());
    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.cursor, "alpha".len());
    editor.handle_key(key(KeyCode::Down));
    assert_eq!(editor.cursor, "alpha\nbeta".len());
}

#[test]
fn arrows_navigate_soft_wrapped_lines() {
    let mut editor = Editor::new();
    editor.set_viewport_width(5);
    editor.insert_text("abcdefgh");

    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.visual_cursor(5).row, 0);
    editor.handle_key(key(KeyCode::Down));
    assert_eq!(editor.visual_cursor(5).row, 1);
}

#[test]
fn history_is_only_reached_from_prompt_boundaries() {
    let mut editor = Editor::new();
    editor.insert_text("previous");
    editor.submit();
    editor.insert_text("draft");

    editor.cursor = 3;
    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.cursor, 0);
    assert_eq!(editor.current_input(), "draft");
    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.current_input(), "previous");
    editor.handle_key(key(KeyCode::Down));
    assert_eq!(editor.current_input(), "previous");
    editor.handle_key(key(KeyCode::Down));
    assert_eq!(editor.current_input(), "");
}

#[test]
fn unicode_edits_keep_cursor_on_byte_boundaries() {
    let mut editor = Editor::new();
    editor.insert_text("café");
    editor.handle_key(key(KeyCode::Left));
    assert_eq!(editor.cursor, 3);
    editor.handle_key(key(KeyCode::Delete));
    assert_eq!(editor.buffer, "caf");
    editor.insert_text("🪨");
    editor.handle_key(key(KeyCode::Backspace));
    assert_eq!(editor.buffer, "caf");
    assert_eq!(editor.cursor, 3);
}

#[test]
fn visual_layout_counts_wide_characters_and_wraps() {
    let mut editor = Editor::new();
    editor.insert_text("ab界cd");
    assert_eq!(editor.visual_lines(4), vec!["ab界", "cd"]);
    assert_eq!(editor.visual_cursor(4), VisualCursor { row: 1, column: 2 });
}

#[test]
fn paste_segments_preserve_unicode_wrap_boundaries() {
    let mut editor = Editor::new();
    editor.insert_text("界 ");
    editor.paste_text("one\ntwo\nthree");
    editor.insert_text(" tail");

    assert_eq!(
        editor.visual_segments(10),
        vec![
            vec![
                VisualSegment {
                    text: "界 ".to_string(),
                    is_chip: false,
                    is_selected: false,
                    mark: None
                },
                // The space after the marker's "[Pasted" hangs past the wrap column.
                VisualSegment {
                    text: "[Pasted ".to_string(),
                    is_chip: true,
                    is_selected: false,
                    mark: None
                },
            ],
            vec![
                VisualSegment {
                    text: "3 lines]".to_string(),
                    is_chip: true,
                    is_selected: false,
                    mark: None
                },
                VisualSegment {
                    text: " ".to_string(),
                    is_chip: false,
                    is_selected: false,
                    mark: None
                },
            ],
            vec![VisualSegment {
                text: "tail".to_string(),
                is_chip: false,
                is_selected: false,
                mark: None
            }],
        ]
    );
}

#[test]
fn selection_covers_a_drag_and_is_replaced_by_typing() {
    let mut editor = Editor::new();
    editor.set_viewport_width(40);
    editor.insert_text("select this text");

    // A drag from column 7 to 11 selects the middle word.
    editor.begin_selection_at_visual(0, 7);
    editor.extend_selection_to_visual(0, 11);
    assert_eq!(editor.selected_text().as_deref(), Some("this"));
    // The covered cells are marked for rendering.
    let segments = editor.visual_segments(40).remove(0);
    assert!(segments.iter().any(|segment| segment.is_selected && segment.text == "this"));

    // Typing replaces it; Shift+arrows extend from the keyboard.
    editor.handle_key(key(KeyCode::Char('X')));
    assert_eq!(editor.current_input(), "select X text");
    assert!(editor.selection().is_none(), "the selection is consumed");

    editor.cursor = 0;
    for _ in 0..6 {
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    }
    assert_eq!(editor.selected_text().as_deref(), Some("select"));
    // Any un-shifted movement drops it.
    editor.handle_key(key(KeyCode::Right));
    assert!(editor.selection().is_none());

    editor.select_all();
    assert_eq!(editor.selected_text().as_deref(), Some("select X text"));
}

#[test]
fn a_selected_paste_token_copies_its_expanded_text() {
    let mut editor = Editor::new();
    editor.paste_text("alpha\nbeta\ngamma");
    editor.select_all();
    assert_eq!(
        editor.selected_text().as_deref(),
        Some("alpha\nbeta\ngamma"),
        "the clipboard gets the pasted text, not the [Pasted N lines] marker"
    );
}

#[test]
fn wrapping_breaks_at_word_boundaries() {
    let mut editor = Editor::new();
    editor.insert_text("the quick brown fox jumps");

    // Words move to the next row whole instead of being cut at the column budget.
    assert_eq!(editor.visual_lines(10), vec!["the quick ", "brown fox ", "jumps"]);
    assert_eq!(editor.visual_lines(16), vec!["the quick brown ", "fox jumps"]);
}

#[test]
fn a_word_longer_than_the_row_still_hard_splits() {
    let mut editor = Editor::new();
    editor.insert_text("hi supercalifragilistic");

    // Nothing to wrap to, so the overlong word is broken at the column budget.
    assert_eq!(editor.visual_lines(8), vec!["hi ", "supercal", "ifragili", "stic"]);
}

#[test]
fn wrapped_word_keeps_the_cursor_with_its_character() {
    let mut editor = Editor::new();
    editor.set_viewport_width(10);
    editor.insert_text("alpha bravo");

    // "bravo" wraps whole, so the caret after it sits on the second row, not the first.
    assert_eq!(editor.visual_cursor(10), VisualCursor { row: 1, column: 5 });
    // The caret between the two words stays at the end of the first row's text.
    editor.cursor = "alpha".len();
    assert_eq!(editor.visual_cursor(10), VisualCursor { row: 0, column: 5 });
    // Moving down from inside the first word lands inside the wrapped word.
    editor.cursor = 2;
    editor.handle_key(key(KeyCode::Down));
    assert_eq!(editor.visual_cursor(10).row, 1);
}

#[test]
fn a_full_row_opens_the_next_one_before_the_word_moves() {
    let mut editor = Editor::new();
    editor.insert_text("aaaa bbb");
    // The row is full, so the caret already belongs on a second row and the box grows now —
    // not one keystroke later, when the word finally moves down.
    assert_eq!(editor.visual_line_count(8), 2);
    assert_eq!(editor.visual_cursor(8), VisualCursor { row: 1, column: 0 });

    // The word moving down reflows the text without changing the height again.
    editor.insert_text("b");
    assert_eq!(editor.visual_lines(8), vec!["aaaa ", "bbbb"]);
    assert_eq!(editor.visual_line_count(8), 2);
}

/// The composer sizes itself from the layout's row count and scrolls to keep the caret in
/// view, so a caret row outside that count is exactly the state where the first line
/// disappears mid-word. Typing a paragraph one character at a time must never reach it.
#[test]
fn the_caret_row_always_fits_inside_the_reported_height() {
    for width in [8u16, 13, 20, 37] {
        let mut editor = Editor::new();
        editor.set_viewport_width(width);
        let mut previous_rows = 1usize;
        for character in
            "the quick brown fox jumps over the lazy dog while carrying supercalifragilistic \
                 expialidocious luggage"
                .chars()
        {
            editor.insert_text(&character.to_string());
            let rows = editor.visual_line_count(width);
            let caret = editor.visual_cursor(width);
            assert!(
                caret.row < rows,
                "width {width}: caret on row {} with only {rows} rows after {character:?}",
                caret.row
            );
            assert!(
                caret.column < usize::from(width),
                "width {width}: caret at column {} past the wrap column after {character:?}",
                caret.column
            );
            // Height only ever climbs by one row at a time while typing forward, so the box
            // never jumps two lines at once.
            assert!(
                rows <= previous_rows + 1,
                "width {width}: height jumped {previous_rows} -> {rows}"
            );
            previous_rows = rows;
        }
    }
}

#[cfg(feature = "clipboard-images")]
#[test]
fn image_attachment_uses_a_compact_token_and_valid_png() {
    use base64::Engine;

    let mut editor = Editor::new();
    assert!(editor.attach_image(vec![255, 0, 0, 255], 1, 1));
    assert_eq!(editor.current_input(), "[Pasted Image]");
    assert_eq!(
        editor.visual_segments(80),
        vec![vec![VisualSegment {
            text: "[Pasted Image]".to_string(),
            is_chip: true,
            is_selected: false,
            mark: None
        }]]
    );
    assert!(editor.has_images());

    let (text, images) = editor.submit().unwrap();
    assert!(text.is_empty());
    assert_eq!(images[0].filename, "clipboard.png");
    let bytes = base64::engine::general_purpose::STANDARD.decode(&images[0].data).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

/// Enter is the caller's: the editor leaves the draft alone rather than clearing it.
///
/// The break this catches: a caller that forgot to intercept Enter lost the draft to history
/// without anything being sent.
#[test]
fn enter_is_left_to_the_caller() {
    let mut editor = Editor::new();
    editor.insert_text("half a thought");
    editor.handle_key(key(KeyCode::Enter));
    assert_eq!(editor.current_input(), "half a thought");
    assert!(editor.history.is_empty(), "and nothing went to history");
}

/// Up walks back one message at a time and stops at the oldest; Down walks forward to the draft,
/// taking the caret to the end of a recalled message before leaving it.
#[test]
fn history_walks_both_ways_through_several_messages() {
    let mut editor = Editor::new();
    for message in ["one", "two", "three"] {
        editor.insert_text(message);
        editor.submit();
    }
    for _ in 0..4 {
        editor.handle_key(key(KeyCode::Up));
    }
    assert_eq!(editor.current_input(), "one", "the oldest is as far back as it goes");
    let mut seen = Vec::new();
    for _ in 0..4 {
        editor.handle_key(key(KeyCode::Down));
        seen.push(editor.current_input().to_string());
    }
    assert_eq!(seen, ["one", "two", "three", ""]);
}

/// History keeps the newest 500 messages, so a long session's pastes do not pile up forever.
#[test]
fn history_keeps_the_newest_five_hundred() {
    let mut editor = Editor::new();
    for message in 0..=500 {
        editor.insert_text(&format!("message {message}"));
        editor.submit();
    }
    assert_eq!(editor.history.len(), 500);
    assert_eq!(editor.history.first().map(String::as_str), Some("message 1"));
    editor.handle_key(key(KeyCode::Up));
    assert_eq!(editor.current_input(), "message 500", "the newest is still one step back");
}

mod text_navigation {
    use super::*;

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn editor(text: &str, cursor: usize) -> Editor {
        Editor::with_text(text, cursor)
    }

    /// Every stop a word jump makes, both ways, across punctuation, runs of spaces and a line
    /// break: the start of each word going left, the end of each going right.
    #[test]
    fn word_jumps_stop_at_each_word_both_ways() {
        let text = "fix  the loop.unwrap()\n  next_line héllo";
        let mut left = editor(text, text.len());
        let mut stops = Vec::new();
        while left.cursor > 0 {
            left.handle_key(with(KeyCode::Left, KeyModifiers::CONTROL));
            stops.push(&text[left.cursor..]);
        }
        assert_eq!(
            stops,
            [
                "héllo",
                "next_line héllo",
                "()\n  next_line héllo",
                "unwrap()\n  next_line héllo",
                ".unwrap()\n  next_line héllo",
                "loop.unwrap()\n  next_line héllo",
                "the loop.unwrap()\n  next_line héllo",
                text,
            ]
        );

        let mut right = editor(text, 0);
        let mut ends = Vec::new();
        while right.cursor < text.len() {
            right.handle_key(with(KeyCode::Right, KeyModifiers::CONTROL));
            ends.push(&text[..right.cursor]);
        }
        assert_eq!(ends.len(), 8, "{ends:?}");
        assert_eq!(ends[0], "fix");
        assert_eq!(ends[3], "fix  the loop.");
        assert_eq!(ends[6], "fix  the loop.unwrap()\n  next_line");
        assert_eq!(ends[7], text);
    }

    /// The same jump arrives as Ctrl+arrow, Alt+arrow, or — from ghostty and Terminal.app on
    /// macOS — readline's Alt+B and Alt+F; Cmd+arrows and Ctrl+A/E go to the line's ends.
    #[test]
    fn every_encoding_of_a_jump_lands_the_same() {
        let text = "one two\nthree four";
        let middle = "one two\nthree fo".len();
        for (key, expected) in [
            (with(KeyCode::Left, KeyModifiers::CONTROL), "one two\nthree ".len()),
            (with(KeyCode::Left, KeyModifiers::ALT), "one two\nthree ".len()),
            (with(KeyCode::Char('b'), KeyModifiers::ALT), "one two\nthree ".len()),
            (with(KeyCode::Right, KeyModifiers::CONTROL), text.len()),
            (with(KeyCode::Right, KeyModifiers::ALT), text.len()),
            (with(KeyCode::Char('f'), KeyModifiers::ALT), text.len()),
            (with(KeyCode::Left, KeyModifiers::SUPER), "one two\n".len()),
            (with(KeyCode::Char('a'), KeyModifiers::CONTROL), "one two\n".len()),
            (with(KeyCode::Right, KeyModifiers::SUPER), text.len()),
            (with(KeyCode::Char('e'), KeyModifiers::CONTROL), text.len()),
        ] {
            let mut field = editor(text, middle);
            field.handle_key(key);
            assert_eq!(field.cursor, expected, "{key:?}");
            assert_eq!(field.current_input(), text, "{key:?} moves, never edits");
        }
        // Home and End, unmodified, do the same as Ctrl+A and Ctrl+E
        let mut field = editor(text, middle);
        field.handle_key(with(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(field.cursor, "one two\n".len());
        field.handle_key(with(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(field.cursor, text.len());
        // Ctrl+E on the first line stops at its end, not the draft's
        let mut field = editor(text, 1);
        field.handle_key(with(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(field.cursor, "one two".len());
    }

    /// Word and line deletions in every encoding, including at the edges: at a line's start the
    /// line deletion joins it to the one above, and the forward deletion reaches the word's end.
    #[test]
    fn deletions_take_a_word_or_the_line_before_the_cursor() {
        let text = "one two\nthree four";
        let end = text.len();
        for (key, cursor, left) in [
            (with(KeyCode::Backspace, KeyModifiers::ALT), end, "one two\nthree "),
            (with(KeyCode::Backspace, KeyModifiers::CONTROL), end, "one two\nthree "),
            (with(KeyCode::Char('u'), KeyModifiers::CONTROL), end, "one two\n"),
            (with(KeyCode::Backspace, KeyModifiers::SUPER), end, "one two\n"),
            (
                with(KeyCode::Char('u'), KeyModifiers::CONTROL),
                "one two\n".len(),
                "one twothree four",
            ),
            (with(KeyCode::Delete, KeyModifiers::CONTROL), 3, "one\nthree four"),
            (with(KeyCode::Delete, KeyModifiers::ALT), 0, " two\nthree four"),
            (with(KeyCode::Backspace, KeyModifiers::ALT), 0, text),
            (with(KeyCode::Delete, KeyModifiers::CONTROL), end, text),
        ] {
            let mut field = editor(text, cursor);
            field.handle_key(key);
            assert_eq!(field.current_input(), left, "{key:?} at {cursor}");
        }
        // Ctrl+W is not a deletion here: Option+Backspace and Ctrl+Backspace are the word keys
        let mut field = editor(text, end);
        field.handle_key(with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(field.current_input(), text);
        let mut field = editor("", 0);
        field.handle_key(with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(field.current_input(), "", "nothing before an empty draft");
        // Unmodified, Backspace and Delete still take one character
        let mut field = editor(text, "one".len());
        field.handle_key(with(KeyCode::Backspace, KeyModifiers::NONE));
        field.handle_key(with(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(field.current_input(), "ontwo\nthree four");
    }

    /// Shift extends a selection by words; a deletion takes the selection when there is one; a
    /// plain jump drops it.
    #[test]
    fn shift_selects_by_word_and_deletions_take_a_selection() {
        let text = "alpha beta gamma";
        let mut field = editor(text, text.len());
        let back = with(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        field.handle_key(back);
        field.handle_key(back);
        assert_eq!(field.selected_text().as_deref(), Some("beta gamma"));
        field.handle_key(with(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(field.current_input(), "alpha ", "the selection, not one more word");

        let mut field = editor(text, text.len());
        field.handle_key(back);
        field.handle_key(with(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(field.selection(), None);
        assert_eq!(field.cursor, "alpha ".len());
    }

    /// A paste placeholder is one word to a jump or a deletion, and deleting it drops the text it
    /// stood for.
    #[test]
    fn a_paste_placeholder_is_one_word() {
        let mut field = Editor::new();
        field.insert_text("see ");
        field.paste_text("one\ntwo\nthree\nfour");
        let placeholder_end = field.cursor;
        field.insert_text(" ok");
        let end = field.cursor;

        field.handle_key(with(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(field.cursor, placeholder_end + 1, "the word after it");
        field.handle_key(with(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(field.cursor, "see ".len(), "the placeholder, whole");
        field.handle_key(with(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(field.cursor, placeholder_end);
        field.cursor = end;
        field.handle_key(with(KeyCode::Backspace, KeyModifiers::ALT));
        field.handle_key(with(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(field.current_input(), "see ");
        assert_eq!(field.expanded_input(), "see ", "the pasted text went with it");

        field.paste_text("one\ntwo\nthree\nfour");
        field.cursor = "see ".len();
        field.handle_key(with(KeyCode::Delete, KeyModifiers::CONTROL));
        assert_eq!(field.expanded_input(), "see ");
    }
}
