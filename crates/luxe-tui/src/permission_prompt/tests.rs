use super::*;

fn state() -> PermissionPromptState {
    PermissionPromptState::new(
        PermissionRequestId::new(),
        "bash".into(),
        "Run: rm -rf build".into(),
        vec!["bash rm *".into()],
    )
}

/// A prompt with nothing to remember, the shape a hook's escalation arrives in.
fn unrememberable() -> PermissionPromptState {
    PermissionPromptState::new(
        PermissionRequestId::new(),
        "bash".into(),
        "Run: rm -f a.txt — recursive delete".into(),
        Vec::new(),
    )
}

/// A key the reader pressed deliberately, after the grace period.
fn press(state: &mut PermissionPromptState, code: KeyCode) -> PermissionPromptOutcome {
    state.on_key_at(&key(code), state.opened_at + GRACE)
}

/// The count the height is measured from has to be the count the panel draws, or the
/// conversation gives up the wrong number of rows.
#[test]
fn the_options_are_counted_where_the_height_is() {
    assert_eq!(state().options().len(), 4);
    assert_eq!(height(&state(), u16::MAX), HEADER_LINES + 4 + CHROME_ROWS);
    assert_eq!(height(&unrememberable(), u16::MAX), HEADER_LINES + 2 + CHROME_ROWS);
    assert_eq!(height(&state(), 4), 4, "a short terminal clips rather than overflowing");
}

#[test]
fn enter_answers_the_selected_option() {
    let mut s = state();
    assert_eq!(
        press(&mut s, KeyCode::Enter),
        PermissionPromptOutcome::Respond(PermissionReply::Once)
    );
    s.move_down();
    assert_eq!(
        press(&mut s, KeyCode::Enter),
        PermissionPromptOutcome::Respond(PermissionReply::Always)
    );
}

#[test]
fn esc_and_ctrl_c_reject() {
    let mut s = state();
    assert_eq!(
        press(&mut s, KeyCode::Esc),
        PermissionPromptOutcome::Respond(PermissionReply::Reject)
    );
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(
        s.on_key_at(&ctrl_c, s.opened_at + GRACE),
        PermissionPromptOutcome::Respond(PermissionReply::Reject)
    );
}

#[test]
fn digits_answer_by_position() {
    let mut s = state();
    assert_eq!(
        press(&mut s, KeyCode::Char('1')),
        PermissionPromptOutcome::Respond(PermissionReply::Once)
    );
    assert_eq!(
        press(&mut s, KeyCode::Char('2')),
        PermissionPromptOutcome::Respond(PermissionReply::Always)
    );
    assert_eq!(
        press(&mut s, KeyCode::Char('3')),
        PermissionPromptOutcome::Respond(PermissionReply::AlwaysPersist)
    );
    assert_eq!(
        press(&mut s, KeyCode::Char('4')),
        PermissionPromptOutcome::Respond(PermissionReply::Reject)
    );
    assert_eq!(press(&mut s, KeyCode::Char('5')), PermissionPromptOutcome::Consumed);
}

/// Letters are what a sentence is made of, and the card opens mid-sentence.
///
/// `w` used to mean "save this approval forever": typing "Now run…" into the composer as a
/// card appeared wrote `bash rm *: allow` into the global permissions file.
#[test]
fn no_letter_answers_the_prompt() {
    let mut s = state();
    for letter in "asdwrqyno".chars() {
        assert_eq!(
            press(&mut s, KeyCode::Char(letter)),
            PermissionPromptOutcome::Consumed,
            "{letter}"
        );
    }
}

/// Right after the card opens, even a deliberate-looking key is typing that was already in
/// flight — except a reject, which is always safe to honor.
#[test]
fn the_grace_period_swallows_everything_but_a_reject() {
    let mut s = state();
    let early = s.opened_at + GRACE / 2;
    for code in [KeyCode::Enter, KeyCode::Char('1'), KeyCode::Char('3')] {
        assert_eq!(s.on_key_at(&key(code), early), PermissionPromptOutcome::Consumed, "{code:?}");
    }
    assert_eq!(
        s.on_key_at(&key(KeyCode::Esc), early),
        PermissionPromptOutcome::Respond(PermissionReply::Reject)
    );
    assert_eq!(
        press(&mut s, KeyCode::Enter),
        PermissionPromptOutcome::Respond(PermissionReply::Once)
    );
}

/// Nothing to remember means no remembering rows, rather than rows that quietly mean "once".
#[test]
fn an_unrememberable_prompt_offers_once_or_reject() {
    let mut s = unrememberable();
    let replies: Vec<PermissionReply> = s.options().iter().map(|(reply, ..)| *reply).collect();
    assert_eq!(replies, [PermissionReply::Once, PermissionReply::Reject]);
    assert_eq!(
        press(&mut s, KeyCode::Char('2')),
        PermissionPromptOutcome::Respond(PermissionReply::Reject)
    );
    assert_eq!(press(&mut s, KeyCode::Char('3')), PermissionPromptOutcome::Consumed);
    s.move_down();
    s.move_down();
    assert_eq!(s.selected(), 0, "the cursor wraps over the rows that are there");
}

#[test]
fn arrows_wrap_and_other_keys_are_swallowed() {
    let mut s = state();
    s.move_up();
    assert_eq!(s.selected(), s.options().len() - 1);
    assert_eq!(press(&mut s, KeyCode::Char('x')), PermissionPromptOutcome::Consumed);
}

#[test]
fn click_selects_within_range() {
    let mut s = state();
    s.select(99);
    assert_eq!(s.selected(), s.options().len() - 1);
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn render_shows_title_summary_options_and_rounded_border() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("dark").expect("dark theme resolves");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "dark");
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let s = state();
    let mut terminal = Terminal::new(TestBackend::new(60, 12)).expect("test terminal");
    terminal
        .draw(|frame| {
            let region = Rect::new(0, 0, 60, 11);
            render_permission_prompt(frame, region, 11, &s, &theme, &hit);
        })
        .expect("render permission prompt");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("Permission required"), "title present");
    assert!(text.contains("Run: rm -rf build"), "summary present");
    assert!(text.contains("Allow once"), "once option present");
    assert!(text.contains("Allow for this session"), "session option present");
    assert!(text.contains("Allow always"), "persisted option present");
    assert!(text.contains("Reject"), "reject option present");
    assert!(text.contains('❯'), "selection cursor present");
    assert!(text.contains('╭'), "rounded panel border present");
    assert!(text.contains("enter answer · 1–4 pick"), "the keys, in the footer");
    // Bottom-aligned to input_top=11 with content 6 + border 2 + top pad 1 + footer 2 = 11, so
    // area.y=0 and the option rows (border+pad+summary+blank = 4 in) sit at y=4..7.
    let regions = hit.borrow();
    assert_eq!(regions.at(2, 4), Some(&crate::mouse::MouseTarget::PermissionRow(0)));
    assert_eq!(regions.at(2, 7), Some(&crate::mouse::MouseTarget::PermissionRow(3)));
}
