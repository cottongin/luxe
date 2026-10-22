//! Routing — keys and clicks reach the surface they belong to: dashboards, pickers and the review.

use super::*;

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Session-picker clicks (plan B4): clicking a session row resolves straight to SessionSelected
/// for the clicked row (not the default first row).
#[test]
fn clicking_a_session_picker_row_selects_that_session() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (120, 40);
    let backend = test_backend(width, height);
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::TerminalResized { width, height });
    app.overlays.session_picker = Some(SessionPickerState::new(vec![
        SessionEntry {
            id: "session-1".to_string(),
            name: "First session".to_string(),
            age: "just now".to_string(),
            search_text: "first".to_string(),
            preview: Vec::new(),
        },
        SessionEntry {
            id: "session-2".to_string(),
            name: "Second session".to_string(),
            age: "1h ago".to_string(),
            search_text: "second".to_string(),
            preview: Vec::new(),
        },
    ]));
    app.current_view = ViewMode::SessionPicker;

    let buffer = render_app(&mut app, &backend);
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, width).contains("Second session"))
        .expect("the second session row renders");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: width / 2,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::SessionSelected(id) => assert_eq!(id, "session-2", "click selects that row"),
        other => panic!("expected SessionSelected from the click, got {other:?}"),
    }
}

/// Picker selection returns the session id, and the main-loop consumer emits LoadSession.
#[test]
fn test_session_picker_selection_emits_load_session() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let entries = vec![
        SessionEntry {
            id: "session-1".to_string(),
            name: "First session".to_string(),
            age: "just now".to_string(),
            search_text: "First session prompt".to_string(),
            preview: Vec::new(),
        },
        SessionEntry {
            id: "session-2".to_string(),
            name: "Second session".to_string(),
            age: "1h ago".to_string(),
            search_text: "Second session prompt".to_string(),
            preview: Vec::new(),
        },
    ];
    app.overlays.session_picker = Some(SessionPickerState::new(entries));
    app.current_view = ViewMode::SessionPicker;

    match route_key(&mut app, &key(KeyCode::Enter)) {
        RouteAction::SessionSelected(id) => {
            assert_eq!(id, "session-1");
            // Mirror the consumer's side effect.
            app.engine_tx.try_send(luxe_core::engine::Action::LoadSession(id)).unwrap();
        }
        other => panic!("expected SessionSelected, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(luxe_core::engine::Action::LoadSession(id)) => assert_eq!(id, "session-1"),
        other => panic!("expected LoadSession action, got {other:?}"),
    }
}

/// Chat (default) view passes keys straight through as Msg::Key.
#[test]
fn test_route_key_chat_passes_through() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;
    let action = route_key(&mut app, &key(KeyCode::Char('a')));
    assert!(
        matches!(action, RouteAction::Msg(ref message) if matches!(**message, Msg::Key(_))),
        "got {action:?}"
    );
}

/// The start screen takes text, so Esc never quits it — that is how a half-typed prompt gets lost
/// by reflex. Ctrl+C does, as it does everywhere.
#[test]
fn test_route_key_dashboard_esc_stays_and_ctrl_c_quits() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let dash = DashboardState::new(vec![SessionSummary {
        id: "s1".to_string(),
        name: "Session one".to_string(),
        age: "now".to_string(),
        search_text: "Session one prompt".to_string(),
        preview: Vec::new(),
        worktree: None,
    }]);
    app.enter_dashboard(dash);
    let action = route_key(&mut app, &key(KeyCode::Esc));
    assert!(
        matches!(action, RouteAction::Dashboard(luxe_tui::dashboard::DashOutcome::None)),
        "got {action:?}"
    );
    let quit = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let action = route_key(&mut app, &quit);
    assert!(
        matches!(action, RouteAction::Dashboard(luxe_tui::dashboard::DashOutcome::Quit)),
        "got {action:?}"
    );
}

/// The project-context list owns the keys and the pointer while it is up.
///
/// Opening it does not change the view mode — the dashboard stays behind it on purpose — and the
/// routing asked for the view mode. So the arrows walked the column *underneath* the overlay, a
/// click reached the menu through it, and Esc quit luxe instead of closing the list.
#[test]
fn the_project_context_list_takes_the_keys_from_the_dashboard_behind_it() {
    let mut app = seeded_app("nord", FixtureScenario::DashboardContextFiles).expect("the fixture");
    app.update(Msg::TerminalResized { width: 160, height: 40 });
    let backend = test_backend(160, 40);
    render_app(&mut app, &backend);

    let selected = |a: &App| a.overlays.context_files.as_ref().expect("list open").selected();
    assert_eq!(selected(&app), 0);

    for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Tab] {
        let action = route_key(&mut app, &key(code));
        match action {
            RouteAction::Msg(message) => app.update(*message),
            other => panic!("{code:?} escaped the list: {other:?}"),
        }
    }
    assert_eq!(selected(&app), 2, "j and ↓ walk the list; Tab is not its key");
    assert!(app.editor.current_input().is_empty(), "and the prompt underneath typed nothing");

    // Esc puts the list away rather than quitting luxe.
    let action = route_key(&mut app, &key(KeyCode::Esc));
    match action {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("Esc left the list: {other:?}"),
    }
    assert!(app.overlays.context_files.is_none());
    assert!(!app.should_quit);
    assert_eq!(app.current_view, ViewMode::Dashboard);

    // And a click lands on the row it is over, opening it the way Enter does.
    let mut app = seeded_app("nord", FixtureScenario::DashboardContextFiles).expect("the fixture");
    app.update(Msg::TerminalResized { width: 160, height: 40 });
    let buffer = render_app(&mut app, &backend);
    let row = (0..buffer.area.height)
        .find(|&y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("release-notes")
        })
        .expect("the fixture lists a skill");
    click_cell(&mut app, buffer.area.width / 2, row, std::time::Instant::now());
    assert_eq!(
        app.outbox.editor_file.take(),
        Some(std::path::PathBuf::from("/repo/.luxe/skills/release-notes/SKILL.md")),
        "a click opens the row it landed on"
    );
}

