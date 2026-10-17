//! Preview keys — the File view and the diff, while the keyboard is in them.
//!
//! `Ctrl+T` walks the keyboard from the composer to the FILES tree to here and back, and `Tab`
//! trades it between the tree and the pane. In the File view a cursor moves line by line, the way
//! the review's diff does, so a pick can be made from the keyboard as well as with a drag: `v` or
//! `V` is vim's visual-line mode, where every move grows the pick until `y`, `Enter` or `Esc`. In
//! the diff the keys scroll. `Ctrl+D` and `Ctrl+U` move half a page in either, the only chords
//! the pane keeps; every other chord is the app's. A key the pane does not know hands the keyboard back and goes to the
//! composer, the same as the tree.

use crate::app::{App, Msg};
use crate::changes::PeekView;
use crate::mouse::PeekSelectionAction;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Lines past which a jump is the whole file, whichever way.
const EVERYTHING: i64 = i64::MAX / 4;

/// Answers whether the pane took `key`.
pub(super) fn peek_took(app: &mut App, key: KeyEvent) -> bool {
    let Some(view) = app.changes.as_mut() else { return false };
    if !view.focused {
        return false;
    }
    let page = i64::try_from(view.viewport_rows.get().saturating_sub(1).max(1)).unwrap_or(1);
    if key.modifiers == KeyModifiers::CONTROL {
        // vim's half-page moves, the one pair of chords the pane keeps for itself
        let half = (page / 2).max(1);
        let delta = match key.code {
            KeyCode::Char('d') => half,
            KeyCode::Char('u') => -half,
            _ => 0,
        };
        if delta != 0 {
            match view.view {
                PeekView::File => view.move_cursor(delta, false),
                PeekView::Diff => app.update(Msg::ScrollChanges {
                    up: delta < 0,
                    lines: usize::try_from(half).unwrap_or(1),
                }),
            }
            return true;
        }
    }
    if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        // A chord is the app's, not the pane's; it passes through and the keyboard stays here
        return false;
    }
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            view.focused = false;
            app.update(Msg::FocusFiles);
            return true;
        }
        KeyCode::Esc if view.visual || view.selection.is_some() => {
            view.visual = false;
            view.selection = None;
        }
        KeyCode::Esc => view.focused = false,
        KeyCode::Char('o') if view.visual => view.swap_pick_ends(),
        KeyCode::Char('o') => {
            app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ChangesOpenInEditor));
        }
        _ if view.view == PeekView::Diff => return diff_key(app, key, page),
        KeyCode::Down | KeyCode::Char('j') => view.move_cursor(1, shift),
        KeyCode::Up | KeyCode::Char('k') => view.move_cursor(-1, shift),
        KeyCode::Char('J') => view.move_cursor(1, true),
        KeyCode::Char('K') => view.move_cursor(-1, true),
        KeyCode::PageDown | KeyCode::Char(' ') => view.move_cursor(page, shift),
        KeyCode::PageUp => view.move_cursor(-page, shift),
        KeyCode::Home | KeyCode::Char('g') => view.move_cursor(-EVERYTHING, shift),
        KeyCode::End | KeyCode::Char('G') => view.move_cursor(EVERYTHING, shift),
        KeyCode::Char('v' | 'V') => view.toggle_visual(),
        KeyCode::Enter | KeyCode::Char('@') => {
            pick_cursor_line(app);
            app.update(Msg::PeekSelectionAction(PeekSelectionAction::Ask));
            // The question about those lines is typed next, and that happens in the composer
            if let Some(view) = app.changes.as_mut() {
                view.focused = false;
                view.visual = false;
            }
        }
        KeyCode::Char('y') => {
            let picked = app.changes.as_ref().is_some_and(|view| view.selection.is_some());
            pick_cursor_line(app);
            app.update(Msg::PeekSelectionAction(PeekSelectionAction::Copy));
            // A yank ends visual mode, as it does in vim, and leaves a plain pick where it was
            if let Some(view) = app.changes.as_mut() {
                if view.visual || !picked {
                    view.selection = None;
                }
                view.visual = false;
            }
        }
        // Anything else is the composer's, and so is the keyboard from here
        _ => {
            view.focused = false;
            return false;
        }
    }
    true
}

/// The diff's keys: it has rows to scroll rather than lines to stand on.
fn diff_key(app: &mut App, key: KeyEvent, page: i64) -> bool {
    let rows = usize::try_from(page).unwrap_or(1);
    let msg = match key.code {
        KeyCode::Down | KeyCode::Char('j') => Msg::ScrollChanges { up: false, lines: 1 },
        KeyCode::Up | KeyCode::Char('k') => Msg::ScrollChanges { up: true, lines: 1 },
        KeyCode::PageDown | KeyCode::Char(' ') => Msg::ScrollChanges { up: false, lines: rows },
        KeyCode::PageUp => Msg::ScrollChanges { up: true, lines: rows },
        KeyCode::Home | KeyCode::Char('g') => Msg::JumpChanges(true),
        KeyCode::End | KeyCode::Char('G') => Msg::JumpChanges(false),
        _ => {
            if let Some(view) = app.changes.as_mut() {
                view.focused = false;
            }
            return false;
        }
    };
    app.update(msg);
    true
}

