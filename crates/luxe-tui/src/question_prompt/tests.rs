use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::question::{QuestionOption, QuestionRequestId};

fn state(multi: bool) -> QuestionPromptState {
    QuestionPromptState::new(
        QuestionRequestId::new(),
        "Pick a stack".into(),
        vec!["Rust".into(), "Go".into()],
        multi,
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn theme() -> crate::theme::styles::ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("dark").expect("dark theme resolves");
    crate::theme::styles::ThemeStyles::from_resolved(&resolved, "dark")
}

/// A flattened buffer cut back into its rows.
fn grid(text: &str, width: u16) -> Vec<String> {
    text.chars()
        .collect::<Vec<_>>()
        .chunks(usize::from(width))
        .map(|row| row.iter().collect())
        .collect()
}

/// Render into a fixed grid and return both the flattened text and the click regions.
fn render(
    state: &QuestionPromptState,
    width: u16,
    height: u16,
) -> (String, Vec<(ratatui::layout::Rect, crate::mouse::MouseTarget)>) {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let theme = theme();
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            render_question_prompt(
                frame,
                Rect::new(0, 0, width, height),
                height,
                state,
                &theme,
                &hit,
            )
        })
        .expect("render question prompt");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    let regions = hit.borrow().clicks().map(|(rect, target)| (rect, target.clone())).collect();
    (text, regions)
}

/// The typed answer is an option like the others: numbered after them, with a checkbox of its
/// own in multi-select, and its number picks it.
#[test]
fn the_typed_answer_is_numbered_and_checked_like_an_option() {
    let (text, _) = render(&state(false), 50, 20);
    assert!(text.contains("3. Type your own answer"), "{text}");

    let mut s = state(true);
    let (text, _) = render(&s, 50, 20);
    assert!(text.contains("[ ] 3. Type your own answer"), "{text}");
    // its number starts the typing, and typing checks it
    s.on_key(&key(KeyCode::Char('3')));
    assert!(s.editing() && s.selected() == s.custom_row());
    assert!(!s.is_checked(s.custom_row()), "nothing typed, nothing to send");
    for c in "Zig".chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    assert!(s.is_checked(s.custom_row()));
    let (text, _) = render(&s, 50, 20);
    assert!(text.contains(&format!("[{}] 3. Zig", crate::glyphs::Mark::Yes)), "{text}");
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["Zig".into()]));
}

/// Typing from any row picks the typed answer, so Enter sends it; Space unchecks it again
/// without throwing the text away.
#[test]
fn typing_picks_the_typed_answer_and_space_can_unpick_it() {
    let mut s = state(true);
    s.on_key(&key(KeyCode::Char(' '))); // check Rust
    for c in "Zig".chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    s.on_key(&key(KeyCode::Esc)); // done typing
    assert!(s.is_checked(s.custom_row()));
    s.on_key(&key(KeyCode::Char(' ')));
    assert!(!s.is_checked(s.custom_row()) && s.custom() == "Zig", "unchecked, still there");
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["Rust".into()]));
}

/// One answer to a single-select question: a draft left in the typed row does not ride along
/// with an option picked after it.
#[test]
fn a_single_answer_leaves_an_abandoned_draft_behind() {
    let mut s = state(false);
    s.on_key(&key(KeyCode::Char('x')));
    s.on_key(&key(KeyCode::Esc));
    s.select(0);
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["Rust".into()]));
}

/// The pointer resting on the row being typed into keeps the caret; moving to another row
/// puts it away.
#[test]
fn hovering_the_row_being_typed_keeps_it_open() {
    let mut s = state(false);
    s.on_key(&key(KeyCode::Char('x')));
    s.select(s.custom_row());
    assert!(s.editing());
    s.select(0);
    assert!(!s.editing());
}

#[test]
fn single_select_enter_answers_the_cursor_option() {
    let mut s = state(false);
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["Rust".into()]));
    s.move_down();
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["Go".into()]));
}

#[test]
fn digit_shortcut_answers_single_select_directly() {
    let mut s = state(false);
    assert_eq!(
        s.on_key(&key(KeyCode::Char('2'))),
        QuestionPromptOutcome::Answer(vec!["Go".into()])
    );
}

