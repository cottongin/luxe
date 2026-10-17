use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn session(id: &str, name: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        name: name.into(),
        age: "2d ago".into(),
        search_text: name.into(),
        preview: Vec::new(),
        worktree: None,
    }
}

fn returning() -> DashboardState {
    let sessions = vec![
        session("a", "Refine the workspace overview"),
        session("b", "Audit the current project structure"),
        session("c", "Why does the dock group changes by directory"),
        session("d", "Make the model picker remember the last choice"),
    ];
    DashboardState::new(sessions.clone())
        .with_sessions(sessions)
        .with_resume(Some(ResumeTarget {
            id: "a".into(),
            title: "Refine the workspace overview".into(),
            age: "12m ago".into(),
        }))
        .with_briefing(DashBriefing { changed_files: 12, added: 640, removed: 118 })
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(state: &mut DashboardState, editor: &mut Editor, text: &str) {
    for character in text.chars() {
        state.on_key(&key(KeyCode::Char(character)), &KeyMap::defaults(), editor);
    }
}

/// The defect the launcher exists to remove: on the menu, `s` opened the session finder, so a
/// prompt that started "Please…" lost its first letter and opened a search. Here every letter of
/// it is text, and nothing runs until Enter.
#[test]
fn every_letter_is_text_and_nothing_runs_until_enter() {
    let mut state = returning();
    let mut editor = Editor::new();
    for character in "Please review the parser, q n s c r".chars() {
        let outcome =
            state.on_key(&key(KeyCode::Char(character)), &KeyMap::defaults(), &mut editor);
        assert_eq!(outcome, DashOutcome::None, "{character:?} ran something");
    }
    assert_eq!(editor.current_input(), "Please review the parser, q n s c r");
    assert_eq!(
        state.on_key(&key(KeyCode::Enter), &KeyMap::defaults(), &mut editor),
        DashOutcome::Ask
    );
}

/// With nothing typed the list is every door the menu had, Resume first and selected, so Enter
/// alone still picks up where you left off.
#[test]
fn an_empty_prompt_lists_every_door_and_enter_resumes() {
    let mut state = returning();
    let mut editor = Editor::new();
    let entries = state.entries();
    for door in [
        Entry::Resume,
        Entry::NewSession,
        Entry::Sessions,
        Entry::Review,
        Entry::Configuration,
        Entry::Quit,
    ] {
        assert!(entries.contains(&door), "{door:?} is missing");
    }
    assert!(entries.contains(&Entry::Heading("RECENT", None)));
    assert!(!entries.contains(&Entry::Recent(3)), "the resumed session is not listed twice");
    assert_eq!(
        state.on_key(&key(KeyCode::Enter), &KeyMap::defaults(), &mut editor),
        DashOutcome::Resume("a".into())
    );
}

/// Every door is still one row away: arrows reach each and Enter runs it.
#[test]
fn the_arrows_reach_every_door() {
    let mut state = returning();
    let mut editor = Editor::new();
    let keymap = KeyMap::defaults();
    let mut seen = Vec::new();
    for _ in 0..12 {
        seen.push(state.activate());
        state.on_key(&key(KeyCode::Down), &keymap, &mut editor);
    }
    for wanted in [
        DashOutcome::NewSession,
        DashOutcome::OpenPicker(PickerKind::FindSession),
        DashOutcome::ReviewChanges,
        DashOutcome::OpenConfig,
        DashOutcome::Quit,
        DashOutcome::Resume("d".into()),
    ] {
        assert!(seen.contains(&wanted), "{wanted:?} is not reachable: {seen:?}");
    }
}

/// Shift+Tab switches the mode, as it does in a session, and only the arrows choose. Tab moving
/// down would make tab-then-shift+tab look like down-then-up while it switched the mode instead.
#[test]
fn shift_tab_switches_the_mode_and_only_the_arrows_choose() {
    let mut state = returning();
    let mut editor = Editor::new();
    let keymap = KeyMap::defaults();
    let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(state.on_key(&back_tab, &keymap, &mut editor), DashOutcome::CycleMode);
    assert_eq!(state.activate(), DashOutcome::Resume("a".into()), "the cursor stays put");
    assert_eq!(state.on_key(&key(KeyCode::Tab), &keymap, &mut editor), DashOutcome::None);
    assert_eq!(state.activate(), DashOutcome::Resume("a".into()), "tab is not down");
    assert!(editor.current_input().is_empty(), "and types nothing");
    state.on_key(&key(KeyCode::Down), &keymap, &mut editor);
    assert_eq!(state.activate(), DashOutcome::NewSession);
}

/// The mode key is the one the reader bound: rebound, the new chord switches and shift+tab is free.
#[test]
fn a_rebound_mode_key_switches_the_mode() {
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("tui.app.cycle_mode".to_string(), vec!["ctrl+y".to_string()]);
    let keymap = KeyMap::defaults().with_overrides(overrides);
    let mut state = returning();
    let mut editor = Editor::new();
    let chord = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);
    assert_eq!(state.on_key(&chord, &keymap, &mut editor), DashOutcome::CycleMode);
    let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(state.on_key(&back_tab, &keymap, &mut editor), DashOutcome::None);
}

