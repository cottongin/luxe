//! Input that reaches the loop: what counts as a key press.

use super::keys::apply_route_action;
use super::*;
use crate::fixture::{seeded_app, FixtureScenario};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

/// A key's release is not another press of it.
///
/// The break this catches: every key event was routed whatever its kind, and the Windows console
/// sends a release after each press, so `a` typed `aa` and one Esc closed two layers.
#[test]
fn a_key_release_does_nothing() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    let mut exit = TuiExit::Quit;
    for kind in [KeyEventKind::Press, KeyEventKind::Release] {
        let key = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            kind,
            KeyEventState::NONE,
        );
        apply_terminal_event(&mut app, Event::Key(key), &mut exit);
    }
    assert_eq!(app.editor.current_input(), "a");
}

/// Picking a session in the picker leaves the start screen for it, at rest, with the engine asked
/// to load the one picked.
#[test]
fn a_picked_session_is_loaded_from_an_empty_conversation() {
    let mut app = seeded_app("dark", FixtureScenario::SessionPicker).expect("fixture");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.engine_tx = tx;
    let mut exit = TuiExit::Quit;

    apply_route_action(&mut app, RouteAction::SessionSelected("s-picked".into()), &mut exit);

    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    assert!(
        app.overlays.session_picker.is_none() && app.dashboard.is_none() && app.convo.is_empty()
    );
    assert!(matches!(app.status.state, crate::app::AppState::Idle));
    let asked = rx.try_recv().expect("the engine was asked");
    assert!(matches!(asked, luxe_core::engine::Action::LoadSession(id) if id == "s-picked"));
    assert_eq!(exit, TuiExit::Quit, "the start screen loads it in place");
}

/// Picking a session mid-session leaves this one for it through the CLI, the way `/github`'s
/// resume does: the engine and the workspace services belong to this session's checkout, and the
/// one picked may live in a worktree of its own.
#[test]
fn a_session_picked_mid_session_is_left_for_through_the_cli() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.engine_tx = tx;
    app.session_catalog.source = Some(std::sync::Arc::new(|| Box::pin(async { Vec::new() })));
    open_filled_picker(&mut app);
    app.editor.insert_text("half a thought");
    let mut exit = TuiExit::Quit;

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    apply_terminal_event(&mut app, Event::Key(enter), &mut exit);

    let TuiExit::ResumeSession(carried) = exit else { panic!("left through the CLI: {exit:?}") };
    assert_eq!(carried.session, "s-older");
    assert_eq!(carried.draft, "half a thought", "with the draft in hand");
    assert!(app.should_quit, "the loop hands it to the CLI");
    assert!(app.overlays.session_picker.is_none());
    assert!(rx.try_recv().is_err(), "nothing is loaded in place");
}

/// Typing in the session picker searches it and nothing else: no key it takes, and no plain key
/// it ignores, reaches the composer hidden behind it.
///
/// The break this catches: the picker answered "nothing chosen" for every key it handled, the
/// router read that as "not mine", and each letter of a search was typed into the composer too.
#[test]
fn the_session_picker_keeps_its_keys() {
    let mut app = seeded_app("dark", FixtureScenario::SessionPicker).expect("fixture");
    assert_eq!(app.current_view, crate::app::ViewMode::SessionPicker);
    let mut exit = TuiExit::Quit;
    for code in [KeyCode::Char('n'), KeyCode::Char('A'), KeyCode::Down, KeyCode::Tab] {
        let modifiers = match code {
            KeyCode::Char('A') => KeyModifiers::SHIFT,
            _ => KeyModifiers::NONE,
        };
        apply_terminal_event(&mut app, Event::Key(KeyEvent::new(code, modifiers)), &mut exit);
    }
    assert_eq!(app.editor.current_input(), "", "the composer behind it got nothing");
    let picker = app.overlays.session_picker.as_ref().expect("still open");
    assert_eq!(picker.query(), "nA", "and the search got the letters, capitals included");
}

/// A turn that started while the picker was open holds the switch, since leaving would stop it. The
/// picker stays, so the reader can pick again once it ends.
#[test]
fn a_running_turn_holds_a_mid_session_pick() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    app.session_catalog.source = Some(std::sync::Arc::new(|| Box::pin(async { Vec::new() })));
    open_filled_picker(&mut app);
    app.status.state = crate::app::AppState::Streaming;
    let mut exit = TuiExit::Quit;

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    apply_terminal_event(&mut app, Event::Key(enter), &mut exit);

    assert_eq!(exit, TuiExit::Quit);
    assert!(!app.should_quit);
    assert!(app.overlays.session_picker.is_some(), "still open to pick again");
    assert!(app.status.toast.is_some(), "it says why");
}

/// `/sessions` mid-session, with the catalog it asked for landed: one other session, `s-older`.
fn open_filled_picker(app: &mut crate::app::App) {
    app.open_sessions();
    let request = app.outbox.session_catalog.take().expect("a read");
    let catalog = vec![crate::dashboard::SessionSummary {
        id: "s-older".into(),
        name: "Rewrite the parser".into(),
        age: "2d ago".into(),
        search_text: String::new(),
        preview: Vec::new(),
        worktree: None,
    }];
    app.update(crate::app::Msg::SessionsListed { request, catalog });
}

/// A session the CLI resumed opens with the draft the reader carried out of the last surface,
/// images and all.
#[test]
fn a_resumed_session_opens_with_the_carried_draft() {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let mut app = crate::app::App::new(
        tx,
        luxe_core::keymap::KeyMap::defaults(),
        luxe_core::config::Config::default(),
    );
    let image = crate::editor::AttachedImage {
        data: "aGk=".into(),
        mime: "image/png".into(),
        filename: "pasted.png".into(),
        width: 1,
        height: 1,
    };
    let launch = ResumeLaunch {
        session: "s-older".into(),
        draft: "half a thought".into(),
        images: vec![image],
    };

    let _auth = open_first_surface(&mut app, Opening::Resumed(launch), None, true);

    assert!(app.editor.current_input().starts_with("half a thought"), "the draft");
    assert!(app.editor.has_images(), "with its image");
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
}
