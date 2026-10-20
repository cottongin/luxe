//! Keys and the route actions every input resolves to, applied to the app.

use super::{route_mouse, TuiExit};
use crate::app::{App, Msg, ViewMode};
use crate::session_picker::SessionPickerKey;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

/// Result of routing a key event through the current view mode.
#[derive(Debug)]
pub enum RouteAction {
    /// Quit the application (Esc from Dashboard).
    Quit,
    /// Dashboard outcome (from dashboard.on_key).
    Dashboard(crate::dashboard::DashOutcome),
    /// Configuration-surface outcome (from configuration.on_key).
    Configuration(crate::configuration::ConfigOutcome),
    /// Session selected (id from picker.on_key). Side effect: picker is closed, view changes to Chat.
    SessionSelected(String),
    /// Picker closed without selection.
    PickerClosed,
    /// Apply a Msg to the app (Guided Review nav, Chat keys).
    Msg(Box<Msg>),
    /// Conversation scroll action (Chat view only, applied to `App::scroll_state`).
    Scroll(ScrollAction),
    /// Search navigate (true = next, false = prev).
    SearchNavigate(bool),
    /// Nothing to do (e.g. a mouse event over dead space). The loop ignores it.
    None,
}

impl RouteAction {
    /// A message for the app to apply, boxed so the other variants stay small.
    pub fn msg(msg: Msg) -> Self {
        RouteAction::Msg(Box::new(msg))
    }
}

/// A scroll action to apply to `App::scroll_state`.
#[derive(Debug)]
pub enum ScrollAction {
    /// Scroll up by n lines (one wheel notch is worth the configured step).
    Up(usize),
    /// Scroll down by n lines (one wheel notch is worth the configured step).
    Down(usize),
    PageUp,
    PageDown,
    ToBottom,
    /// Jump to where a scrollbar drag points: `row` within a `height`-tall track.
    DragTo {
        row: u16,
        height: u16,
    },
}

/// Routes a key through the current view's own handling, or hands it to the app as a message.
///
/// Not pure: the dashboard edits its prompt here, and Chat's search and block selection change
/// before the app sees anything. What comes back is what is left for `apply_route_action`.
pub fn route_key(app: &mut App, key: &KeyEvent) -> RouteAction {
    // The login wizard is modal wherever it opens: it is collecting a secret, and the surface
    // underneath would otherwise read `q` as "quit".
    if app.overlays.auth_wizard.is_some() {
        return RouteAction::msg(Msg::Key(*key));
    }

    let routed = match app.current_view {
        // A surface opened *from* the dashboard owns the keys while it is up, and Esc closes it
        // rather than quitting luxe — the surface underneath is what the reader came from.
        ViewMode::Dashboard if !app.dashboard_owns_input() => None,
        ViewMode::Dashboard => dashboard_key(app, key),
        // The model picker is the last step of setting a provider up, so while it is open it owns
        // the surface's keys — the same deal the wizard gets above.
        // The theme picker the settings open is drawn over the surface and owns it the same way
        ViewMode::Configuration
            if app.overlays.model_picker.is_some() || app.overlays.theme_picker.is_some() =>
        {
            None
        }
        ViewMode::Configuration => app
            .overlays
            .configuration
            .as_mut()
            .map(|surface| RouteAction::Configuration(surface.on_key(key))),
        // Esc always exits the picker, even one that is somehow not there
        ViewMode::SessionPicker if key.code == KeyCode::Esc => Some(RouteAction::PickerClosed),
        // something open over the picker takes the keys first, as it would over anything
        ViewMode::SessionPicker if app.overlays.cover_the_session_picker() || app.flyout_open() => {
            None
        }
        ViewMode::SessionPicker => {
            app.overlays.session_picker.as_mut().and_then(|picker| match picker.on_key(key) {
                SessionPickerKey::Selected(id) => Some(RouteAction::SessionSelected(id)),
                SessionPickerKey::Used => Some(RouteAction::msg(Msg::Tick)),
                SessionPickerKey::Ignored => None,
            })
        }
        // Review routes inside `App` like every other surface: `app::input::review_keys` claims
        // what the walkthrough owns, and `app::input::review` the overlays. Routed here instead,
        // half of one surface's input would be outside anything an `App`-level test or a
        // recording can drive
        ViewMode::GuidedReview => None,
        ViewMode::Chat => chat_key(app, key),
    };
    routed.unwrap_or_else(|| RouteAction::msg(Msg::Key(*key)))
}

