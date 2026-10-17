//! Dashboard — the start screen, setup and the configuration surface.

use super::*;

/// Dashboard: entering dashboard mode sets `current_view` and clears the editor.
#[test]
fn test_enter_dashboard() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);

    // Type something into the editor.
    app.editor.insert_text("hello");

    let dashboard = crate::dashboard::DashboardState::new(vec![]);
    app.enter_dashboard(dashboard);

    assert!(matches!(app.current_view, ViewMode::Dashboard));
    assert!(app.dashboard.is_some());
    assert!(app.editor.current_input().is_empty(), "editor should be cleared on dashboard entry");
}

/// Configuration opens as its own surface and hands the dashboard back on the way out — with the
/// menu rebuilt against what is configured *now*, so a provider set up here stops the nagging.
#[test]
fn configuration_replaces_the_dashboard_and_returns_it_updated() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let mut dashboard = crate::dashboard::DashboardState::new(vec![]);
    dashboard.set_provider_configured(false);
    app.enter_dashboard(dashboard);

    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::OpenConfig);
    assert!(matches!(app.current_view, ViewMode::Configuration));
    assert!(app.overlays.configuration.is_some());
    assert!(app.dashboard.is_some(), "the briefing is kept, just not drawn");

    app.close_configuration();
    assert!(matches!(app.current_view, ViewMode::Dashboard));
    assert!(app.overlays.configuration.is_none());
    // No auth service in this harness, so "configured" is the honest answer: nothing to nag about.
    assert!(app
        .dashboard
        .as_ref()
        .is_some_and(crate::dashboard::DashboardState::provider_configured));
}

/// Mid-session there is no dashboard to go back to, so `/config` returns to the conversation.
#[test]
fn configuration_from_a_session_returns_to_the_conversation() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.open_configuration();
    assert!(matches!(app.current_view, ViewMode::Configuration));
    app.close_configuration();
    assert!(matches!(app.current_view, ViewMode::Chat));
}

/// Dashboard: Quit outcome sets should_quit.
#[test]
fn test_dashboard_quit_outcome() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.enter_dashboard(crate::dashboard::DashboardState::new(vec![]));

    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::Quit);
    assert!(app.should_quit);
}

/// Dashboard: NewSession outcome switches to Chat, clears the transcript, and actually asks
/// the engine for a fresh session (so the sidebar doesn't keep a prior session's title).
#[test]
fn test_dashboard_new_session_outcome() {
    let (tx, mut rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.session_id = Some("prior-session".into());
    app.enter_dashboard(crate::dashboard::DashboardState::new(vec![]));
    // Push a fake conversation block.
    app.push_convo(ConvoBlock::User { text: "hi".into(), attached: vec![], chips: Vec::new() });

    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::NewSession);
    assert!(matches!(app.current_view, ViewMode::Chat));
    assert!(app.dashboard.is_none());
    assert!(app.convo.is_empty(), "conversation should be cleared");
    assert!(
        app.session_id.is_none(),
        "active session id should be dropped until the new one loads"
    );
    assert!(
        matches!(rx.try_recv(), Ok(luxe_core::engine::Action::NewSession)),
        "dashboard new session must ask the engine for a fresh session"
    );
}

/// A startup notice survives the dashboard, and arrives in the conversation the reader ends up in.
///
/// The defect this pins: every row on the dashboard loads a session, and a load replaces the
/// transcript. Notices were pushed on the *first* `SessionLoaded`, which happens while the
/// dashboard is still up — so a theme that did not load, a `--no-tool` that disabled nothing and a
/// project root that is not the checkout were all wiped unread in the default launch.
#[test]
fn a_startup_notice_survives_the_dashboard_and_the_session_load_after_it() {
    let (tx, _rx) = mpsc::channel(100);
    let keymap = luxe_core::keymap::KeyMap::defaults();
    let config = luxe_core::config::Config::default();
    let mut app = App::new(tx, keymap, config);
    app.enter_dashboard(crate::dashboard::DashboardState::new(vec![]));

    // The engine says its piece while the reader is still on the menu.
    app.update(Msg::Engine(Event::Notice("--no-tool websearch: no tool by that name".into())));
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s1".into(), messages: vec![] }));
    assert!(app.convo.is_empty(), "nothing is drawn under the dashboard");

    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::NewSession);
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s2".into(), messages: vec![] }));

    let shown: Vec<&str> = app
        .convo
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["--no-tool websearch: no tool by that name"]);

    // And only once: a later session switch is not an occasion to repeat startup.
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s3".into(), messages: vec![] }));
    assert!(app.convo.is_empty(), "a reload replays the session, not the launch");
}