#[test]
fn multi_select_toggles_and_submits_checked_options() {
    let mut s = state(true);
    assert_eq!(s.on_key(&key(KeyCode::Char(' '))), QuestionPromptOutcome::Consumed); // check Rust
    s.move_down();
    assert_eq!(s.on_key(&key(KeyCode::Char(' '))), QuestionPromptOutcome::Consumed); // check Go
    assert!(s.is_checked(0) && s.is_checked(1));
    assert_eq!(
        s.on_key(&key(KeyCode::Enter)),
        QuestionPromptOutcome::Answer(vec!["Rust".into(), "Go".into()])
    );
}

#[test]
fn typing_jumps_to_free_text_and_enter_submits_it() {
    let mut s = state(false);
    assert_eq!(s.on_key(&key(KeyCode::Char('x'))), QuestionPromptOutcome::Consumed);
    assert!(s.editing());
    assert_eq!(s.custom(), "x");
    for c in "yz".chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["xyz".into()]));
}

/// Pasting is typing: an answer copied from somewhere else lands in the free-text row rather
/// than in the composer behind the modal.
#[test]
fn a_paste_lands_in_the_free_text_row() {
    let mut s = state(false);
    s.paste("copied answer\n");
    assert!(s.editing());
    assert_eq!(s.custom(), "copied answer");
    assert_eq!(s.selected(), s.custom_row(), "and the cursor followed it there");

    s.paste("  ");
    assert_eq!(s.custom(), "copied answer", "a paste of nothing types nothing");
    assert_eq!(
        s.on_key(&key(KeyCode::Enter)),
        QuestionPromptOutcome::Answer(vec!["copied answer".into()])
    );
}

#[test]
fn free_text_combines_with_checked_options_in_multi() {
    let mut s = state(true);
    s.on_key(&key(KeyCode::Char(' '))); // check Rust
    s.select(s.custom_row());
    s.on_key(&key(KeyCode::Char(' '))); // enter edit on free-text row
    assert!(s.editing());
    for c in "Zig".chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    assert_eq!(
        s.on_key(&key(KeyCode::Enter)),
        QuestionPromptOutcome::Answer(vec!["Rust".into(), "Zig".into()])
    );
}

#[test]
fn esc_cancels_when_not_editing_and_exits_edit_first() {
    let mut s = state(false);
    s.on_key(&key(KeyCode::Char('a'))); // start editing
    assert!(s.editing());
    assert_eq!(s.on_key(&key(KeyCode::Esc)), QuestionPromptOutcome::Consumed); // exit edit
    assert!(!s.editing());
    assert_eq!(s.on_key(&key(KeyCode::Esc)), QuestionPromptOutcome::Cancel);
    // now cancel
}

#[test]
fn render_shows_title_prompt_options_and_free_text_row() {
    let (text, _) = render(&state(true), 60, 14);
    assert!(text.contains("Question"), "title present");
    assert!(text.contains("Pick a stack"), "prompt present");
    assert!(text.contains("1. Rust"), "first option present");
    assert!(text.contains("[ ]"), "multi checkbox present");
    assert!(text.contains("Type your own answer"), "free-text row present");
    assert!(text.contains('❯'), "selection cursor present");
    assert!(text.contains('╭'), "rounded panel border present");
}

fn advised() -> QuestionPromptState {
    QuestionPromptState::new(
        QuestionRequestId::new(),
        "Which runtime?".into(),
        vec![
            QuestionOption::new("async-std").with_description("smaller, fewer users"),
            QuestionOption::new("tokio")
                .with_description("already a dependency here")
                .recommended(),
        ],
        false,
    )
}

#[test]
fn the_cursor_opens_on_the_recommended_option() {
    let mut s = advised();
    assert_eq!(s.selected(), 1, "the recommendation, not the first row");
    // So Enter answers with it, without the user weighing every row first.
    assert_eq!(s.on_key(&key(KeyCode::Enter)), QuestionPromptOutcome::Answer(vec!["tokio".into()]));
}