/// Makes the cursor's line the pick when nothing is picked, so a key that acts on a pick acts on
/// the line the reader is standing on.
fn pick_cursor_line(app: &mut App) {
    let Some(view) = app.changes.as_mut() else { return };
    if view.selection.is_none() {
        let at = view.cursor.unwrap_or_else(|| view.top_line());
        view.selection = Some((at, at));
    }
}

#[cfg(test)]
mod tests {
    use crate::app::Msg;
    use crate::fixture::{seed_file_view, seeded_app, FixtureScenario};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut crate::app::App, code: KeyCode, modifiers: KeyModifiers) {
        app.update(Msg::Key(KeyEvent::new(code, modifiers)));
    }

    fn previewing() -> crate::app::App {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
        let text: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        seed_file_view(&mut app, "src/lib.rs", &text, None);
        app.changes.as_ref().expect("a preview").viewport_rows.set(10);
        app
    }

    /// The walk: Ctrl+T's message takes the keyboard from the composer to the tree to the preview,
    /// Tab trades it back to the tree, and Esc gives it to the composer.
    #[test]
    fn the_keyboard_walks_from_the_composer_to_the_tree_to_the_preview() {
        let mut app = previewing();
        app.update(Msg::FocusFiles);
        assert!(app.files.focused);
        app.update(Msg::FocusFiles);
        let view = app.changes.as_ref().expect("a preview");
        assert!(view.focused && !app.files.focused);
        assert_eq!(view.cursor, Some(1), "the cursor starts on the first line showing");

        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert!(app.files.focused && !app.changes.as_ref().expect("a preview").focused);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert!(app.changes.as_ref().expect("a preview").focused);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.changes.as_ref().expect("still open").focused && !app.files.focused);
    }

    /// The cursor moves line by line and the view follows it; Shift, or the capitals, grow a pick
    /// from where it stood; a plain move drops the pick.
    #[test]
    fn the_cursor_moves_scrolls_and_picks() {
        let mut app = previewing();
        app.update(Msg::FocusPeek);
        for _ in 0..12 {
            press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        }
        let view = app.changes.as_ref().expect("a preview");
        assert_eq!(view.cursor, Some(13));
        assert_eq!(view.scroll, 3, "scrolled the least that keeps line 13 on a 10-row screen");

        press(&mut app, KeyCode::Char('J'), KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
        assert_eq!(app.changes.as_ref().expect("a preview").selected_lines(), Some((13, 15)));
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.changes.as_ref().expect("a preview").selection, None);

        press(&mut app, KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(app.changes.as_ref().expect("a preview").cursor, Some(40));
        press(&mut app, KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(app.changes.as_ref().expect("a preview").cursor, Some(1));
    }

    /// Enter asks about the pick, or the line the cursor is on, and hands the keyboard to the
    /// composer, where the question is typed; a letter the pane does not know goes there too.
    #[test]
    fn enter_asks_about_the_cursor_line_and_typing_goes_to_the_composer() {
        let mut app = previewing();
        app.update(Msg::FocusPeek);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            app.editor.current_input().contains("src/lib.rs:2"),
            "{}",
            app.editor.current_input()
        );
        assert!(!app.changes.as_ref().expect("still open").focused);

        app.update(Msg::FocusPeek);
        press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
        assert!(!app.changes.as_ref().expect("still open").focused);
        assert!(app.editor.current_input().ends_with('w'), "{}", app.editor.current_input());
    }

    /// `v` is visual-line mode: every move after it grows the pick, `o` swaps which end moves,
    /// `y` copies and leaves the mode, and `Esc` drops it. Before, a move after `v` dropped the
    /// pick, which is what a plain move does and not what anyone pressing `v` meant.
    #[test]
    fn v_keeps_growing_the_pick_until_it_is_used_or_dropped() {
        let mut app = previewing();
        app.update(Msg::FocusPeek);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        let view = app.changes.as_ref().expect("a preview");
        assert!(view.visual);
        assert_eq!(view.selected_lines(), Some((2, 4)));

        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.changes.as_ref().expect("a preview").selected_lines(), Some((1, 4)));

        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        let view = app.changes.as_ref().expect("a preview");
        assert!(!view.visual && view.selection.is_none(), "a yank ends the mode");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.changes.as_ref().expect("a preview").selection, None, "plain moves again");

        press(&mut app, KeyCode::Char('V'), KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let view = app.changes.as_ref().expect("a preview");
        assert!(!view.visual && view.selection.is_none() && view.focused, "Esc drops it, stays");
    }

    /// `Ctrl+D` and `Ctrl+U` move the cursor half a page, and in visual mode grow the pick by it.
    #[test]
    fn ctrl_d_and_ctrl_u_move_half_a_page() {
        let mut app = previewing();
        app.update(Msg::FocusPeek);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        // ten rows on screen: a page is nine, half of it four
        assert_eq!(app.changes.as_ref().expect("a preview").cursor, Some(5));
        press(&mut app, KeyCode::Char('v'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.changes.as_ref().expect("a preview").selected_lines(), Some((5, 9)));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        let view = app.changes.as_ref().expect("a preview");
        assert_eq!(view.cursor, Some(5));
        assert!(view.focused, "the keyboard stays in the pane");
    }
}
