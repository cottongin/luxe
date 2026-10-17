//! Pointer routing — presses, drags, releases, right-clicks and hover, as [`RouteAction`]s.

use super::click::select_overlay_row;
use super::{click, clicked_target, wheel, RouteAction, ScrollAction};
use crate::app::{App, Msg};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::Duration;

/// Route a mouse event to a [`RouteAction`].
///
/// The wheel scrolls the region under the pointer; a press either acts on the chrome it hit or
/// anchors a text selection; a drag extends it and the release copies. Never panics on
/// out-of-bounds coordinates.
pub fn route_mouse(app: &mut App, mouse: &MouseEvent) -> RouteAction {
    route_mouse_at(app, mouse, std::time::Instant::now())
}

/// [`route_mouse`] against an explicit clock.
///
/// The only thing that reads it is the multi-click cadence — double-click selects a word, triple
/// selects the line — and a test cannot pace the wall clock, so the instant is a parameter.
pub fn route_mouse_at(app: &mut App, mouse: &MouseEvent, now: std::time::Instant) -> RouteAction {
    match mouse.kind {
        MouseEventKind::ScrollUp => wheel(app, mouse.column, mouse.row, true),
        MouseEventKind::ScrollDown => wheel(app, mouse.column, mouse.row, false),
        MouseEventKind::Down(MouseButton::Left) => press(app, mouse.column, mouse.row, now),
        MouseEventKind::Down(MouseButton::Right) => right_press(app, mouse.column, mouse.row),
        MouseEventKind::Drag(MouseButton::Left) => {
            match (app.pointer.divider_drag, app.pointer.scrollbar_drag) {
                (Some(handle), _) => {
                    // A press that went on to drag was a drag, not half of a double-click
                    app.pointer.divider_pressed = None;
                    RouteAction::msg(Msg::ResizePane {
                        pane: handle.pane,
                        width: handle.width_at(mouse.column),
                    })
                }
                (None, Some((region, y, height))) => {
                    steer(region, mouse.row.saturating_sub(y), height)
                }
                (None, None) => drag(app, mouse.column, mouse.row),
            }
        }
        MouseEventKind::Up(MouseButton::Left) => release(app),
        MouseEventKind::Moved => hover(app, mouse.column, mouse.row),
        _ => RouteAction::None,
    }
}

/// Handles a left press: grab the scrollbar, act on chrome, or anchor a text selection.
///
/// A press over selectable text does *not* act on what it hit. It cannot yet know whether the
/// gesture is a click or the start of a drag, and acting now is what made every attempt to select a
/// few words also select — and copy — the whole message under them. The target is remembered
/// instead and spent by [`release`] if no drag intervened.
pub(super) fn press(app: &mut App, col: u16, row: u16, now: std::time::Instant) -> RouteAction {
    // An open menu answers the click: an item does what it says, anywhere else puts it away
    if app.overlays.context_menu.is_some() {
        let item = match app.hit.borrow().at(col, row) {
            Some(crate::mouse::MouseTarget::ContextMenuItem(index)) => Some(*index),
            _ => None,
        };
        return match item {
            Some(index) => RouteAction::msg(Msg::ChooseMenuItem(index)),
            None => {
                app.overlays.context_menu = None;
                RouteAction::None
            }
        };
    }
    // A click anywhere puts the viewer away, the way a lightbox should. It is covering whatever
    // was clicked, so nothing else could have been meant.
    if app.overlays.image_viewer.is_some() {
        return RouteAction::msg(Msg::CloseImageViewer);
    }
    // The key card likewise: it is a thing to read, and a click is the reader done with it
    if app.overlays.keys_card {
        return RouteAction::msg(Msg::CloseKeysCard);
    }
    // A divider owns the gesture the same way: the pane follows the pointer until release
    let divider = match app.hit.borrow().at(col, row) {
        Some(crate::mouse::MouseTarget::Divider(handle)) => Some(*handle),
        _ => None,
    };
    if let Some(handle) = divider {
        return press_divider(app, handle, now);
    }
    // Grabbing the scrollbar owns the whole gesture: it jumps to the grabbed position and keeps
    // steering until release, so it must not also anchor a text selection.
    if let Some((region, y, height)) = scrollbar_track(app, col, row) {
        app.pointer.scrollbar_drag = Some((region, y, height));
        return steer(region, row.saturating_sub(y), height);
    }
    let target = app.hit.borrow().at(col, row).cloned();
    // Chrome drawn over a text surface — an overlay row above the transcript — was never going to
    // select anything, so it acts on the press the way a button should.
    let chrome = target.as_ref().is_some_and(|target| !target.acts_on_release());
    let hit = app.hit.borrow().text_at(col, row);
    let Some(hit) = hit.filter(|_| !chrome) else {
        app.pointer.pending_click = None;
        app.update(Msg::ClearTextSelection);
        return click(app, col, row);
    };
    app.pointer.pending_click = target;
    let span = app.press_chain(col, row, now);
    // An image row holds no words, so a chained click on one cannot be asking to select any. It is
    // asking to see the picture properly.
    if span != crate::selection::SelectionSpan::Caret {
        if let Some(crate::mouse::MouseTarget::ConversationImage(key)) =
            app.pointer.pending_click.take()
        {
            app.update(Msg::ClearTextSelection);
            return RouteAction::msg(Msg::OpenImageViewer(key));
        }
    }
    app.update(Msg::BeginTextSelection { hit, span });
    RouteAction::None
}