#[test]
fn dashboard_session_action_opens_local_picker() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    let session = crate::dashboard::SessionSummary {
        id: "session-1".into(),
        name: "Fix the command palette".into(),
        age: "just now".into(),
        search_text: "Fix the command palette navigation".into(),
        preview: Vec::new(),
        worktree: None,
    };
    app.enter_dashboard(
        crate::dashboard::DashboardState::new(vec![session.clone()]).with_sessions(vec![session]),
    );

    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::OpenPicker(
        crate::dashboard::PickerKind::FindSession,
    ));

    assert_eq!(app.current_view, ViewMode::SessionPicker);
    assert_eq!(app.overlays.session_picker.as_ref().map(|picker| picker.match_count()), Some(1));
    assert!(app.dashboard.is_some(), "dashboard should remain behind the picker");
    assert!(rx.try_recv().is_err(), "picker opening must not be delegated to the engine");
}

/// `/settings` opens the pages; changing a setting writes that one key to the reader's file,
/// applies it now where it can, and says so where the reader is looking.
#[tokio::test]
async fn a_setting_changed_in_the_surface_is_written_and_applied() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));

    let home = tempfile::tempdir().expect("temp dir");
    let _config_home = crate::test_env::TempConfigHome::at(home.path());
    let path = luxe_core::config::Config::global_path();
    std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
    std::fs::write(&path, "{\n  \"theme\": \"luxe\",\n  \"from_a_newer_build\": 1\n}\n")
        .expect("seed");

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Default::default());
    app.project_root = Some(home.path().to_path_buf());
    app.open_settings();
    assert_eq!(app.current_view, ViewMode::Configuration);
    let settings = app.overlays.configuration.as_ref().and_then(|c| c.settings()).expect("open");
    assert_eq!(settings.page(), luxe_core::settings::Page::Appearance);

    // Dock & statusline is the third page; the mouse is its last row
    app.update(key(KeyCode::Char('3')));
    for _ in 0..3 {
        app.update(key(KeyCode::Down));
    }
    assert!(app.mouse_enabled, "on by default");
    app.update(key(KeyCode::Enter));

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(written["mouse"], false, "the one key changed");
    assert_eq!(written["from_a_newer_build"], 1, "and nothing else did");
    assert!(!app.mouse_enabled, "applied without a restart");
    let canvas = ratatui::backend::TestBackend::new(150, 40);
    let mut terminal = ratatui::Terminal::new(canvas).expect("terminal");
    terminal.draw(|f| crate::view::render(f, &mut app)).expect("draw");
    let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("Saved to"), "the write is acknowledged on the surface");

    // Esc leaves the pages for the menu, and a second Esc leaves the surface
    app.update(key(KeyCode::Esc));
    assert!(!app.overlays.configuration.as_ref().expect("still open").showing_settings());
}

/// The theme row hands over to the live picker, drawn over the surface, which takes the keys
/// until it closes.
#[tokio::test]
async fn the_theme_setting_opens_the_picker_over_the_surface() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let home = tempfile::tempdir().expect("temp dir");
    let _config_home = crate::test_env::TempConfigHome::at(home.path());

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Default::default());
    app.project_root = Some(home.path().to_path_buf());
    app.open_settings();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.overlays.theme_picker.is_some(), "the theme row opens the picker");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
    assert!(app.overlays.theme_picker.is_some(), "and the picker, not the pages, took the arrow");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.overlays.theme_picker.is_none());
    assert!(app.overlays.configuration.as_ref().expect("open").showing_settings(), "pages");
}

