//! Chat keys — arrows, paging and the scroll state under them.

use super::*;

/// Plain arrows belong to the composer; modified arrows remain conversation bindings.
#[test]
fn test_route_key_chat_arrows_edit_the_composer() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;
    app.editor.insert_text("alpha\nbeta");

    match route_key(&mut app, &key(KeyCode::Up)) {
        RouteAction::Msg(message) => app.update(*message),
        action => panic!("plain Up did not reach the editor: {action:?}"),
    }
    assert_eq!(app.editor.visual_cursor(80).row, 0);

    let alt_down = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);
    assert!(matches!(
        route_key(&mut app, &alt_down),
        RouteAction::Msg(message) if matches!(*message, Msg::Key(_))
    ));
}

#[test]
fn open_palette_receives_arrow_keys_before_chat_scrolling() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.overlays.palette = Some(luxe_tui::palette::PaletteState::open(
        luxe_tui::commands::CommandRegistry::all().arrange(
            &luxe_tui::commands::CommandContext::default(),
            &KeyMap::defaults(),
            &[],
        ),
    ));

    match route_key(&mut app, &key(KeyCode::Down)) {
        RouteAction::Msg(message) => app.update(*message),
        action => panic!("palette arrow leaked to chat routing: {action:?}"),
    }

    assert_eq!(app.overlays.palette.as_ref().map(|state| state.selected()), Some(1));
    assert_eq!(app.scroll_state.offset, 0);
}

#[test]
fn test_route_key_chat_inserts_j_and_k() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;

    for character in ['j', 'k'] {
        match route_key(&mut app, &key(KeyCode::Char(character))) {
            RouteAction::Msg(msg) => app.update(*msg),
            action => panic!("expected editor input for {character:?}, got {action:?}"),
        }
    }

    assert_eq!(app.editor.current_input(), "jk");
}

/// Page keys scroll the conversation without reserving printable editor keys.
#[test]
fn test_route_key_chat_page_scroll() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;

    assert!(matches!(
        route_key(&mut app, &key(KeyCode::PageUp)),
        RouteAction::Scroll(luxe_tui::main_loop::ScrollAction::PageUp)
    ));
    assert!(matches!(
        route_key(&mut app, &key(KeyCode::PageDown)),
        RouteAction::Scroll(luxe_tui::main_loop::ScrollAction::PageDown)
    ));

    for character in ['b', 'f', 'G', 'y'] {
        match route_key(&mut app, &key(KeyCode::Char(character))) {
            RouteAction::Msg(message) => app.update(*message),
            action => panic!("printable {character:?} was intercepted: {action:?}"),
        }
    }
    assert_eq!(app.editor.current_input(), "bfGy");
}

#[test]
fn standard_paste_bindings_request_a_unified_clipboard_read() {
    let mut app = create_app();
    for paste_key in [
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Insert, KeyModifiers::SHIFT),
    ] {
        app.outbox.clipboard_paste = false;
        match route_key(&mut app, &paste_key) {
            RouteAction::Msg(message) => app.update(*message),
            action => panic!("standard paste did not reach App: {action:?}"),
        }
        assert!(app.outbox.clipboard_paste);
    }
}

#[test]
fn test_route_key_chat_ctrl_end_scrolls_to_bottom() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;
    let ctrl_end = KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL);

    assert!(matches!(
        route_key(&mut app, &ctrl_end),
        RouteAction::Scroll(luxe_tui::main_loop::ScrollAction::ToBottom)
    ));
}

/// Chat view: non-scroll keys still pass through as Msg::Key.
#[test]
fn test_route_key_chat_non_scroll_passes_through() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::Chat;

    assert!(matches!(
        route_key(&mut app, &key(KeyCode::Char('a'))),
        RouteAction::Msg(message) if matches!(*message, Msg::Key(_))
    ));
    assert!(matches!(
        route_key(&mut app, &key(KeyCode::Char('/'))),
        RouteAction::Msg(message) if matches!(*message, Msg::Key(_))
    ));
}

/// Scroll state: line_down moves offset by 1.
#[test]
fn test_scroll_state_new_starts_at_bottom() {
    let scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    assert_eq!(scroll.offset, 90); // starts at bottom (100 - 10)
    assert!(scroll.follow);
}

#[test]
fn test_scroll_state_line_up() {
    let mut scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    scroll.follow = false;
    scroll.line_up();
    assert_eq!(scroll.offset, 89);
}

#[test]
fn test_scroll_state_page_down_at_bottom() {
    let mut scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    scroll.follow = false;
    scroll.offset = 50;
    scroll.page_down();
    assert_eq!(scroll.offset, 60); // moved down by 10
}

#[test]
fn test_scroll_state_to_bottom() {
    let mut scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    scroll.offset = 50;
    scroll.follow = false;
    scroll.to_bottom();
    assert_eq!(scroll.offset, 90);
    assert!(scroll.follow);
}

#[test]
fn test_scroll_state_position_percent() {
    let scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    assert_eq!(scroll.position_percent(), 100); // at bottom (90/90)

    let mut scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    scroll.follow = false;
    scroll.offset = 45;
    assert_eq!(scroll.position_percent(), 50); // 45 / 90 = 50%
}

#[test]
fn test_scroll_state_window() {
    let scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    let win = scroll.window();
    assert_eq!(win.start, 90);
    assert_eq!(win.end, 100);
}

#[test]
fn test_scroll_state_on_resize() {
    let mut scroll = luxe_tui::scroll::ScrollState::new(10, 100);
    scroll.follow = false;
    scroll.offset = 80;
    scroll.on_resize(20);
    assert_eq!(scroll.viewport, 20);
    assert_eq!(scroll.offset, 80); // within new bounds
}

#[test]
fn test_default_chat_omits_conversation_label() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    let buffer = render_app(&mut app, &backend);
    let top = row_text(&buffer, 0, 80);

    assert!(!top.contains("Conversation"), "conversation label should be hidden: {top:?}");
    assert!(!top.contains('%'), "scroll percentage should be hidden: {top:?}");
}
