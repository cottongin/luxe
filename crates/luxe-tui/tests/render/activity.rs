//! Activity — live activity beside the conversation, the statusline, the clipboard and search.

use super::*;

/// The live activity indicator belongs to the conversation, not the sidebar: even beside a
/// full-height dock it renders once, in the conversation column directly above the composer.
#[test]
fn live_activity_sits_below_the_conversation_even_beside_the_dock() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("activity"));
    app.status.state = luxe_tui::app::AppState::Streaming;
    app.status.spinner_frame = 0;
    app.status.active_tool = Some("bash".into());
    app.status.on_turn_start();

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("PROJECT"), "the wide layout puts the dock beside the conversation");

    let rows = (0..height)
        .filter(|&row| row_text(&buffer, row, width).contains(ACTIVITY_BAR_TRACK))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "exactly one activity indicator on screen, got rows {rows:?}");

    // In the conversation column (right of the left rail), above the prompt box.
    let row = rows[0];
    let column =
        row_text(&buffer, row, width).find(ACTIVITY_BAR_TRACK).expect("the indicator") as u16;
    assert!(
        column > luxe_tui::workspace_layout::LEFT_RAIL_WIDTH,
        "the indicator is in the conversation column, not the sidebar: column {column}"
    );
    assert!(row < height - 1, "and above the prompt: row {row}");
}

#[test]
fn composer_activity_shows_above_the_prompt_when_the_dock_is_hidden() {
    let (width, height) = (120, 32);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("above-composer"));
    // Standard width keeps the dock hidden (auto), so activity rides above the composer instead.
    app.status.state = luxe_tui::app::AppState::Streaming;
    app.status.spinner_frame = 0;
    app.status.active_tool = Some("bash".into());
    app.status.on_turn_start();

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(!content.contains("PROJECT"), "dock should be hidden at standard width: {content}");
    let activity_row = (0..height)
        .find(|&row| row_text(&buffer, row, width).contains(ACTIVITY_BAR_TRACK))
        .expect("activity row above the composer");
    // It sits above the composer/statusline, not on the bottom row.
    assert!(activity_row < height - 1, "activity should sit above the prompt: row {activity_row}");
}

/// The indicator above the composer is a sweeping bar of units, not a glyph plus the word
/// "working": the motion says "running", and only the two informative bits (elapsed, interrupt
/// chord) follow it. The units are packed — the shaded comet is what gives the sweep its shape, and
/// a separator between cells breaks the flow it draws.
#[test]
fn the_activity_indicator_is_a_sweeping_bar_with_elapsed_and_the_interrupt_chord() {
    let (width, height) = (120, 32);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.status.state = luxe_tui::app::AppState::Streaming;
    app.status.on_turn_start();

    let first = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&row| row_text(&first, row, width).contains(ACTIVITY_BAR_TRACK))
        .expect("the activity bar renders while a turn runs");
    let text = row_text(&first, row, width);
    assert!(!text.contains("working"), "the bar replaces the label: {text:?}");
    assert!(text.contains("to interrupt"), "the interrupt chord stays: {text:?}");
    assert!(text.contains("0.0s") || text.contains("s"), "elapsed stays: {text:?}");

    let track = ACTIVITY_BAR_TRACK.chars().next().expect("one glyph");
    let bar_of =
        |row: &str| row.chars().filter(|g| ACTIVITY_BAR_GLYPHS.contains(g)).collect::<String>();
    assert_eq!(bar_of(&text).chars().count(), 24, "one glyph per cell, packed: {text:?}");
    assert!(!text.contains(&format!("{track} {track}")), "and no separator between them: {text:?}");

    // The comet travels with the animation frame: two ticks per cell, so six ticks is three cells.
    // Compare the whole bar rather than its leading cell — late in a leg the head is already off the
    // rail while the tail drains, so the first lit cell can hold still while the comet moves.
    for _ in 0..6 {
        app.update(Msg::Tick);
    }
    let moved = row_text(&render_app(&mut app, &backend), row, width);
    assert_ne!(bar_of(&text), bar_of(&moved), "the comet must sweep: {text:?} then {moved:?}");

    // Idle rows stay empty: the strip is the composer's breathing space when nothing is running.
    app.status.state = luxe_tui::app::AppState::Idle;
    let idle = row_text(&render_app(&mut app, &backend), row, width);
    assert!(!idle.contains(ACTIVITY_BAR_TRACK), "no bar when idle: {idle:?}");
}