/// Typing turns the list into answers: asking first, then the sessions whose titles match, then
/// the commands — and Enter on a session resumes it with the draft still in hand.
#[test]
fn typing_finds_sessions_by_title_and_commands_by_name() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "dock");
    let entries = state.entries();
    assert_eq!(entries.first(), Some(&Entry::Ask));
    assert!(
        entries.iter().any(|entry| matches!(entry, Entry::Session(2, _))),
        "the session about the dock: {entries:?}"
    );
    state.on_key(&key(KeyCode::Down), &KeyMap::defaults(), &mut editor);
    assert_eq!(state.activate(), DashOutcome::Resume("c".into()));
    assert_eq!(editor.current_input(), "dock", "the draft stays for the session it goes to");

    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "model");
    let commands: Vec<String> = state
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Command(index, _) => state.command(*index).map(|command| command.id.to_string()),
            _ => None,
        })
        .collect();
    assert!(commands.contains(&"cmd.model".to_string()), "{commands:?}");
}

#[test]
fn a_slash_draft_runs_the_command_it_spells() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "/model");
    match state.activate() {
        DashOutcome::RunCommand(command) => assert_eq!(command.id, "cmd.model"),
        other => panic!("expected the command, got {other:?}"),
    }
}

/// Esc clears a draft and otherwise stays put: quitting by reflex from a screen that takes text is
/// how a draft gets lost. Ctrl+C is the way out.
#[test]
fn escape_clears_and_ctrl_c_quits() {
    let mut state = returning();
    let mut editor = Editor::new();
    let keymap = KeyMap::defaults();
    type_text(&mut state, &mut editor, "half a thought");
    assert_eq!(state.on_key(&key(KeyCode::Esc), &keymap, &mut editor), DashOutcome::None);
    assert!(editor.current_input().is_empty());
    assert_eq!(state.entries().first(), Some(&Entry::Resume), "the menu is back");
    assert_eq!(state.on_key(&key(KeyCode::Esc), &keymap, &mut editor), DashOutcome::None);
    let quit = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(state.on_key(&quit, &keymap, &mut editor), DashOutcome::Quit);
}

#[test]
fn with_no_provider_setup_leads_and_takes_the_cursor() {
    let mut state = DashboardState::new(Vec::new());
    state.set_provider_configured(false);
    let entries = state.entries();
    assert_eq!(entries.first(), Some(&Entry::SetUp));
    assert!(!entries.contains(&Entry::Configuration), "one door to setup, not two");
    assert_eq!(state.activate(), DashOutcome::OpenConfig);
    state.set_provider_configured(true);
    assert_eq!(state.entries().first(), Some(&Entry::NewSession));
}

/// The minimal style is the prompt alone until `↓` asks for the list, and typing shows it.
#[test]
fn the_minimal_style_folds_the_list_until_asked() {
    let mut state = returning().with_style(DashboardStyle::Minimal);
    let mut editor = Editor::new();
    assert!(!state.list_shown());
    state.on_key(&key(KeyCode::Down), &KeyMap::defaults(), &mut editor);
    assert!(state.list_shown(), "↓ opens it");
    state.on_key(&key(KeyCode::Esc), &KeyMap::defaults(), &mut editor);
    assert!(!state.list_shown(), "esc folds it again");
    type_text(&mut state, &mut editor, "x");
    assert!(state.list_shown(), "a draft shows what it would match");
}

/// A chord bound to a dashboard action still works; a bare letter bound there would steal a
/// character from every prompt, so it is text.
#[test]
fn a_bound_chord_runs_its_action_and_a_bare_letter_types() {
    let mut overrides = std::collections::HashMap::new();
    overrides
        .insert("tui.dashboard.find_session".to_string(), vec!["ctrl+o".to_string(), "f".into()]);
    let keymap = KeyMap::defaults().with_overrides(overrides);
    let mut state = returning();
    let mut editor = Editor::new();
    let chord = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert_eq!(
        state.on_key(&chord, &keymap, &mut editor),
        DashOutcome::OpenPicker(PickerKind::FindSession)
    );
    assert_eq!(state.on_key(&key(KeyCode::Char('f')), &keymap, &mut editor), DashOutcome::None);
    assert_eq!(editor.current_input(), "f");
}

