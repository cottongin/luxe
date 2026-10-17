//! Sessions — `/sessions` mid-session: the picker over the conversation, filled off the frame.

use super::*;
use crate::dashboard::{SessionCatalogSource, SessionSummary};

fn summary(id: &str, name: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        name: name.into(),
        age: "2d ago".into(),
        search_text: name.into(),
        preview: Vec::new(),
        worktree: None,
    }
}

/// A conversation on screen, in session `current`, whose CLI can list the project's sessions.
fn in_a_session() -> App {
    let (tx, _rx) = mpsc::channel(16);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.dashboard = None;
    app.current_view = ViewMode::Chat;
    app.session_id = Some("current".into());
    app.status.model = "model-underneath".into();
    let source: SessionCatalogSource = std::sync::Arc::new(|| Box::pin(async { Vec::new() }));
    app.session_catalog.source = Some(source);
    app
}

fn run(app: &mut App, slash: &str) {
    let command = crate::commands::CommandRegistry::all().parse_slash(slash).expect("a command");
    super::super::commands::dispatch(app, command);
}

fn press(app: &mut App, code: KeyCode) {
    let key = KeyEvent::new(code, KeyModifiers::NONE);
    match crate::main_loop::route_key(app, &key) {
        crate::main_loop::RouteAction::Msg(message) => app.update(*message),
        other => panic!("unexpected route from the picker: {other:?}"),
    }
}

/// `/sessions` opens the picker over the conversation at once and leaves the reading to the loop,
/// since it walks every recent session's log. What lands fills it, without the session already on
/// screen, and a search typed while it was reading still applies.
#[test]
fn the_sessions_command_opens_a_picker_the_catalog_fills_when_it_lands() {
    let mut app = in_a_session();
    run(&mut app, "/sessions");
    assert_eq!(app.current_view, ViewMode::SessionPicker);
    let request = app.outbox.session_catalog.expect("the read is parked for the loop");
    let (text, _) = screen(&mut app, 100, 30);
    assert!(text.contains("Reading sessions"), "it says it is reading:\n{text}");
    assert!(text.contains("model-underneath"), "over the conversation, not instead:\n{text}");

    for character in "pars".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    let catalog = vec![
        summary("current", "Fix the parser"),
        summary("older", "Rewrite the parser"),
        summary("other", "Tidy the dock"),
    ];
    app.update(Msg::SessionsListed { request, catalog });
    let picker = app.overlays.session_picker.as_ref().expect("still open");
    assert_eq!(picker.query(), "pars", "the search typed meanwhile");
    assert_eq!(picker.match_count(), 1, "the session on screen is not one to switch to");
    assert_eq!(picker.selected_session().map(|session| session.id.as_str()), Some("older"));
    assert_eq!(editor_text(&app), "", "and the composer behind it got nothing");
}

/// A catalog that lands after the picker closed opens nothing.
#[test]
fn a_catalog_that_lands_after_the_picker_closed_is_dropped() {
    let mut app = in_a_session();
    run(&mut app, "/sessions");
    let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(
        crate::main_loop::route_key(&mut app, &key),
        crate::main_loop::RouteAction::PickerClosed
    ));
    app.overlays.session_picker = None;
    app.current_view = ViewMode::Chat;
    let catalog = vec![summary("older", "Rewrite the parser")];
    app.update(Msg::SessionsListed { request: 1, catalog });
    assert!(app.overlays.session_picker.is_none());
    assert_eq!(app.current_view, ViewMode::Chat);
}

/// With nothing to list sessions from, `/sessions` says so rather than opening an empty picker.
#[test]
fn with_no_catalog_the_sessions_command_says_so() {
    let mut app = in_a_session();
    app.session_catalog.source = None;
    run(&mut app, "/sessions");
    assert_eq!(app.current_view, ViewMode::Chat);
    assert!(app.overlays.session_picker.is_none());
    assert!(app.status.toast.is_some(), "it says why nothing opened");
}

/// A scan that lands after its picker closed and another opened is not the one the new picker
/// waits for: filling from it would show a stale list and then drop the fresh one.
#[test]
fn a_scan_for_an_earlier_picker_does_not_fill_a_reopened_one() {
    let mut app = in_a_session();
    run(&mut app, "/sessions");
    let first = app.outbox.session_catalog.take().expect("the first read");
    app.overlays.session_picker = None;
    app.current_view = ViewMode::Chat;
    run(&mut app, "/sessions");
    let second = app.outbox.session_catalog.take().expect("the second read");
    assert_ne!(first, second);

    let stale = vec![summary("old", "Stale list")];
    app.update(Msg::SessionsListed { request: first, catalog: stale });
    let picker = app.overlays.session_picker.as_ref().expect("open");
    assert!(picker.is_reading(), "still waiting for its own read");
    app.update(Msg::SessionsListed { request: second, catalog: vec![summary("new", "Fresh")] });
    let picker = app.overlays.session_picker.as_ref().expect("open");
    assert_eq!(picker.selected_session().map(|session| session.id.as_str()), Some("new"));
}

/// Leaving for another session mid-turn would stop the turn, so `/sessions` waits for it to end
/// or be stopped.
#[test]
fn the_sessions_command_waits_for_a_running_turn() {
    let mut app = in_a_session();
    app.status.state = crate::app::AppState::Streaming;
    run(&mut app, "/sessions");
    assert!(app.overlays.session_picker.is_none());
    assert_eq!(app.current_view, ViewMode::Chat);
    assert!(app.status.toast.is_some(), "it says why");
}

/// The picker is modal over the conversation: nothing under it answers the pointer, so a drag in
/// its preview cannot select the transcript hidden behind it.
#[test]
fn nothing_under_the_mid_session_picker_answers_the_pointer() {
    let mut app = in_a_session();
    app.push_convo(ConvoBlock::Assistant("hidden words ".repeat(400)));
    run(&mut app, "/sessions");
    let request = app.outbox.session_catalog.take().expect("a read");
    app.update(Msg::SessionsListed { request, catalog: vec![summary("older", "Rewrite it")] });
    screen(&mut app, 100, 30);
    let hit = app.hit.borrow();
    assert!(
        hit.clicks().all(|(_, target)| matches!(target, crate::mouse::MouseTarget::SessionRow(_))),
        "only the picker's rows are clickable"
    );
    for (x, y) in [(50, 15), (2, 28), (98, 2)] {
        assert!(hit.text_at(x, y).is_none(), "no text to select at {x},{y}");
        assert!(hit.scroll_at(x, y).is_none(), "nothing to scroll at {x},{y}");
    }
}

fn editor_text(app: &App) -> String {
    app.editor.current_input().to_string()
}