/// A prompt typed on the start screen asks for a new session and is sent once it has loaded —
/// through the composer, so it shows in the transcript like anything typed in a session.
#[test]
fn a_prompt_typed_on_the_start_screen_is_sent_in_the_new_session() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.status.model = "qwen3.8-27b".into();
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    for character in "summarize the parser".chars() {
        let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
        let dashboard = app.dashboard.as_mut().expect("up");
        let outcome = dashboard.on_key(&key, &app.keymap, &mut app.editor);
        assert_eq!(outcome, crate::dashboard::DashOutcome::None);
    }
    let dashboard = app.dashboard.as_mut().expect("up");
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let outcome = dashboard.on_key(&enter, &app.keymap, &mut app.editor);
    app.handle_dashboard_outcome(outcome);

    assert_eq!(app.current_view, ViewMode::Chat);
    assert!(matches!(rx.try_recv(), Ok(luxe_core::engine::Action::NewSession)));
    assert!(rx.try_recv().is_err(), "nothing is sent before the session exists");
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s1".into(), messages: vec![] }));
    match rx.try_recv() {
        Ok(luxe_core::engine::Action::SubmitUserMessage { content, .. }) => {
            assert_eq!(content, "summarize the parser")
        }
        other => panic!("the draft was not sent: {other:?}"),
    }
    assert!(app.editor.current_input().is_empty(), "and it left the composer");
}

/// With no model chosen, asking opens the picker over the start screen and keeps the draft — the
/// same courtesy the composer extends in a session.
#[test]
fn asking_with_no_model_keeps_the_draft_and_opens_the_picker() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.model_catalog = vec![crate::model_picker::ModelEntry {
        info: luxe_core::provider::ModelInfo {
            r#ref: luxe_core::provider::ModelRef::new("lmstudio", "qwen3.8-27b"),
            ..Default::default()
        },
        label: "qwen3.8-27b".into(),
        configured: true,
        unverified: false,
    }];
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    app.editor.insert_text("a question");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::Ask);
    assert_eq!(app.current_view, ViewMode::Dashboard, "still on the start screen");
    assert!(app.overlays.model_picker.is_some(), "choosing a model comes first");
    assert_eq!(app.editor.current_input(), "a question", "with the draft kept");
    assert!(rx.try_recv().is_err(), "and nothing sent");
}

/// Applies what routing a key or a click on the start screen left for the app, as the loop does.
fn apply_on_start_screen(app: &mut App, action: crate::main_loop::RouteAction) {
    match action {
        crate::main_loop::RouteAction::Dashboard(outcome) => app.handle_dashboard_outcome(outcome),
        crate::main_loop::RouteAction::Msg(message) => app.update(*message),
        other => panic!("unexpected route from the start screen: {other:?}"),
    }
}

/// Shift+Tab on the start screen switches the mode the session will start in, as it does in a
/// session, and the launcher's keys say so, naming the mode it switches to.
#[test]
fn shift_tab_on_the_start_screen_switches_the_mode() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    in_mode(&mut app, "build");
    let (text, _) = screen(&mut app, 100, 30);
    assert!(text.contains("shift+tab plan mode"), "the keys name the switch:\n{text}");

    let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    let routed = crate::main_loop::route_key(&mut app, &back_tab);
    apply_on_start_screen(&mut app, routed);
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "plan"));
    assert_eq!(app.current_view, ViewMode::Dashboard, "still on the start screen");

    in_mode(&mut app, "plan");
    let (text, _) = screen(&mut app, 100, 30);
    assert!(text.contains("shift+tab build mode"), "and back again:\n{text}");
}

/// The composer's title is the mode switch on the start screen too, since it is the same composer.
#[test]
fn the_start_screen_composer_title_switches_the_mode() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    in_mode(&mut app, "build");
    let (text, _) = screen(&mut app, 100, 30);
    let row = text.lines().position(|line| line.contains("build ─")).expect("a titled composer");
    let line = text.lines().nth(row).expect("the row");
    let column = line[..line.find("build").expect("the title")].chars().count() as u16;
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    };
    let routed = crate::main_loop::route_mouse(&mut app, &click);
    apply_on_start_screen(&mut app, routed);
    assert!(matches!(rx.try_recv(), Ok(Action::SetMode(mode)) if mode == "plan"));
    assert_eq!(app.current_view, ViewMode::Dashboard);
}