#[test]
fn a_session_catalog_skips_sessions_with_no_words() {
    let now = std::time::SystemTime::now();
    let record = |id: &str, title: &str| SessionRecord {
        id: id.into(),
        modified: now,
        title: title.into(),
        search_text: String::new(),
        preview: Vec::new(),
        worktree: None,
    };
    let catalog = DashboardState::populate_session_catalog(
        vec![record("a", "  Fix   the parser "), record("b", "   ")],
        now,
    );
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].name, "Fix the parser");
    assert_eq!(catalog[0].age, "just now");
}

/// A few words are a search; a sentence is a prompt. Once the draft reads as one, the list folds
/// away and Enter asks — a whole sentence echoed into a search row is noise, and a fuzzy match
/// that needs every character in order has stopped finding anything by then.
#[test]
fn a_draft_that_reads_as_a_prompt_stops_searching() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "dock group");
    assert!(state.list_shown() && state.entries().len() > 1, "a fragment searches");

    type_text(&mut state, &mut editor, " changes by directory and explain");
    assert!(state.drafting_a_prompt());
    assert!(!state.list_shown(), "the list folds away");
    assert_eq!(state.entries(), vec![Entry::Ask]);
    assert_eq!(
        state.on_key(&key(KeyCode::Enter), &KeyMap::defaults(), &mut editor),
        DashOutcome::Ask
    );

    // Back under the line, it searches again
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "one two three four five six");
    assert!(state.drafting_a_prompt());
    state.on_key(&key(KeyCode::Backspace), &KeyMap::defaults(), &mut editor);
    for _ in 0..4 {
        state.on_key(&key(KeyCode::Backspace), &KeyMap::defaults(), &mut editor);
    }
    assert!(!state.drafting_a_prompt(), "five words are a search again");
}

/// Once the draft is a prompt the list is gone, so the vertical arrows move through the text.
#[test]
fn a_prompt_draft_takes_the_vertical_arrows() {
    let mut state = returning();
    let mut editor = Editor::new();
    let last = "stalls on large inputs";
    editor.insert_text(&format!("please explain why the parser\n{last}"));
    state.sync(&editor);
    assert!(!state.list_shown());
    let end = editor.cursor();
    state.on_key(&key(KeyCode::Up), &KeyMap::defaults(), &mut editor);
    assert_eq!(editor.cursor(), last.len(), "up reaches the same column a line above");
    state.on_key(&key(KeyCode::Down), &KeyMap::defaults(), &mut editor);
    assert_eq!(editor.cursor(), end, "down comes back");
}

/// With the list showing, the arrows move through a draft of several lines and only choose a row
/// once the cursor is on the draft's edge.
#[test]
fn the_arrows_leave_a_search_draft_only_at_its_edge() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "/compact keep");
    state.on_key(
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        &KeyMap::defaults(),
        &mut editor,
    );
    type_text(&mut state, &mut editor, "notes");
    assert!(state.list_shown(), "a command keeps its rows over two lines");
    state.on_key(&key(KeyCode::Up), &KeyMap::defaults(), &mut editor);
    assert_eq!(editor.cursor(), "notes".len(), "up moves into the first line");
    state.on_key(&key(KeyCode::Down), &KeyMap::defaults(), &mut editor);
    assert_eq!(editor.cursor(), editor.current_input().len(), "down moves back to the last line");
    assert!(matches!(state.activate(), DashOutcome::RunCommand(_)), "and neither chose a row");
}

/// A draft on one line still lets the arrows choose: up at the top row is the list's, not a jump
/// to the start of the text.
#[test]
fn the_arrows_choose_from_a_one_line_draft() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "dock");
    state.on_key(&key(KeyCode::Down), &KeyMap::defaults(), &mut editor);
    assert_eq!(state.activate(), DashOutcome::Resume("c".into()));
    state.on_key(&key(KeyCode::Up), &KeyMap::defaults(), &mut editor);
    assert_eq!(state.activate(), DashOutcome::Ask);
    assert_eq!(editor.cursor(), "dock".len(), "the cursor stays put");
}

/// Shift with an arrow selects text, as it does in a session, wherever the cursor is.
#[test]
fn shift_arrows_select_text_on_the_dashboard() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "dock");
    let shift_up = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT);
    state.on_key(&shift_up, &KeyMap::defaults(), &mut editor);
    assert_eq!(editor.selected_text().as_deref(), Some("dock"));
}

/// A command keeps its rows whatever its arguments, and a paste is always a prompt.
#[test]
fn a_command_is_never_a_prompt_and_a_paste_always_is() {
    let mut state = returning();
    let mut editor = Editor::new();
    type_text(&mut state, &mut editor, "/compact keep the API decisions and the open questions");
    assert!(!state.drafting_a_prompt());
    assert!(matches!(state.entries().first(), Some(Entry::Slash(_))));

    let mut state = returning();
    let mut editor = Editor::new();
    editor.paste_text("line one\nline two\nline three");
    state.sync(&editor);
    assert!(state.drafting_a_prompt());
}