#[test]
fn render_shows_the_rationale_and_the_recommended_badge() {
    let (text, _) = render(&advised(), 60, 20);
    assert!(text.contains("1. async-std"), "labels stay numbered");
    assert!(text.contains("smaller, fewer users"), "rationale rendered");
    assert!(text.contains("already a dependency here"));
    assert!(text.contains("recommended"), "the recommendation is badged, not implied");
}

#[test]
fn click_regions_cover_the_rows_a_wrapped_rationale_actually_occupies() {
    let long = "a".repeat(30) + " " + &"b".repeat(30);
    let s = QuestionPromptState::new(
        QuestionRequestId::new(),
        "Pick".into(),
        vec![QuestionOption::new("first").with_description(&long), QuestionOption::new("second")],
        false,
    );
    let (_, regions) = render(&s, 40, 20);

    let rows: Vec<_> = regions
        .iter()
        .filter_map(|(rect, target)| match target {
            crate::mouse::MouseTarget::QuestionRow(i) => Some((*i, *rect)),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 3, "two options plus the free-text row");
    assert!(rows[0].1.height > 1, "the wrapped rationale is part of its own row");
    // Regions stack without overlapping, so a click never lands on two rows.
    for pair in rows.windows(2) {
        assert_eq!(pair[0].1.bottom(), pair[1].1.y, "rows are contiguous: {rows:?}");
    }
}

#[test]
fn a_short_panel_drops_rationale_instead_of_clipping_the_options() {
    let s = advised();
    let (tall, _) = render(&s, 60, 20);
    assert!(tall.contains("already a dependency here"));

    // Ten rows fits the chrome, prompt, and three option/free-text rows — nothing more. Nine
    // used to, by clipping the keys off the bottom.
    let (short, regions) = render(&s, 60, 10);
    assert!(!short.contains("already a dependency"), "rationale dropped: {short}");
    assert!(short.contains("1. async-std") && short.contains("2. tokio"), "options survive");
    assert!(short.contains("Type your own answer"), "free text survives");
    assert_eq!(
        regions
            .iter()
            .filter(|(_, target)| matches!(target, crate::mouse::MouseTarget::QuestionRow(_)))
            .count(),
        3
    );
}

#[test]
fn a_long_question_wraps_instead_of_being_cut_off() {
    let s = QuestionPromptState::new(
        QuestionRequestId::new(),
        "Should the migration run before or after the index is rebuilt on the replica?".into(),
        vec!["before".into(), "after".into()],
        false,
    );
    let (text, _) = render(&s, 44, 20);
    assert!(text.contains("Should the migration run"));
    assert!(text.contains("replica?"), "the tail of the question survives: {text}");
}

#[test]
fn a_narrow_panel_drops_rationale_rather_than_shredding_it() {
    let (wide, _) = render(&advised(), 60, 20);
    assert!(wide.contains("already a dependency here"));

    // Under ~24 columns of rationale width the wrap turns into fragments; labels alone win.
    // Four of those columns are the slab's frame — a cap and the pad inside it at each end —
    // which every row spends whether or not it is the chosen one, and that is what keeps the
    // label column from shifting sideways as the cursor moves.
    let (narrow, _) = render(&advised(), 26, 20);
    assert!(!narrow.contains("already"), "rationale dropped: {narrow}");
    assert!(narrow.contains("1. async-std"), "full labels survive: {narrow}");
    assert!(narrow.contains("2. tokio"), "including the recommended one: {narrow}");
    assert!(!narrow.contains("recommended"), "the badge yields to the label, not vice versa");
}

/// The label column sits at the same cell on every row, chosen or not. A list that shifts
/// sideways under the cursor is the thing the slab's always-spent frame cells exist to prevent.
#[test]
fn the_option_column_does_not_move_under_the_cursor() {
    const WIDTH: u16 = 60;
    let mut state = advised();
    // The buffer is a flat run of cells, so the column is the offset within its own row.
    let column_of = |state: &QuestionPromptState| {
        let (text, _) = render(state, WIDTH, 20);
        let cells: Vec<char> = text.chars().collect();
        let label: Vec<char> = "1. async-std".chars().collect();
        let at = cells.windows(label.len()).position(|window| window == label.as_slice())?;
        Some(at % usize::from(WIDTH))
    };
    // `advised()` opens on the recommended option, so option 1 starts unselected.
    let unselected = column_of(&state).expect("the first option renders");
    state.select(0);
    let selected = column_of(&state).expect("and still does when chosen");
    assert_eq!(unselected, selected, "the label must not move as the cursor lands on it");
}

#[test]
fn a_short_panel_spends_its_last_rows_on_the_options_not_the_question() {
    let s = QuestionPromptState::new(
        QuestionRequestId::new(),
        "Should the migration run before or after the replica index is rebuilt?".into(),
        vec!["before".into(), "after".into()],
        false,
    );
    // Wide enough to wrap the question over three rows, too short to afford them.
    let (text, _) = render(&s, 44, 11);
    assert!(text.contains("1. before") && text.contains("2. after"), "options: {text}");
    assert!(text.contains("Type your own answer"), "free text survives");
    assert!(text.contains("esc cancel"), "the way out survives");
}

/// A footer too wide for the panel drops whole keys and keeps the way out, rather than cutting
/// the last one to `Es…`.
#[test]
fn a_narrow_footer_drops_keys_not_letters() {
    let s = QuestionPromptState::new(
        QuestionRequestId::new(),
        "Which?".into(),
        vec!["a".into(), "b".into()],
        true,
    );
    let (text, _) = render(&s, 52, 14);
    assert!(text.contains("esc cancel"), "the way out survives: {text}");
    assert!(!text.contains('…'), "no key is cut short: {text}");
}

#[test]
fn wrapped_rows_clips_with_an_ellipsis_and_skips_empty_text() {
    assert!(wrapped_rows("   ", 20, 2).is_empty());
    assert!(wrapped_rows("text", 0, 2).is_empty());
    let rows = wrapped_rows(&"word ".repeat(20), 10, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].ends_with('…'), "clipped tail is marked: {rows:?}");
    assert!(rows.iter().all(|row| row.chars().count() <= 10));
    // Text that exactly fills its rows was not clipped, so it is not marked as though it was.
    assert_eq!(wrapped_rows("aaaa bbbb", 4, 2), vec!["aaaa", "bbbb"]);
}