/// A command the start screen cannot host gets a session first, then runs in it.
#[test]
fn a_command_found_on_the_start_screen_runs_in_a_new_session() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    let registry = crate::commands::CommandRegistry::all();
    let review = registry.parse_slash("/review the parser").expect("the review command");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::RunCommand(review));
    assert_eq!(app.current_view, ViewMode::Chat);
    let actions: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(matches!(actions.first(), Some(luxe_core::engine::Action::NewSession)), "{actions:?}");
    assert!(
        actions.iter().any(|action| matches!(action, Action::RequestReview { .. })),
        "then the command ran: {actions:?}"
    );

    // One the start screen can host stays on it
    let mut app = App::new(mpsc::channel(8).0, KeyMap::defaults(), Default::default());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    let theme = registry.parse_slash("/theme").expect("the theme command");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::RunCommand(theme));
    assert_eq!(app.current_view, ViewMode::Dashboard);
    assert!(app.overlays.theme_picker.is_some());
}

/// `/mode` on the start screen switches the mode the session will start in, as Shift+Tab does
/// there, rather than starting a session to switch it in.
#[test]
fn the_mode_command_on_the_start_screen_stays_on_it() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    let registry = crate::commands::CommandRegistry::all();
    let mode = registry.parse_slash("/mode plan").expect("the mode command");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::RunCommand(mode));
    assert_eq!(app.current_view, ViewMode::Dashboard);
    let actions: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(matches!(actions.as_slice(), [Action::SetMode(mode)] if mode == "plan"), "{actions:?}");
}

/// Setup's answers reach the reader's file one key at a time, and skipping writes a file that
/// chooses nothing, so the first run asks once.
#[tokio::test]
async fn finishing_setup_writes_the_answers_and_skipping_writes_nothing_but_the_file() {
    let home = tempfile::tempdir().expect("temp dir");
    let _config_home = crate::test_env::TempConfigHome::at(home.path());
    let path = luxe_core::config::Config::global_path();

    let mut app = App::new(mpsc::channel(8).0, KeyMap::defaults(), Default::default());
    app.project_root = Some(home.path().to_path_buf());
    app.enter_dashboard(crate::dashboard::DashboardState::new(Vec::new()));
    app.open_setup(crate::configuration::SetupMode::FirstRun);
    assert!(app.setup_showing());
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
    assert!(!app.setup_showing(), "skipped");
    assert_eq!(app.current_view, ViewMode::Dashboard, "onto the start screen");
    assert_eq!(
        std::fs::read_to_string(&path).expect("written"),
        "{\n  \"$schema\": \"./config.schema.json\"\n}\n"
    );

    std::fs::remove_file(&path).expect("start over");
    app.open_setup(crate::configuration::SetupMode::Init);
    app.apply_configuration_outcome(crate::configuration::ConfigOutcome::FinishSetup(
        crate::configuration::SetupChoices {
            theme: Some("nord".into()),
            disabled_tools: vec!["debug".into()],
            start: crate::configuration::StartScreen::Minimal,
        },
    ));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(written["theme"], "nord");
    assert_eq!(written["tools"]["disabled"], serde_json::json!(["debug"]));
    assert_eq!(written["dashboard"], serde_json::json!({ "style": "minimal" }));
    assert!(app.should_quit, "under luxe init, finishing leaves");
}

fn summary(id: &str, name: &str) -> crate::dashboard::SessionSummary {
    summary_in(id, name, None)
}

fn summary_in(id: &str, name: &str, worktree: Option<&str>) -> crate::dashboard::SessionSummary {
    crate::dashboard::SessionSummary {
        id: id.into(),
        name: name.into(),
        age: "2d ago".into(),
        search_text: name.into(),
        preview: Vec::new(),
        worktree: worktree.map(str::to_string),
    }
}