/// Handles a right press: open the menu for what is under the pointer, when it has one.
///
/// A row of the FILES tree, or a line of the File view. A line outside the current pick becomes
/// the pick first, the way a right-click on an unselected file selects it in every file manager.
pub(super) fn right_press(app: &mut App, col: u16, row: u16) -> RouteAction {
    use crate::context_menu::ContextMenu;
    use crate::mouse::MouseTarget;
    let target = app.hit.borrow().at(col, row).cloned().or_else(|| file_line_at(app, col, row));
    let chord = |app: &App, action, fallback: &str| {
        app.keymap
            .chords_for(action)
            .and_then(|chords| chords.first())
            .map_or_else(|| fallback.to_string(), ToString::to_string)
    };
    app.overlays.context_menu = match target {
        Some(MouseTarget::DockFile(path)) => {
            let changed = app.project_changes().iter().any(|item| item.path == path);
            Some(ContextMenu::for_file((col, row), &path, changed))
        }
        Some(MouseTarget::DockFolder(path)) => {
            let changes = app.project_changes();
            let open = app.files.rows(&changes).iter().any(|row| {
                matches!(row, crate::files::TreeRow::Folder { path: at, open: true, .. } if *at == path)
            });
            Some(ContextMenu::for_folder((col, row), &path, open))
        }
        Some(MouseTarget::PeekLine(line)) => {
            if let Some(view) = app.changes.as_mut() {
                view.pick_for_menu(line);
            }
            let ask = chord(app, luxe_core::keymap::UiAction::AskAboutSelection, "alt+l");
            let copy = chord(app, luxe_core::keymap::UiAction::CopySelection, "");
            Some(ContextMenu::for_selection((col, row), &ask, &copy))
        }
        _ => None,
    };
    RouteAction::None
}

/// The File view line whose code is under `(col, row)`. Only a line's gutter is a click target, its
/// code being selectable text, but a right-click on either means that line.
fn file_line_at(app: &App, col: u16, row: u16) -> Option<crate::mouse::MouseTarget> {
    let hit = app.hit.borrow().text_at(col, row)?;
    let file_view =
        app.changes.as_ref().is_some_and(|view| view.view == crate::changes::PeekView::File);
    let code = hit.surface == crate::mouse::TextSurface::Peek(crate::mouse::PeekSide::Whole);
    let line = u32::try_from(hit.point.line + 1).ok()?;
    (file_view && code).then_some(crate::mouse::MouseTarget::PeekLine(line))
}

/// Handles a left release: copy what the gesture selected, or spend the remembered click.
///
/// Whether cells ended up covered is the whole test. A gesture that covered some is a selection, so
/// releasing copies it — the copy-on-select a terminal would have done. One that covered none never
/// moved off its cell, so it was a click.
pub(super) fn release(app: &mut App) -> RouteAction {
    if std::mem::take(&mut app.pointer.peek_dragging) {
        app.pointer.pending_click = None;
        return RouteAction::None;
    }
    if app.pointer.divider_drag.take().is_some() {
        app.pointer.pending_click = None;
        return RouteAction::msg(Msg::FinishPaneResize);
    }
    if app.pointer.scrollbar_drag.take().is_some() {
        app.pointer.pending_click = None;
        return RouteAction::None;
    }
    let pending = app.pointer.pending_click.take();
    if app.pointer.selection_drag.take().is_none() {
        return RouteAction::None;
    }
    if app.text_selection_is_active() {
        return RouteAction::msg(Msg::CopySelection);
    }
    match pending {
        Some(target) => clicked_target(app, target),
        None => RouteAction::None,
    }
}

/// Two presses on one divider this close together are a double-click.
pub(super) const DIVIDER_DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Grabs a divider, or puts its pane back to its own width when this is the second click of a
/// double-click.
pub(super) fn press_divider(
    app: &mut App,
    handle: crate::mouse::DividerHandle,
    now: std::time::Instant,
) -> RouteAction {
    app.pointer.pending_click = None;
    let again = app.pointer.divider_pressed.take().is_some_and(|(pane, at)| {
        pane == handle.pane && now.saturating_duration_since(at) <= DIVIDER_DOUBLE_CLICK
    });
    if again {
        return RouteAction::msg(Msg::ResetPaneWidth(handle.pane));
    }
    app.pointer.divider_pressed = Some((handle.pane, now));
    app.pointer.divider_drag = Some(handle);
    RouteAction::None
}

/// The scrollbar track under `(col, row)`, as the surface it steers and its `(y, height)`.
pub(super) fn scrollbar_track(
    app: &App,
    col: u16,
    row: u16,
) -> Option<(crate::mouse::ScrollRegionId, u16, u16)> {
    match app.hit.borrow().at(col, row) {
        Some(crate::mouse::MouseTarget::Scrollbar { region, y, height }) => {
            Some((*region, *y, *height))
        }
        _ => None,
    }
}