/// The start screen's keys: its prompt is the composer, so the dashboard edits it and decides
/// what the rest mean. An open `/` or `@` flyout takes them first, as it does in a session.
fn dashboard_key(app: &mut App, key: &KeyEvent) -> Option<RouteAction> {
    if app.flyout_open() {
        return None;
    }
    let dashboard = app.dashboard.as_mut()?;
    let outcome = dashboard.on_key(key, &app.keymap, &mut app.editor);
    if outcome == crate::dashboard::DashOutcome::None {
        app.sync_dashboard_flyouts();
    }
    Some(RouteAction::Dashboard(outcome))
}

/// Chat's own keys: search while it is active, Esc out of a selected block, and the page keys.
/// An open overlay owns every key.
fn chat_key(app: &mut App, key: &KeyEvent) -> Option<RouteAction> {
    if app.overlays.take_chat_keys() {
        return None;
    }
    // Search takes priority when active.
    if app.search_state.is_active() {
        if let Some(action) = chat_search_key(app, key) {
            return Some(action);
        }
    }
    if key.code == KeyCode::Esc && app.selected_block.is_some() {
        app.selected_block = None;
        return Some(RouteAction::msg(Msg::Tick));
    }
    match key.code {
        KeyCode::PageUp => Some(RouteAction::Scroll(ScrollAction::PageUp)),
        KeyCode::PageDown => Some(RouteAction::Scroll(ScrollAction::PageDown)),
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(RouteAction::Scroll(ScrollAction::ToBottom))
        }
        _ => None,
    }
}

/// A key typed into Chat's active search.
fn chat_search_key(app: &mut App, key: &KeyEvent) -> Option<RouteAction> {
    let has_query = !app.search_state.query.is_empty();
    match key.code {
        KeyCode::Esc => {
            app.search_state.clear();
            Some(RouteAction::msg(Msg::Tick))
        }
        // Enter = next match (wraps); Up/Down also jump prev/next per spec.
        KeyCode::Enter if has_query => Some(RouteAction::SearchNavigate(true)),
        KeyCode::Up if has_query => Some(RouteAction::SearchNavigate(false)),
        KeyCode::Down if has_query => Some(RouteAction::SearchNavigate(true)),
        // ALL printable chars go to the query — n/N must never be stolen. A chord
        // is not a character, and typed in it went nowhere the reader could see.
        KeyCode::Char(c)
            if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.search_state.type_char(c, &[]);
            Some(RouteAction::msg(Msg::Tick))
        }
        // Backspace removes last char from query.
        KeyCode::Backspace => {
            app.search_state.query.pop();
            if app.search_state.query.is_empty() {
                app.search_state.clear();
            }
            Some(RouteAction::msg(Msg::Tick))
        }
        _ => None,
    }
}

/// Applies a routed [`RouteAction`] (from a key or mouse event) to the app. Shared by the key
/// and mouse paths so both drive identical behavior. `exit` is updated for the outcomes that end
/// the TUI (open config/project).
pub(super) fn apply_route_action(app: &mut App, action: RouteAction, exit: &mut TuiExit) {
    match action {
        RouteAction::Quit => {
            app.should_quit = true;
        }
        RouteAction::Dashboard(outcome) => {
            app.handle_dashboard_outcome(outcome);
        }
        // The one outcome the app cannot apply itself: an editor needs the terminal back, so the
        // TUI exits and the CLI runs it.
        RouteAction::Configuration(crate::configuration::ConfigOutcome::EditFile(path)) => {
            *exit = TuiExit::OpenInEditor(path);
            app.should_quit = true;
        }
        RouteAction::Configuration(outcome) => {
            app.apply_configuration_outcome(outcome);
        }
        RouteAction::SessionSelected(session_id) => app.open_picked_session(session_id),
        RouteAction::PickerClosed => {
            app.overlays.session_picker = None;
            app.current_view = if app.dashboard.is_some() {
                crate::app::ViewMode::Dashboard
            } else {
                crate::app::ViewMode::Chat
            };
        }
        RouteAction::Msg(msg) => app.update(*msg),
        RouteAction::Scroll(action) => apply_scroll(app, action),
        RouteAction::SearchNavigate(forward) => {
            if let Some(line_idx) =
                if forward { app.search_state.next_match() } else { app.search_state.prev_match() }
            {
                let viewport = app.scroll_state.viewport;
                if line_idx < app.scroll_state.offset {
                    app.scroll_state.offset = line_idx;
                } else if line_idx >= app.scroll_state.offset + viewport {
                    app.scroll_state.offset = line_idx.saturating_sub(viewport.saturating_sub(1));
                }
            }
        }
        RouteAction::None => {}
    }
}