/// Typing an answer the row cannot hold wraps it. Truncating instead capped how much of their
/// own answer a reader could ever see, which is most of what makes a free-text row usable.
#[test]
fn a_typed_answer_wraps_instead_of_being_cut_off() {
    let mut s = state(false);
    let answer = "keep the runtime minimal and pull in only what the first milestone needs";
    for c in answer.chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    let (text, regions) = render(&s, 50, 24);
    assert!(text.contains("keep the runtime minimal"), "the head is on screen");
    assert!(text.contains("first milestone needs"), "and so is the tail: {text}");
    assert!(!text.contains('…'), "nothing was clipped: {text}");

    // The number and the cursor belong to the row, not to each of its lines: both sit on the
    // first, and the wrap below them runs under the text rather than under the chrome.
    let rows = grid(&text, 50);
    let numbered = rows.iter().position(|row| row.contains("3. keep")).expect("numbered");
    assert!(rows[numbered].contains('❯'), "the cursor sits on the row's first line");
    assert_eq!(rows.iter().filter(|row| row.contains('❯')).count(), 1, "and only there");
    let column = |row: &str, text: &str| row.find(text).map(|at| row[..at].chars().count());
    assert_eq!(
        column(&rows[numbered + 1], "only"),
        column(&rows[numbered], "keep"),
        "the wrap runs under the text: {rows:#?}"
    );

    // The row a click has to reach is as tall as the answer made it.
    let custom = regions
        .iter()
        .find(|(_, target)| matches!(target, crate::mouse::MouseTarget::QuestionRow(i) if *i == 2))
        .expect("the free-text row publishes a click region");
    assert!(custom.0.height > 1, "the wrapped answer is part of its own row: {custom:?}");
}