/// With nothing configured, setup leads the list and Enter opens the surface that answers it — and
/// the dashboard is not drawn underneath.
#[test]
fn test_route_key_dashboard_configuration_opens_the_setup_surface() {
    let mut app = create_app();
    let mut dashboard = DashboardState::new(Vec::new());
    dashboard.set_provider_configured(false);
    app.enter_dashboard(dashboard);

    let action = route_key(&mut app, &key(KeyCode::Enter));
    assert!(
        matches!(action, RouteAction::Dashboard(luxe_tui::dashboard::DashOutcome::OpenConfig)),
        "got {action:?}"
    );
    let RouteAction::Dashboard(outcome) = action else { unreachable!() };
    app.handle_dashboard_outcome(outcome);
    assert_eq!(app.current_view, ViewMode::Configuration);
    assert!(app.overlays.configuration.is_some());

    let backend = test_backend(120, 40);
    let buffer = render_app(&mut app, &backend);
    let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("Configuration"), "{screen}");
    assert!(!screen.contains("New session"), "the dashboard is gone, not behind it: {screen}");
}

/// The whole point of the surface: a login opens *in* it, so nothing is ever half-covered.
#[test]
fn a_login_started_from_configuration_draws_inside_the_surface() {
    let mut app = seeded_app("nord", FixtureScenario::Configuration).expect("the fixture");
    app.update(Msg::TerminalResized { width: 120, height: 40 });
    let backend = test_backend(120, 40);
    let text =
        |buffer: &Buffer| -> String { buffer.content.iter().map(|cell| cell.symbol()).collect() };

    // Row one: sign in to a subscription.
    match route_key(&mut app, &key(KeyCode::Enter)) {
        RouteAction::Configuration(outcome) => app.apply_configuration_outcome(outcome),
        other => panic!("expected a configuration outcome, got {other:?}"),
    }
    assert!(app.overlays.auth_wizard.is_some());

    let screen = text(&render_app(&mut app, &backend));
    assert!(screen.contains("Sign in to a subscription"), "{screen}");
    assert!(
        !screen.contains("Edit configuration files"),
        "the menu gave up its region rather than sitting behind the panel: {screen}"
    );

    // Escape returns to the menu it was chosen from, not out of the surface.
    app.update(Msg::Key(key(KeyCode::Esc)));
    assert!(app.overlays.auth_wizard.is_none());
    assert_eq!(app.current_view, ViewMode::Configuration);
    assert!(text(&render_app(&mut app, &backend)).contains("Edit configuration files"));
}

/// Choosing a model is the last step of setting a provider up, so a login that landed walks into
/// it by itself, in the same region — and it has to be *drawn*: an overlay that owns the keyboard
/// while invisible is the worst of both.
#[test]
fn a_finished_login_walks_into_the_model_picker_inside_the_surface() {
    use luxe_tui::fixture::{seed_login_wizard, LoginFixture};

    let mut app = seeded_app("nord", FixtureScenario::Configuration).expect("the fixture");
    app.update(Msg::TerminalResized { width: 120, height: 40 });
    seed_login_wizard(&mut app, LoginFixture::Done);

    assert!(app.overlays.auth_wizard.is_none(), "success is not a screen to dismiss");
    assert!(app.overlays.model_picker.is_some(), "a finished login opens the model picker itself");

    let backend = test_backend(120, 40);
    let buffer = render_app(&mut app, &backend);
    let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("Select model"), "{screen}");
    assert!(screen.contains("gpt-5.6"), "{screen}");
    // What the login had to say rides along as the picker's opening line, and with no model
    // chosen for the reader, the line asks them to choose one.
    assert!(screen.contains("Signed in to OpenAI"), "{screen}");
    assert!(screen.contains("Choose a model to finish setting up."), "{screen}");
    // The count of models is not on it: they are the rows underneath.
    assert!(!screen.contains("47 models available"), "{screen}");
}

/// Esc in the SessionPicker closes it (even without a live picker).
#[test]
fn test_route_key_session_picker_esc_closes() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::SessionPicker;
    let action = route_key(&mut app, &key(KeyCode::Esc));
    assert!(matches!(action, RouteAction::PickerClosed), "got {action:?}");
}

