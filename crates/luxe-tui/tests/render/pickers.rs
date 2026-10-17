//! Pickers — the thinking, palette and slash rows answer a click.

use super::*;

/// Picker clicks (plan B4): clicking a thinking-level row selects + applies it (closing the
/// picker) via the shared accept path.
#[test]
fn clicking_a_thinking_level_row_applies_and_closes_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.open_thinking_picker();
    assert!(app.overlays.thinking_picker.is_some(), "picker opens");

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Medium"))
        .expect("a thinking-level row renders");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: width / 2,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("expected a synthetic-accept message from the click, got {other:?}"),
    }
    assert!(app.overlays.thinking_picker.is_none(), "clicking a level applies and closes it");
}

/// The picker only offers what the active model accepts: a level it can't honor is labeled and
/// dimmed, the cursor jumps over it, a click on it does nothing, and the frame names the model the
/// annotations belong to.
#[test]
fn the_thinking_picker_disables_levels_the_model_cannot_honor() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use luxe_core::provider::{ModelInfo, ThinkingLevel};
    let (width, height) = (100, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    // An extended-thinking Claude: it reasons, but there is no xhigh or ultra to ask for.
    app.set_model_catalog(
        Vec::new(),
        Some(ModelInfo {
            display_name: Some("Claude Sonnet 4.5".to_string()),
            reasoning: true,
            thinking_level_map: std::collections::HashMap::from([
                ("high".to_string(), Some("high".to_string())),
                ("xhigh".to_string(), None),
                ("ultra".to_string(), None),
            ]),
            ..Default::default()
        }),
    );
    app.open_thinking_picker();

    let buffer = render_app(&mut app, &backend);
    let screen = buffer_to_string(&buffer);
    assert!(screen.contains("Claude Sonnet 4.5"), "the frame names the model: {screen}");
    // Rows sit inside the modal frame behind a cursor gutter, so strip the chrome before matching.
    let level_row = |label: &str| {
        (0..height)
            .find(|&y| {
                row_text(&buffer, y, width)
                    .trim_start()
                    .trim_start_matches(['│', '❯', ' '])
                    .starts_with(label)
            })
            .unwrap_or_else(|| panic!("no row for {label}: {screen}"))
    };
    assert!(
        !row_text(&buffer, level_row("Medium"), width).contains("not supported"),
        "a level the model honors carries no note"
    );
    for label in ["X-High", "Ultra"] {
        assert!(
            row_text(&buffer, level_row(label), width).contains("not supported"),
            "{label} must be labeled unsupported: {screen}"
        );
    }

    let click = |row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: width / 2,
        row,
        modifiers: KeyModifiers::NONE,
    };
    assert!(
        matches!(route_mouse(&mut app, &click(level_row("Ultra"))), RouteAction::None),
        "clicking an unsupported level does nothing"
    );
    assert!(app.overlays.thinking_picker.is_some(), "the picker stays open");

    match route_mouse(&mut app, &click(level_row("Medium"))) {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("a supported level should accept the click, got {other:?}"),
    }
    assert!(app.overlays.thinking_picker.is_none(), "a supported level applies and closes it");
    assert_eq!(app.status.thinking, ThinkingLevel::Medium);
}

/// Palette clicks (plan B5): clicking a palette row moves the selection there and dispatches it
/// via the shared accept path, closing the palette.
#[test]
fn clicking_a_palette_row_dispatches_and_closes_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (120, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    // Ctrl+K opens the command palette.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    assert!(app.overlays.palette.is_some(), "ctrl+k opens the palette");

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Change theme"))
        .expect("a palette command row renders");
    // On the words, since the pane beside the list is not a row
    let column = row_text(&buffer, row, width).find("Change theme").expect("found above") as u16;
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("expected a synthetic-accept message from the click, got {other:?}"),
    }
    assert!(app.overlays.palette.is_none(), "clicking a palette row dispatches and closes it");
    assert!(app.overlays.theme_picker.is_some(), "and runs the row that was clicked");
}

/// Slash-flyout clicks: clicking a flyout row accepts it via the shared path, and accepting
/// **runs** the command.
///
/// `/diff` takes an optional target, and picking it used to complete `"/diff "` into the composer
/// and stop — which reads as nothing having happened, and was reported as exactly that. Tab is the
/// key for completing an argument; Enter and a click run the command's bare form.
#[test]
fn clicking_a_slash_flyout_row_accepts_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (120, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)));
    assert!(app.overlays.slash_flyout.is_some(), "typing / opens the slash flyout");
    // Filter down to it rather than trusting its position: the flyout shows a bounded window, so
    // asserting against the unfiltered list makes this test fail whenever a command is added
    // anywhere above `/diff`.
    for character in "dif".chars() {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Review changes"))
        .expect("the /diff (Review changes) flyout row renders");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 20,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("expected a synthetic-accept message from the click, got {other:?}"),
    }
    assert!(app.overlays.slash_flyout.is_none(), "accepting closes the flyout");
    assert_eq!(app.editor.current_input(), "", "accepting runs the command and clears the draft");
}
