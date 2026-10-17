//! Fixture peek — the changes pane on a file's text, filtered and focused.

use crate::app::App;

/// Seed a subscription's remaining allowance, so the statusline's plan meter can be reviewed.
///
/// Real values, copied from a live ChatGPT Plus response: a weekly window with a reset days out.
/// Opens `path` in the Changes pane's File view with `text` as its content, and `pick` as the
/// lines picked in it, for reviewing the File view without an engine to read the file.
pub fn seed_file_view(app: &mut App, path: &str, text: &str, pick: Option<(u32, u32)>) {
    let files = crate::app::messages::changes::changed_files(app);
    let mut view = crate::changes::ChangesView::new(path.to_string(), files);
    view.view = crate::changes::PeekView::File;
    view.text = crate::changes::TextLoad::Ready(Box::new(luxe_core::project_files::ProjectFile {
        path: path.to_string(),
        text: text.to_string(),
        bytes: text.len() as u64,
        truncated: false,
        binary: false,
    }));
    view.selection = pick;
    view.scroll = pick.map_or(0, |(first, _)| first.saturating_sub(8) as usize);
    view.scroll_max.set(usize::MAX);
    app.changes = Some(view);
    app.files.reveal(path);
    app.files.cursor = Some(path.to_string());
}

/// Puts the preview in visual-line mode from `from` to `to`, keyboard and all.
pub fn preview_visual(app: &mut App, from: u32, to: u32) {
    focus_preview(app, Some(from));
    if let Some(view) = app.changes.as_mut() {
        // What a first frame would have measured, so the move scrolls as it would on screen
        view.viewport_rows.set(30);
        view.toggle_visual();
        view.move_cursor(i64::from(to) - i64::from(from), false);
    }
}

/// Filters the tree by `query` over `files`, keyboard in it and still typing.
pub fn filter_tree(app: &mut App, files: &[String], query: &str) {
    app.update(crate::app::Msg::FocusFiles);
    app.files.set_project_files(files);
    app.files.open_filter();
    for character in query.chars() {
        app.files.edit_filter(Some(character));
    }
    let changes = crate::app::messages::files::changed(app);
    app.files.cursor = app.files.hits(&changes).best.clone();
}

/// Puts the keyboard in the preview, its cursor on `line` when given.
pub fn focus_preview(app: &mut App, line: Option<u32>) {
    app.update(crate::app::Msg::FocusPeek);
    if let (Some(view), Some(line)) = (app.changes.as_mut(), line) {
        view.cursor = Some(line);
    }
}
