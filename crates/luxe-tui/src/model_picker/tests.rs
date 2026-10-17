use super::*;

fn entry(provider: &str, id: &str, configured: bool) -> ModelEntry {
    ModelEntry {
        info: ModelInfo { r#ref: ModelRef::new(provider, id), ..Default::default() },
        label: id.to_string(),
        configured,
        unverified: false,
    }
}

#[test]
fn a_row_says_where_its_model_came_from() {
    let configured = entry("foundry-anthropic", "claude-opus-5", true);
    assert!(!meta_suffix(&configured).contains("catalog"));
    assert!(meta_suffix(&entry("anthropic", "claude-opus-5", false)).contains("catalog"));
    // A guess for a resource that could have said what it serves is not passed off as either
    let guess = ModelEntry { unverified: true, ..entry("foundry-anthropic", "x", false) };
    let suffix = meta_suffix(&guess);
    assert!(suffix.contains("unverified") && !suffix.contains("catalog"), "{suffix}");
}

fn catalog() -> Vec<ModelEntry> {
    vec![
        entry("lmstudio", "qwen3.6-35b", true),
        entry("openai", "gpt-5-codex", true),
        entry("anthropic", "claude-sonnet-4-6", true),
        entry("anthropic", "claude-haiku-4-5", false),
    ]
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn opens_on_the_active_model() {
    let picker = ModelPickerState::open(catalog(), Some(ModelRef::new("openai", "gpt-5-codex")));
    assert_eq!(
        picker.selected_entry().map(ModelEntry::qualified_id).as_deref(),
        Some("openai/gpt-5-codex")
    );
}

#[test]
fn filters_by_provider_and_id() {
    let mut picker = ModelPickerState::open(catalog(), None);
    for character in "claude".chars() {
        picker.on_key(&key(KeyCode::Char(character)));
    }
    assert_eq!(picker.match_count(), 2);
}

#[test]
fn enter_selects_the_full_model_info() {
    let mut picker =
        ModelPickerState::open(catalog(), Some(ModelRef::new("lmstudio", "qwen3.6-35b")));
    let outcome = picker.on_key(&key(KeyCode::Enter));
    match outcome {
        ModelPickerOutcome::Select(info) => {
            assert_eq!(info.r#ref, ModelRef::new("lmstudio", "qwen3.6-35b"));
        }
        other => panic!("expected a selection, got {other:?}"),
    }
}

#[test]
fn esc_closes() {
    let mut picker = ModelPickerState::open(catalog(), None);
    assert_eq!(picker.on_key(&key(KeyCode::Esc)), ModelPickerOutcome::Close);
}

#[test]
fn printable_keys_filter_rather_than_navigate() {
    let mut picker = ModelPickerState::open(catalog(), None);
    picker.on_key(&key(KeyCode::Char('n')));
    assert_eq!(picker.query(), "n");
}

/// A list that arrives while someone is reading it must not move what they were looking at.
///
/// The local refresh lands off the frame, so the picker is already open and possibly already
/// filtered — a rebuild that resets either is a row changing under the cursor at the moment
/// Enter is pressed.
#[test]
fn a_refreshed_catalog_keeps_the_query_and_the_row_under_the_cursor() {
    let mut picker = ModelPickerState::open(catalog(), None);
    for character in "claude".chars() {
        picker.on_key(&key(KeyCode::Char(character)));
    }
    picker.on_key(&key(KeyCode::Down));
    let chosen = picker.selected_entry().map(ModelEntry::qualified_id);
    assert_eq!(chosen.as_deref(), Some("anthropic/claude-haiku-4-5"));

    // The server answered with one more model. Nothing the reader had is disturbed.
    let mut grown = catalog();
    grown.push(entry("lmstudio", "gemma-4-12b", true));
    picker.refresh(grown);
    assert_eq!(picker.query(), "claude");
    assert_eq!(
        picker.selected_entry().map(ModelEntry::qualified_id),
        chosen,
        "the row is re-found by id, not by position"
    );

    // A model that has gone away takes the cursor to the top, which is all that is left.
    picker.refresh(vec![entry("anthropic", "claude-sonnet-4-6", true)]);
    assert_eq!(
        picker.selected_entry().map(ModelEntry::qualified_id).as_deref(),
        Some("anthropic/claude-sonnet-4-6")
    );
}

/// The opening line is measured, not guessed at: a panel sized without it clips its own
/// hint row out through the bottom border, and one sized to the rows alone truncates it.
#[test]
fn an_opening_line_is_paid_for_in_height_and_width() {
    let area = Rect::new(0, 0, 200, 40);
    let plain = ModelPickerState::open(vec![entry("openai", "gpt-5.6", true)], None);
    let noted =
        ModelPickerState::open(vec![entry("openai", "gpt-5.6", true)], None).with_note(vec![
            "Connected to LM Studio (local).".to_string(),
            "Now on some-rather-long-local-build-name-27b — enter keeps it, or choose another."
                .to_string(),
        ]);

    // Measured twice: once with enough rows that the list decides the height, and once
    // filtered down to where the panel's own floor does. A note has to be paid for in both,
    // and it is the floor that hides an error in the other.
    let many: Vec<ModelEntry> =
        (0..12).map(|index| entry("openai", &format!("gpt-{index}"), true)).collect();
    let long = |note: Vec<String>| ModelPickerState::open(many.clone(), None).with_note(note);
    assert_eq!(
        long(noted.note.clone()).desired_height(area),
        long(Vec::new()).desired_height(area) + 3,
        "two lines and the blank under them"
    );
    assert_eq!(
        noted.desired_height(area),
        plain.desired_height(area) + 3,
        "and the floor a short list falls to moves with it"
    );
    assert!(
        noted.content_modal_width(area.width) > plain.content_modal_width(area.width),
        "a line longer than every row widens the panel"
    );
    // Which is the promise: at the width it asked for, the line is drawn whole.
    assert!(
        drawn(&noted, area).contains("choose another."),
        "the opening line is cut by the panel it sized:\n{}",
        drawn(&noted, area)
    );
}

/// On a terminal narrower than its longest row, a row ends in an ellipsis inside the frame,
/// and the one under the cursor keeps its closing cap; the frame used to cut both off.
#[test]
fn a_row_too_long_for_the_frame_is_shortened_inside_it() {
    let long = "an-uncommonly-long-model-identifier-that-will-not-fit-in-seventy-six-columns";
    let picker =
        ModelPickerState::open(vec![entry("anthropic", long, true), entry("x", long, true)], None);
    let screen = drawn(&picker, Rect::new(0, 0, 76, 20));
    let rows: Vec<&str> = screen.lines().filter(|row| row.contains("an-uncommonly")).collect();
    assert_eq!(rows.len(), 2, "{screen}");
    for row in &rows {
        let inside = row.trim_end().strip_suffix('│').expect("the frame's edge");
        assert!(inside.trim_end_matches(['▌', ' ']).ends_with('…'), "{screen}");
    }
    assert!(rows[0].trim_end().ends_with("▌│"), "the cursor's row keeps its cap: {screen}");
}

/// The picker as it reaches the screen, centered in `area` at its own fitted width.
fn drawn(picker: &ModelPickerState, area: Rect) -> String {
    use ratatui::{backend::TestBackend, Terminal};

    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("a terminal");
    terminal.draw(|frame| picker.view(frame, frame.area(), &theme, &hit)).expect("a frame");
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