/// Points `region` at `row` of a `height`-tall track — what a press on a thumb, and every drag
/// after it, means.
pub(super) fn steer(region: crate::mouse::ScrollRegionId, row: u16, height: u16) -> RouteAction {
    match region {
        crate::mouse::ScrollRegionId::DockList(list) => {
            RouteAction::msg(Msg::DragDockList { list, row, height })
        }
        crate::mouse::ScrollRegionId::Conversation => {
            RouteAction::Scroll(ScrollAction::DragTo { row, height })
        }
        region => RouteAction::msg(Msg::DragScrollbar { region, row, height }),
    }
}

/// Extends the in-flight selection, auto-scrolling the transcript when the drag leaves the pane.
pub(super) fn drag(app: &mut App, col: u16, row: u16) -> RouteAction {
    // Picking lines in the File view: whichever line the pointer is over now is the far end
    if app.pointer.peek_dragging {
        let line = match app.hit.borrow().at(col, row) {
            Some(crate::mouse::MouseTarget::PeekLine(line)) => Some(*line),
            _ => None,
        };
        return match line {
            Some(head) => RouteAction::msg(Msg::ExtendPeekPick(head)),
            None => RouteAction::None,
        };
    }
    let Some(surface) = app.pointer.selection_drag else {
        return RouteAction::None;
    };
    let inside = app.hit.borrow().text_at(col, row).filter(|hit| hit.surface == surface);
    drag_scroll(app, surface, row);
    let hit = inside.or_else(|| app.hit.borrow().nearest_text(surface, col, row));
    match hit {
        Some(hit) => RouteAction::msg(Msg::ExtendTextSelection { hit }),
        None => RouteAction::None,
    }
}

/// Dragging past the top or bottom edge keeps a scrollable surface moving under the pointer, the
/// way a selection in any other scrollable view does. Only *vertical* overshoot scrolls; a drag
/// that merely strays sideways (over the dock, say) shouldn't.
fn drag_scroll(app: &mut App, surface: crate::mouse::TextSurface, row: u16) {
    use crate::mouse::TextSurface;
    let span = app.hit.borrow().text_span_y(surface);
    // Edge-inclusive: dragging *to* the first/last visible row is how you ask for more, and there's
    // nothing above row 0 to move onto
    let up = match span {
        Some((top, _)) if row <= top => true,
        Some((_, bottom)) if row + 1 >= bottom => false,
        _ => return,
    };
    match (surface, up) {
        (TextSurface::Conversation, true) => app.scroll_state.line_up(),
        (TextSurface::Conversation, false) => app.scroll_state.line_down(),
        (TextSurface::Peek(_), up) => app.update(Msg::ScrollChanges { up, lines: 1 }),
        (TextSurface::Composer, _) => {}
    }
}

/// Moves the highlight to whatever the pointer is over: list rows adopt the same selection the
/// arrow keys would set, other affordances just record a hover so their renderer can light up.
pub(super) fn hover(app: &mut App, col: u16, row: u16) -> RouteAction {
    use crate::mouse::MouseTarget;
    let target = app.hit.borrow().at(col, row).cloned();
    // An overlay's list follows the pointer as it follows the arrows; over the information column
    // that moves the *focus* there too, so the cursor the pointer left behind is the one Enter uses
    let overlay_row = target.as_ref().and_then(|target| select_overlay_row(app, target));
    let moved_row = overlay_row.unwrap_or_else(|| match &target {
        // A level the model can't honor refuses the highlight, so hovering it changes nothing.
        Some(MouseTarget::ThinkingRow(index)) => {
            app.overlays.thinking_picker.as_mut().is_some_and(|picker| picker.select(*index))
        }
        // Hovering a theme previews it, exactly as arrowing onto it does — the preview *is* the
        // picker, so the pointer should not be a second-class way to drive it.
        Some(MouseTarget::ThemeRow(index)) => {
            match app.overlays.theme_picker.as_mut().and_then(|picker| picker.select(*index)) {
                Some(theme) => {
                    app.preview_theme(&theme);
                    true
                }
                None => false,
            }
        }
        Some(MouseTarget::SessionRow(index))
            if app.current_view == crate::app::ViewMode::SessionPicker =>
        {
            app.overlays.session_picker.as_mut().map(|picker| picker.select(*index)).is_some()
        }
        Some(MouseTarget::DashboardItem(index)) if app.dashboard_owns_input() => {
            app.dashboard.as_mut().map(|dashboard| dashboard.select(*index)).is_some()
        }
        Some(MouseTarget::ConfigRow(index))
            if app.current_view == crate::app::ViewMode::Configuration =>
        {
            app.overlays.configuration.as_mut().map(|surface| surface.select(*index)).is_some()
        }
        Some(MouseTarget::ConfigPage(_)) => false,
        _ => false,
    });
    if app.pointer.hover == target && !moved_row {
        return RouteAction::None;
    }
    RouteAction::msg(Msg::HoverTarget(target))
}
