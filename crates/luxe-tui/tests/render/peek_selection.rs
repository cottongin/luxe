//! Peek selection — text selected with the mouse in the Changes pane, by the character.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use luxe_tui::fixture::seed_file_view;

const FILE: &str = "fn alpha() {\n\tlet bravo = 1;\n}\n";

/// A File view on `text`, drawn once so the pointer has something to land on.
fn file_view(text: &str, width: u16, height: u16) -> (App, TestBackend, Buffer) {
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    seed_file_view(&mut app, "src/lib.rs", text, None);
    let buffer = render_app(&mut app, &backend);
    (app, backend, buffer)
}

fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
    let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    let action = route_mouse(app, &event);
    apply(app, action);
}

/// Presses at `from`, drags to `to` and lets go.
fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), from);
    mouse(app, MouseEventKind::Drag(MouseButton::Left), to);
    mouse(app, MouseEventKind::Up(MouseButton::Left), to);
}

/// Where `needle` first sits on screen, as `(column, row)`.
fn find(buffer: &Buffer, needle: &str, width: u16, height: u16) -> (u16, u16) {
    let row = row_containing(buffer, needle, width, height);
    (column_of(&row_text(buffer, row, width), needle), row)
}

/// A drag over the File view's code highlights the characters it covers and copies them on
/// release, as it does in the transcript. The code used to be only a line-pick target, so the same
/// drag picked the whole line instead.
#[test]
fn dragging_over_file_view_code_selects_and_copies_the_characters() {
    let (width, height) = (100, 20);
    let (mut app, backend, buffer) = file_view(FILE, width, height);
    let (column, row) = find(&buffer, "bravo", width, height);

    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), (column, row));
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), (column + 5, row));
    let buffer = render_app(&mut app, &backend);
    let selected = app.theme_styles.selected_bg.bg.expect("a selection background");
    assert_eq!(buffer[(column, row)].bg, selected, "the dragged code is highlighted");
    assert_ne!(buffer[(column + 6, row)].bg, selected, "and nothing past it");

    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), (column + 5, row));
    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"));
    assert!(!shown.contains("ask about"), "no lines were picked: {shown}");
}

/// A drag across lines copies the source between its ends: no line numbers, and the tab that
/// indents the second line comes back a tab rather than the spaces it is drawn as.
#[test]
fn a_drag_across_lines_copies_the_source_tabs_and_all() {
    let (width, height) = (100, 20);
    let (mut app, _backend, buffer) = file_view(FILE, width, height);
    let start = find(&buffer, "alpha", width, height);
    let (column, row) = find(&buffer, "bravo", width, height);

    drag(&mut app, start, (column + 5, row));
    assert_eq!(app.outbox.copy.as_deref(), Some("alpha() {\n\tlet bravo"));
}

/// The line numbers still pick whole lines, which is how a reader asks the agent about them.
#[test]
fn a_drag_down_the_line_numbers_still_picks_lines() {
    let (width, height) = (100, 20);
    let (mut app, backend, buffer) = file_view(FILE, width, height);
    let (code, first) = find(&buffer, "fn alpha", width, height);
    let (_, second) = find(&buffer, "bravo", width, height);
    // The number, the keyboard's mark, the change bar and a space come before the code
    let gutter = code - 4;

    drag(&mut app, (gutter, first), (gutter, second));
    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(shown.contains("ask about lines 1–2"), "the lines are picked: {shown}");
    assert!(app.outbox.copy.is_none(), "and a pick copies nothing by itself");
}

/// A double-click on the File view's code takes the word under it, the way it does elsewhere.
#[test]
fn a_double_click_on_file_view_code_takes_the_word() {
    let (width, height) = (100, 20);
    let (mut app, backend, buffer) = file_view(FILE, width, height);
    let (column, row) = find(&buffer, "bravo", width, height);
    let now = std::time::Instant::now();

    click_cell(&mut app, column + 2, row, now);
    click_cell(&mut app, column + 2, row, now);
    render_app(&mut app, &backend);
    assert_eq!(app.outbox.copy.as_deref(), Some("bravo"));
}

/// A long line wraps, and a drag on a later row selects what that row shows, not the same columns
/// from the start of the line.
#[test]
fn a_drag_on_a_wrapped_row_selects_what_that_row_shows() {
    let (width, height) = (90, 20);
    let line = (0..60).map(|index| format!("w{index:03}")).collect::<Vec<_>>().join(" ");
    let (mut app, _backend, buffer) = file_view(&line, width, height);
    let (_, first) = find(&buffer, "w000", width, height);
    // Whichever word sits whole on the row after the first, wherever the wrap fell
    let text = row_text(&buffer, first + 1, width);
    let word = (0..60)
        .map(|index| format!("w{index:03}"))
        .find(|word| text.contains(&format!("{word} ")))
        .expect("the line wraps onto a second row");
    let column = column_of(&text, &word);

    drag(&mut app, (column, first + 1), (column + 4, first + 1));
    assert_eq!(app.outbox.copy.as_deref(), Some(word.as_str()));
}

/// In a side-by-side diff a drag stays in the column it began in: down the new side it copies the
/// new file's lines and skips the deletion the old side draws beside them.
#[test]
fn a_drag_down_the_new_column_copies_only_new_code() {
    let (width, height) = (260, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("peek-select-split", 2));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let buffer = render_app(&mut app, &backend);
    assert!(split_row(&buffer, "fn sum() {", &app, width, height), "the diff is side by side");

    let top = row_containing(&buffer, "fn sum() {", width, height);
    let text = row_text(&buffer, top, width);
    let (byte, _) = text.match_indices("fn sum() {").nth(1).expect("both columns show it");
    let new_side = text[..byte].chars().count() as u16;
    let (end, bottom) = find(&buffer, "let total = a - b;", width, height);

    drag(&mut app, (new_side, top), (end + 18, bottom));
    assert_eq!(app.outbox.copy.as_deref(), Some("fn sum() {\nlet total = a - b;"));
}

/// The unified diff selects the same way, across a deletion and the addition under it, and never
/// copies the line numbers or the `-`/`+` signs.
#[test]
fn a_drag_over_the_unified_diff_copies_its_code() {
    let (width, height) = (90, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("peek-select-unified", 2));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let buffer = render_app(&mut app, &backend);
    assert!(!split_row(&buffer, "fn sum() {", &app, width, height), "the diff is unified");

    let start = find(&buffer, "let total = a + b;", width, height);
    let (end, bottom) = find(&buffer, "let total = a - b;", width, height);
    drag(&mut app, start, (end + 18, bottom));
    assert_eq!(app.outbox.copy.as_deref(), Some("let total = a + b;\nlet total = a - b;"));
}