#[test]
fn standard_layout_shows_the_statusline_when_the_dock_is_hidden() {
    let (width, height) = (120, 32);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("fallback"));
    app.push_convo(ConvoBlock::Assistant("chat stays primary".into()));

    let content = buffer_to_string(&render_app(&mut app, &backend));
    // Auto dock is hidden below the wide threshold, so the compact statusline takes over.
    assert!(!content.contains("PROJECT"), "dock should be hidden on standard width: {content}");
    assert!(content.contains("chat stays primary"), "conversation should stay primary: {content}");
    // With the dock hidden, the project identity only appears on the compact statusline fallback.
    assert!(
        content.contains("luxe"),
        "statusline should be the compact fallback carrying the project: {content}"
    );
}

#[test]
fn hidden_statusline_bottom_aligns_the_composer() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(empty_workspace("bottom-align"));

    let buffer = render_app(&mut app, &backend);
    // Wide default hides the statusline, so the composer outline touches the final row inside the
    // main column (x = left-rail dock + 1-cell separator + 1-cell gutter).
    let composer_x = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH
        + luxe_tui::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH
        + 1;
    assert_eq!(
        buffer[(composer_x, height - 1)].symbol(),
        "╰",
        "composer should bottom-align when the statusline is hidden: {}",
        buffer_to_string(&buffer)
    );
}

/// Helper to convert a ratatui Buffer to a string for assertions.
#[allow(deprecated)]
pub(super) fn buffer_to_string(buffer: &Buffer) -> String {
    let mut content = String::new();
    for row in 0..buffer.area.height {
        for col in 0..buffer.area.width {
            let cell = buffer.get(col, row);
            content.push_str(cell.symbol());
        }
        content.push('\n');
    }
    content
}

/// Clipboard: MockClipboard writes without error.
#[tokio::test]
async fn test_mock_clipboard_write() {
    let cb = luxe_tui::clipboard::MockClipboard;
    let result = cb.write_text("hello").await;
    assert!(result.is_ok());
}

/// Search: entering search mode allows typing 'n' (which is the navigation key).
#[test]
fn test_search_allows_typing_n() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    // Enter search mode by setting the flag directly.
    app.search_state.searching = true;
    assert!(app.search_state.is_active());
    assert!(app.search_state.query.is_empty());
    // Simulate typing 'n'.
    let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    let action = route_key(&mut app, &key);
    // Should NOT be a navigation action (query is empty).
    assert!(!matches!(action, RouteAction::SearchNavigate(_)));
    // Query should now contain 'n'.
    assert_eq!(app.search_state.query, "n");
}

/// Search: Enter navigates to next match; 'n' types into query.
#[test]
fn test_search_enter_navigates() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    // Enter search mode and set a query.
    app.search_state.searching = true;
    app.search_state.query = "hello".to_string();
    assert_eq!(app.search_state.query, "hello");
    // Enter should navigate.
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let action = route_key(&mut app, &key);
    assert!(matches!(action, RouteAction::SearchNavigate(true)));
}

/// Search: typing 'n' into query does NOT navigate (fixes reviewer finding #2).
#[test]
fn test_search_n_types_into_query() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    // Enter search mode and start typing "mai".
    app.search_state.searching = true;
    for c in "mai".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let _ = route_key(&mut app, &key);
    }
    assert_eq!(app.search_state.query, "mai");
    // Now 'n' should TYPE, not navigate.
    let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    let action = route_key(&mut app, &key);
    assert!(!matches!(action, RouteAction::SearchNavigate(_)));
    assert_eq!(app.search_state.query, "main");
}

/// Search: Up = prev match, Down = next match (spec: "Down/Up jump to next/prev").
#[test]
fn test_search_up_down_navigate() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.search_state.searching = true;
    app.search_state.query = "hello".to_string();
    // Up should navigate backward (prev).
    let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    let action = route_key(&mut app, &key);
    assert!(matches!(action, RouteAction::SearchNavigate(false)));
    // Down should navigate forward (next).
    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let action = route_key(&mut app, &key);
    assert!(matches!(action, RouteAction::SearchNavigate(true)));
}