/// The Sessions row names the command that opens it, the way New session names `/new`, and
/// nothing else: a count of this project's sessions told the reader nothing to act on.
#[test]
fn the_sessions_row_names_its_command() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    let sessions = vec![summary("a", "Fix the parser"), summary("b", "Tidy the dock")];
    app.enter_dashboard(
        crate::dashboard::DashboardState::new(sessions.clone()).with_sessions(sessions),
    );
    let (text, _) = screen(&mut app, 100, 30);
    let row = text.lines().find(|line| line.contains("Sessions…")).expect("the Sessions row");
    assert!(row.contains("/sessions"), "{row:?}");
    assert!(!row.contains("in this project"), "{row:?}");
}

/// `/sessions` on the start screen opens the same picker its Sessions row does, over it.
#[test]
fn the_sessions_command_on_the_start_screen_opens_its_picker() {
    let (tx, mut rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    let sessions = vec![summary("a", "Fix the parser"), summary("b", "Tidy the dock")];
    app.enter_dashboard(
        crate::dashboard::DashboardState::new(sessions.clone()).with_sessions(sessions),
    );
    let command = crate::commands::CommandRegistry::all().parse_slash("/sessions").expect("it");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::RunCommand(command));
    assert_eq!(app.current_view, ViewMode::SessionPicker);
    let picker = app.overlays.session_picker.as_ref().expect("the picker");
    assert_eq!(picker.match_count(), 2, "every session the start screen knows");
    assert!(app.dashboard.is_some(), "over the start screen, which esc returns to");
    assert!(rx.try_recv().is_err(), "and no session started for it");
}

/// A start screen whose sessions are `w`, in the `topic` worktree, and `m`, in the main checkout,
/// for a run working in `here`.
fn start_screen_with_worktrees(here: Option<&str>) -> (App, mpsc::Receiver<Action>) {
    let (tx, rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Default::default());
    let sessions = vec![
        summary_in("w", "Fix the loop", Some("topic")),
        summary_in("m", "Tidy the dock", None),
    ];
    let dashboard = crate::dashboard::DashboardState::new(sessions.clone())
        .with_sessions(sessions)
        .with_worktree(here.map(str::to_string));
    app.enter_dashboard(dashboard);
    (app, rx)
}

/// Resuming a session that works in another worktree goes through the CLI, which moves the process
/// there first, carrying the draft. Loaded in place it ran in the checkout the start
/// screen stood in, which is not the one its work is in.
#[test]
fn a_session_in_another_worktree_is_resumed_through_the_cli() {
    let (mut app, mut rx) = start_screen_with_worktrees(None);
    app.editor.insert_text("half a thought");
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::Resume("w".into()));
    let carried = app.outbox.resume.as_ref().expect("left through the CLI");
    assert_eq!((carried.session.as_str(), carried.draft.as_str()), ("w", "half a thought"));
    assert!(rx.try_recv().is_err(), "nothing is loaded where its work is not");

    let (mut app, mut rx) = start_screen_with_worktrees(None);
    app.handle_dashboard_outcome(crate::dashboard::DashOutcome::Resume("m".into()));
    assert!(app.outbox.resume.is_none());
    assert!(matches!(rx.try_recv(), Ok(Action::LoadSession(id)) if id == "m"), "in place");
}

/// A run standing in a worktree leaves it for a session in the main checkout, and the session
/// picker routes the same way the rows do.
#[test]
fn the_session_picker_routes_by_worktree_too() {
    let (mut app, mut rx) = start_screen_with_worktrees(Some("topic"));
    app.open_picked_session("m".into());
    assert_eq!(app.outbox.resume.as_ref().map(|launch| launch.session.as_str()), Some("m"));
    assert!(rx.try_recv().is_err());

    let (mut app, mut rx) = start_screen_with_worktrees(Some("topic"));
    app.open_picked_session("w".into());
    assert!(app.outbox.resume.is_none());
    assert!(matches!(rx.try_recv(), Ok(Action::LoadSession(id)) if id == "w"), "same worktree");
}
