//! Support — the app, backend and click helpers every render scenario builds on.

use super::*;

/// Helper to create a test backend with the given size.
pub(crate) fn test_backend(width: u16, height: u16) -> TestBackend {
    TestBackend::new(width, height)
}

/// Cells a dock heading badge spends in front of its label: the opening cap and one pad.
///
/// The badge's *leading edge* is what lands on the panel's content margin, so a test looking for
/// the label finds it this far in. Spent at every glyph tier — below nerd the cap is a filled cell
/// rather than a chamfer, so the column does not move.
pub(crate) const DOCK_HEADING_LEAD: u16 = 2;

/// The activity bar's unspent unit — `▏`, the default `ActivityBarStyle::Bars` track glyph. How a
/// test finds the row the indicator is on.
pub(crate) const ACTIVITY_BAR_TRACK: &str = "\u{258f}";
/// Every glyph the default bar can draw — the comet's weights, tapering onto the rail's own.
pub(crate) const ACTIVITY_BAR_GLYPHS: [char; 4] = ['\u{258c}', '\u{258d}', '\u{258e}', '\u{258f}'];

/// Screen column where `needle` starts in a rendered row (cells, not bytes).
pub(crate) fn column_of(text: &str, needle: &str) -> u16 {
    let byte = text.find(needle).unwrap_or_else(|| panic!("{needle:?} missing from row {text:?}"));
    text[..byte].chars().count() as u16
}

/// Applies a routed mouse action the way the event loop would (the subset the mouse tests emit).
pub(crate) fn apply(app: &mut App, action: RouteAction) {
    use luxe_tui::main_loop::ScrollAction;
    match action {
        RouteAction::Msg(message) => app.update(*message),
        RouteAction::Scroll(ScrollAction::Up(lines)) => app.scroll_state.up_by(lines),
        RouteAction::Scroll(ScrollAction::Down(lines)) => app.scroll_state.down_by(lines),
        RouteAction::None => {}
        other => panic!("unexpected route action in a mouse test: {other:?}"),
    }
}

/// Presses and releases the left button at one cell, at `at` on the multi-click clock.
///
/// Two clicks a test fires back to back land microseconds apart, which is a *double*-click; passing
/// the instant is how a test says "two separate clicks" or "one deliberate double-click".
pub(crate) fn click_cell(app: &mut App, column: u16, row: u16, at: std::time::Instant) {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
        let action = route_mouse_at(app, &event, at);
        apply(app, action);
    }
}

/// Helper to render the app and get the buffer.
pub(crate) fn render_app(app: &mut App, backend: &TestBackend) -> Buffer {
    let mut terminal = Terminal::new(backend.clone()).unwrap();
    terminal
        .draw(|f| {
            render(f, app);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Helper to create an app with a dummy channel.
pub(crate) fn create_app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    App::new(tx, KeyMap::defaults(), Config::default())
}

/// Helper to create an app with a specific theme.
pub(crate) fn create_app_with_config(config: Config) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    App::new(tx, KeyMap::defaults(), config)
}

/// Helper to create an app with a specific theme.
pub(crate) fn create_app_with_theme(theme: &str) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    App::new(
        tx,
        KeyMap::defaults(),
        Config { theme: Some(theme.to_string()), ..Default::default() },
    )
}

/// An app in a light palette. No built-in is light any more, but a reader's own theme can be.
pub(crate) fn create_app_with_light_theme() -> App {
    let mut app = create_app_with_theme("luxe");
    app.theme_styles = luxe_tui::fixture::light_theme_styles();
    app
}