/// Past the rows it is allowed, the window follows the caret — what you are typing now is
/// what you need to see — and says the answer carries on above it.
#[test]
fn a_long_answer_follows_the_caret_and_marks_what_is_above_it() {
    let mut s = state(false);
    for c in "alpha ".repeat(40).chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    for c in "omega".chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    let (typing, _) = render(&s, 50, 30);
    assert!(typing.contains("omega▏"), "the caret stays on screen: {typing}");
    assert!(typing.contains('…'), "and the row says there is more above it");

    // At rest it reads from the top instead, with the clipped tail marked as usual.
    s.on_key(&key(KeyCode::Esc));
    assert!(!s.editing());
    let (resting, _) = render(&s, 50, 30);
    assert!(!resting.contains("omega"), "the tail is folded away: {resting}");
    assert!(resting.contains('…'));
}

/// The answer grows the panel upward; it must not push the options sideways as it goes. The
/// cut is measured from the width, not from the tallest row, for exactly this reason.
#[test]
fn a_growing_answer_leaves_the_option_column_where_it_was() {
    const WIDTH: u16 = 60;
    let column_of = |state: &QuestionPromptState, label: &str| {
        let (text, _) = render(state, WIDTH, 30);
        let cells: Vec<char> = text.chars().collect();
        let label: Vec<char> = label.chars().collect();
        let at = cells.windows(label.len()).position(|window| window == label.as_slice())?;
        Some(at % usize::from(WIDTH))
    };
    for (mut s, label) in [(state(false), "1. Rust"), (advised(), "1. async-std")] {
        let before = column_of(&s, label).expect("the first option renders");
        for c in "a".repeat(400).chars() {
            s.on_key(&key(KeyCode::Char(c)));
        }
        assert_eq!(column_of(&s, label), Some(before), "{label} moved as the answer grew");
    }
}

/// The answer grows to its budget and not a line more, at every width.
///
/// A narrow panel used to get a single row: the slab's two caps were halves of one diagonal, so
/// a row could be no taller than the cut had cells to lean through, and a width that reserved
/// none capped the answer at one line. Square caps have no such ceiling, so the only thing
/// deciding how tall the answer gets is how much answer there is.
#[test]
fn the_answer_takes_its_whole_budget_and_not_a_line_more() {
    let theme = theme();
    for (width, expected) in [
        (26u16, MAX_CUSTOM_ROWS),
        (44, MAX_CUSTOM_ROWS),
        (60, MAX_CUSTOM_ROWS),
        (100, MAX_CUSTOM_ROWS),
    ] {
        let mut s = advised();
        for c in "a longer answer than one row can hold ".repeat(6).chars() {
            s.on_key(&key(KeyCode::Char(c)));
        }
        let rows = lay_out_rows(&s, &theme, inner_width(width), true, MAX_CUSTOM_ROWS);
        let tallest = rows.iter().map(|row| row.lines.len()).max().unwrap_or(0);
        assert_eq!(tallest, expected, "{width}: the answer got {tallest} lines");
        // And the shape still fits: nothing spills past the panel's inner width.
        for row in &rows {
            for line in &row.lines {
                let cells: usize = line
                    .spans
                    .iter()
                    .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
                    .sum();
                assert!(cells <= inner_width(width), "{width}: a line of {cells} cells");
            }
        }
    }
}

/// A panel with no room left shrinks the answer's window rather than clipping itself — and
/// shrinks it no further than it has to, because every row it gives up is answer nobody can
/// read. The caret and the way out are what a clipped panel loses first.
#[test]
fn a_short_panel_shrinks_the_answer_before_it_clips_itself() {
    let mut s = advised();
    for c in "a longer answer than one row can hold ".repeat(3).chars() {
        s.on_key(&key(KeyCode::Char(c)));
    }
    let (text, _) = render(&s, 60, 12);
    assert!(text.contains('▏'), "the caret is on screen: {text}");
    assert!(text.contains("esc cancel"), "and so is the way out: {text}");
    assert!(text.contains("1. async-std") && text.contains("2. tokio"), "options: {text}");
    assert_eq!(height(&s, &theme(), 60, 12), 12, "it spends every row it was given");
}