/// Guided Review: row and page keys move the diff window rather than the composer.
///
/// One meaning per key, in every state the surface can be in. These used to be conditional on
/// which pane held focus, which is how `↑`/`↓` came to do nothing at all on the landing surface.
#[test]
fn test_route_key_guided_review_navigation_moves_diff_rows() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width: 160, height: 44 });
    assert!(matches!(claim(&mut app, &key(KeyCode::Down)), Some(Msg::GuidedReviewLineNext)));
    assert!(matches!(claim(&mut app, &key(KeyCode::Up)), Some(Msg::GuidedReviewLinePrevious)));
    assert!(matches!(
        claim(&mut app, &KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL)),
        Some(Msg::GuidedReviewFileNext)
    ));
    assert!(matches!(
        claim(&mut app, &KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
        Some(Msg::GuidedReviewFilePrevious)
    ));
    assert!(matches!(claim(&mut app, &key(KeyCode::PageDown)), Some(Msg::GuidedReviewPageNext)));
    assert!(matches!(claim(&mut app, &key(KeyCode::PageUp)), Some(Msg::GuidedReviewPagePrevious)));
    assert!(matches!(claim(&mut app, &key(KeyCode::Char(' '))), Some(Msg::GuidedReviewHunkNext)));
    assert!(matches!(claim(&mut app, &key(KeyCode::Right)), Some(Msg::GuidedReviewColumnNext)));
    assert!(matches!(claim(&mut app, &key(KeyCode::Left)), Some(Msg::GuidedReviewColumnPrevious)));
    // Chords the keymap owns fall through to it rather than being claimed here.
    for bound in [
        key(KeyCode::Char('n')),
        key(KeyCode::Char(']')),
        key(KeyCode::Char('[')),
        key(KeyCode::Home),
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT),
    ] {
        assert!(claim(&mut app, &bound).is_none(), "{bound:?} belongs to the keymap");
    }
    // And the key the surface retired answers with nothing rather than something surprising: Tab
    // used to move focus, and a focus move that quietly became a pane move would be worse than a
    // key that does nothing.
    assert!(
        matches!(claim(&mut app, &key(KeyCode::Tab)), Some(Msg::Tick)),
        "Tab is swallowed, never leaked to the composer"
    );
}

/// The Guided Review surface is borderless (it reads as the same canvas as chat), while its
/// overlays keep the shared rounded panel corners (`PANEL_BORDER_TYPE`).
#[test]
fn guided_review_surface_is_borderless_and_overlays_use_rounded_corners() {
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    // Borderless surface: no outer frame corners anywhere (only the thin rail divider remains).
    let buffer = render_app(&mut app, &backend);
    let any_frame_corner = (0..width)
        .flat_map(|x| (0..height).map(move |y| (x, y)))
        .any(|(x, y)| matches!(buffer[(x, y)].symbol(), "╭" | "╮" | "╰" | "╯"));
    assert!(!any_frame_corner, "the review surface must be borderless");

    // Opening the help overlay adds an interior rounded panel — proves overlays are rounded too.
    app.update(Msg::GuidedReviewToggleHelp);
    let with_help = render_app(&mut app, &backend);
    let interior_rounded_corner = (1..width - 1)
        .flat_map(|x| (1..height - 1).map(move |y| (x, y)))
        .any(|(x, y)| with_help[(x, y)].symbol() == "╭");
    assert!(interior_rounded_corner, "help overlay must render a rounded top-left corner");
    let any_square_corner = (0..width)
        .flat_map(|x| (0..height).map(move |y| (x, y)))
        .any(|(x, y)| matches!(with_help[(x, y)].symbol(), "┌" | "┐" | "└" | "┘"));
    assert!(!any_square_corner, "no Guided Review panel may use square corners");
}

/// Region-precise wheel: after a render publishes the hit regions, a notch over the guide scrolls
/// the guide's own prose, and over the diff scrolls source lines.
///
/// The guide's wheel used to walk the file selection, which made reading the section you were not
/// standing in impossible — every notch dragged the review's position along with the view.
#[test]
fn wheel_over_review_panes_scrolls_the_surface_under_the_cursor() {
    use crossterm::event::{MouseEvent, MouseEventKind};
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let _ = render_app(&mut app, &backend); // publishes app.hit scroll regions

    let wheel = |kind, column| MouseEvent { kind, column, row: 6, modifiers: KeyModifiers::NONE };
    // Left rail (guide pane) -> the prose scrolls; the cursor is not what moves.
    assert!(matches!(
        route_mouse(&mut app, &wheel(MouseEventKind::ScrollDown, 5)),
        RouteAction::Msg(message) if matches!(*message, Msg::GuidedReviewScrollGuide { up: false })
    ));
    // Right pane (diff) -> line scroll.
    assert!(matches!(
        route_mouse(&mut app, &wheel(MouseEventKind::ScrollUp, 90)),
        RouteAction::Msg(message) if matches!(*message, Msg::GuidedReviewLinePrevious)
    ));
}