/// Applies a conversation [`ScrollAction`] to the app's scroll state.
pub(super) fn apply_scroll(app: &mut App, action: ScrollAction) {
    match action {
        ScrollAction::Up(lines) => app.scroll_state.up_by(lines),
        ScrollAction::Down(lines) => app.scroll_state.down_by(lines),
        ScrollAction::PageUp => app.scroll_state.page_up(),
        ScrollAction::PageDown => app.scroll_state.page_down(),
        ScrollAction::ToBottom => app.scroll_state.to_bottom(),
        ScrollAction::DragTo { row, height } => app.scroll_state.drag_to(row, height),
    }
}

/// Applies one terminal event. Returns whether it was pointer motion that resolved to nothing.
pub(super) fn apply_terminal_event(app: &mut App, event: Event, exit: &mut TuiExit) -> bool {
    let idle = apply_terminal_event_inner(app, event, exit);
    collect_exit(app, exit);
    idle
}

/// [`collect_exit`], for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) fn collect_exit_for_tests(app: &mut App, exit: &mut TuiExit) {
    collect_exit(app, exit);
}

/// Turns what the app parked for the loop into the loop's exit.
///
/// A surface that picked a file to edit cannot run the editor itself — it needs the terminal back,
/// and only this loop can hand it over — and a worktree cannot be removed while the process stands
/// in it. Collected after engine events as well as keys: the removal is decided by the engine's
/// answer, and one that waited for the next key press left luxe running until something was typed.
pub(super) fn collect_exit(app: &mut App, exit: &mut TuiExit) {
    if let Some(parked) = app.outbox.exit() {
        *exit = parked;
        app.should_quit = true;
    }
}

pub(super) fn apply_terminal_event_inner(app: &mut App, event: Event, exit: &mut TuiExit) -> bool {
    match event {
        // the Windows console reports a release after every press; acting on it typed each key twice
        Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Release => false,
        Event::Key(key) => {
            let key = crate::option_layer::normalize(key);
            let action = route_key(app, &key);
            apply_route_action(app, action, exit);
            false
        }
        // Mouse: wheel scroll + clicks route through the hit-region subsystem, sharing the same
        // RouteAction application as keys. Ignored when capture is off.
        Event::Mouse(mouse) if app.mouse_enabled => {
            let action = route_mouse(app, &mouse);
            let idle = mouse.kind == MouseEventKind::Moved && matches!(action, RouteAction::None);
            apply_route_action(app, action, exit);
            idle
        }
        // The terminal already read the clipboard; probing it again delays text paste.
        Event::Paste(text) => {
            app.update(Msg::PasteText(text));
            false
        }
        Event::Resize(width, height) => {
            app.update(Msg::TerminalResized { width, height });
            false
        }
        // Nothing on screen changes; the workspace runtime paces its GitHub checks on it, and a
        // notification is sent only while the terminal is in the background
        Event::FocusGained | Event::FocusLost => {
            let focused = matches!(event, Event::FocusGained);
            app.terminal_focused = Some(focused);
            app.send_quietly(luxe_core::Action::FocusChanged { focused }, "report focus");
            true
        }
        _ => false,
    }
}
