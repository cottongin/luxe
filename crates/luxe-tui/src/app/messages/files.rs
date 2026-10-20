//! FILES reduction — opening folders, choosing what the tree shows, and the listings it waits on.
//!
//! The tree owns no project state. It asks the engine for a folder at a time
//! (`Action::ListDirectory`) and draws what comes back beside what the workspace snapshot says
//! changed.

use crate::app::{App, Msg};
use crate::workspace_dock::{DockChangeItem, DockChanges};
use luxe_core::engine::Action;
use luxe_core::project_files::ListingRequestId;

/// What the snapshot says changed in the project, in the shape the tree reads.
pub(crate) fn changed(app: &App) -> Vec<DockChangeItem> {
    app.workspace
        .as_ref()
        .and_then(DockChanges::from_snapshot)
        .map(|changes| changes.files)
        .unwrap_or_default()
}

/// Most listings the tree has asked for and not had back.
///
/// A change across the project stales every open folder at once, and asking for all of them in one
/// go would fill the engine's queue ahead of what the reader sends next: a steer, a cancel. Each
/// landed listing syncs again, so the rest follow as these come back.
pub(crate) const MAX_LISTINGS_IN_FLIGHT: usize = 8;

/// Asks for the listings the tree is about to need and does not have, a few at a time.
///
/// Called whenever what is open, or what changed, may have moved, and on every landed listing.
/// The reader didn't ask for these, so a refused send says nothing; it leaves the folder unasked,
/// and the next sync tries again rather than the tree waiting on an answer that is never coming.
pub(crate) fn sync(app: &mut App) {
    if app.workspace.is_none() {
        return;
    }
    let changes = changed(app);
    app.files.refresh_if_changed(&changes);
    let room = MAX_LISTINGS_IN_FLIGHT.saturating_sub(app.files.in_flight());
    for folder in app.files.wanted(&changes).into_iter().take(room) {
        // A full queue refuses the rest of this burst too
        if !ask_for_listing(app, &folder) {
            break;
        }
    }
}

/// Asks the engine for one folder's listing and notes it as in flight. Answers whether it went.
fn ask_for_listing(app: &mut App, folder: &str) -> bool {
    let request_id = ListingRequestId::new();
    let action = Action::ListDirectory {
        request_id: request_id.clone(),
        path: folder.to_string(),
        include_ignored: app.files.show_ignored,
    };
    if !app.send_quietly(action, "list a folder") {
        return false;
    }
    app.files.requested(folder, request_id);
    true
}

/// Applies a landed listing or its failure. Answers whether the event was the tree's.
pub(crate) fn apply(app: &mut App, event: &luxe_core::event::Event) -> bool {
    use luxe_core::event::Event;
    match event {
        Event::DirectoryListed { request_id, listing } => {
            app.files.accept(request_id, listing.clone());
        }
        Event::DirectoryListFailed { request_id, path, error } => {
            app.files.fail(request_id, path, error.to_string());
        }
        _ => return false,
    }
    // A folder that just arrived may hold open folders of its own
    sync(app);
    true
}

/// Does what a context-menu item offers.
pub(crate) fn run_menu(app: &mut App, action: crate::context_menu::MenuAction) {
    use crate::context_menu::MenuAction;
    match action {
        MenuAction::Preview(path) => app.update(Msg::OpenProjectFile(path)),
        MenuAction::ShowDiff(path) => {
            app.update(Msg::OpenProjectFile(path));
            if app.changes.as_ref().is_some_and(|view| view.view != crate::changes::PeekView::Diff)
            {
                app.update(Msg::TogglePeekView);
            }
        }
        MenuAction::OpenInEditor(path) => {
            crate::app::commands::editor::open_project_path(app, &path);
        }
        MenuAction::OpenInFileManager(path) => {
            crate::app::commands::editor::open_in_file_manager(app, &path);
        }
        MenuAction::Mention(path) => {
            if !app.editor.current_input().is_empty() && !app.editor.current_input().ends_with(' ')
            {
                app.editor.insert_text(" ");
            }
            app.editor.insert_reference(&format!("@[file:{path}]"));
            app.editor.insert_text(" ");
            app.files.focused = false;
        }
        MenuAction::Review(path) => {
            app.changes = None;
            app.review.pending_file = Some(path);
            crate::app::commands::dispatch_action(app, luxe_core::keymap::UiAction::ReviewOpen);
        }
        MenuAction::CopyPath(path) => app.update(Msg::CopyText(path)),
        MenuAction::ToggleFolder(path) => app.update(Msg::ToggleFolder(path)),
        MenuAction::ToggleScope => app.update(Msg::ToggleFilesScope),
        MenuAction::ToggleIgnored => app.update(Msg::ToggleIgnoredFiles),
        MenuAction::Selection(action) => {
            super::changes::selection_action(app, action);
        }
    }
}

/// Takes the project's files, read again off the frame for the tree and the `@` picker; `None` when
/// the read failed, which leaves the last listing standing.
pub(super) fn listed(app: &mut App, files: Option<Vec<String>>) {
    app.file_listing.in_flight = false;
    if let Some(files) = files {
        app.file_provider.set_listing(files);
        app.files.set_project_files(app.file_provider.files());
        if let Some(mention) = app.overlays.mention.as_mut() {
            mention.set_files(app.file_provider.clone());
        }
    }
}

/// Reduces one FILES message.
pub(super) fn reduce(app: &mut App, msg: Msg) {
    match msg {
        Msg::ToggleFolder(folder) => {
            let changes = changed(app);
            app.files.toggle(&folder, &changes);
            app.files.cursor = Some(folder);
        }
        Msg::ToggleFilesScope => app.files.changed_only = !app.files.changed_only,
        Msg::ToggleIgnoredFiles => app.files.toggle_ignored(),
        Msg::FocusPeek => {
            app.files.focused = false;
            if let Some(view) = app.changes.as_mut() {
                view.focused = true;
                if view.cursor.is_none() {
                    view.cursor = Some(view.top_line());
                }
            }
            return;
        }
        Msg::ChooseMenuItem(index) => {
            let chosen = app.overlays.context_menu.take().and_then(|mut menu| {
                menu.selected = index;
                menu.chosen()
            });
            if let Some(action) = chosen {
                run_menu(app, action);
            }
            return;
        }
        // The keyboard's walk: composer, then the tree, then the preview when one is open, then
        // the composer again. `Tab` inside the tree and the preview trades between those two.
        Msg::FocusFiles => {
            let previewing = app.changes.as_ref().is_some_and(|view| view.focused);
            if previewing {
                if let Some(view) = app.changes.as_mut() {
                    view.focused = false;
                }
                app.files.focused = true;
            } else if app.files.focused && app.changes.is_some() {
                app.files.focused = false;
                return app.update(Msg::FocusPeek);
            } else {
                app.files.focused = !app.files.focused;
            }
            if app.files.focused {
                // A tree the reader cannot see is not one they can move through
                if app.dock.presentation(app.workspace_layout)
                    == crate::workspace_dock::DockPresentation::Hidden
                {
                    app.dock.show();
                }
                let changes = changed(app);
                let rows = app.files.rows(&changes);
                let showing = app.changes.as_ref().map(|view| view.path.clone());
                let landed = app
                    .files
                    .cursor
                    .clone()
                    .or(showing)
                    .filter(|path| rows.iter().any(|row| row.path() == Some(path.as_str())));
                app.files.cursor =
                    landed.or_else(|| rows.iter().find_map(|row| row.path().map(str::to_string)));
            }
        }
        _ => return,
    }
    sync(app);
}