/// What the panel measures is what it draws, or the transcript stands down from the wrong
/// number of rows and the modal covers the output it is asking about.
#[test]
fn the_measured_height_is_the_height_it_draws() {
    let theme = theme();
    let mut s = state(false);
    for width in [40u16, 60, 100] {
        for available in [9u16, 16, 40] {
            let measured = height(&s, &theme, width, available);
            let (text, _) = render(&s, width, available);
            let drawn = grid(&text, width)
                .iter()
                .filter(|row| row.chars().any(|cell| !cell.is_whitespace()))
                .count();
            assert_eq!(usize::from(measured), drawn, "{width}x{available}");
        }
        // And still does once the answer has grown the panel.
        for c in "a longer answer than the row can hold on its own ".repeat(4).chars() {
            s.on_key(&key(KeyCode::Char(c)));
        }
    }
    assert_eq!(height(&s, &theme, 0, 20), 0, "no width, no panel");
}

fn type_answer(state: &mut QuestionPromptState, text: &str) {
    for c in text.chars() {
        state.on_key(&key(KeyCode::Char(c)));
    }
}

/// The typed answer edits where its caret is: the arrows move it, and typing, Backspace and Delete
/// act there rather than at the end of the text.
#[test]
fn the_typed_answer_edits_at_its_caret() {
    let mut state = state(false);
    type_answer(&mut state, "helo");
    state.on_key(&key(KeyCode::Left));
    state.on_key(&key(KeyCode::Left));
    state.on_key(&key(KeyCode::Char('l')));
    assert_eq!(state.custom(), "hello");
    state.on_key(&key(KeyCode::Home));
    state.on_key(&key(KeyCode::Delete));
    assert_eq!(state.custom(), "ello", "delete takes the character after the caret");
    state.on_key(&key(KeyCode::End));
    state.on_key(&key(KeyCode::Backspace));
    assert_eq!(state.custom(), "ell");
    assert!(state.editing(), "none of that left the answer");
}

/// A word jump moves through the answer the way it does in the composer.
#[test]
fn a_word_jump_moves_through_the_typed_answer() {
    let mut state = state(false);
    type_answer(&mut state, "one two");
    state.on_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    type_answer(&mut state, "big ");
    assert_eq!(state.custom(), "one big two");
}

/// Up and down move through the rows a long answer wraps to, and up from its first row leaves the
/// answer for the option above, the way the launcher hands its arrows to the list.
#[test]
fn up_and_down_move_through_a_wrapped_answer_then_leave_it() {
    let mut state = state(false);
    type_answer(&mut state, "alpha beta gamma delta epsilon zeta eta theta iota kappa");
    // Drawn once, so the answer has wrapped at the panel's width
    render(&state, 30, 24);
    let end = state.answer().cursor();

    state.on_key(&key(KeyCode::Up));
    assert!(state.editing() && state.answer().cursor() < end, "up reached the row above");
    state.on_key(&key(KeyCode::Down));
    assert_eq!(state.answer().cursor(), end, "and down came back");

    // Bounded, so a regression that never leaves fails rather than hangs
    for _ in 0..10 {
        if state.editing() {
            state.on_key(&key(KeyCode::Up));
        }
    }
    assert!(!state.editing(), "up from the first row leaves the answer");
    assert_eq!(state.selected(), 1, "for the option above it");
}

/// An answer short enough for one row has nowhere to go: down leaves it at once.
#[test]
fn down_from_a_one_row_answer_leaves_it() {
    let mut state = state(false);
    type_answer(&mut state, "short");
    render(&state, 60, 24);
    state.on_key(&key(KeyCode::Down));
    assert!(!state.editing());
    assert_eq!(state.selected(), 0, "down from the last row wraps to the first option");
}

/// The caret is drawn where typing lands, not after the last character.
#[test]
fn the_caret_is_drawn_where_typing_lands() {
    let mut state = state(false);
    type_answer(&mut state, "abc");
    state.on_key(&key(KeyCode::Left));
    let (text, _) = render(&state, 60, 12);
    assert!(text.contains("ab▏c"), "the caret sits before the c: {text}");
}
