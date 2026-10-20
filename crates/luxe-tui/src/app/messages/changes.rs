//! Changes-pane reduction — opening, stepping, scrolling, and the capture it waits on.
//!
//! The pane owns no project state. It asks the engine for one file's diff
//! (`Action::LoadFileDiff`), renders whatever comes back, and hands the reader on to Guided Review
//! when a glance turns out not to be enough.

use crate::app::{App, ConvoBlock, Msg};
use crate::changes::{ChangedFile, ChangesView};
use crate::workspace_dock::{DockChangeRow, DockChanges, DockExternal};
use luxe_core::engine::Action;
use luxe_core::file_diff::{is_external_path, FileDiffRequestId};

/// Rows the pane's own file list may hold.
///
/// Generous where the dock's budget is tight — the dock is competing for a rail with four other
/// sections, and this list only has to be walkable.
const FILE_LIST_MAX: usize = 500;

/// Opens the pane on `path`, or points an open one at a different file.
pub(super) fn open(app: &mut App, path: String) {
    let files = changed_files(app);
    mark_seen(app, &files, &path);
    match app.changes.as_mut() {
        Some(view) if view.path == path => {}
        Some(view) => {
            view.resync(files);
            view.show(path);
        }
        None => {
            app.changes = Some(ChangesView::new(path, files));
        }
    }
    if let Some(view) = app.changes.as_mut() {
        view.view = crate::changes::PeekView::Diff;
    }
    request_capture(app);
}

/// Records a look at a file outside the project, with the ± it carried when it was looked at.
///
/// Only outside the project. A glance at a *project* file marks nothing, because FILES counts
/// what a review has covered and a peek is not a review; nothing can review these, so a look is
/// the only state they have.
fn mark_seen(app: &mut App, files: &[ChangedFile], path: &str) {
    if !is_external_path(path) {
        return;
    }
    let Some(file) = files.iter().find(|file| file.path == path) else {
        return;
    };
    app.external_seen.insert(
        path.to_string(),
        (file.additions.unwrap_or_default(), file.deletions.unwrap_or_default()),
    );
}

/// Shows a project file from the FILES tree: its diff when it changed, its text when it did not.
///
/// One pane for both, so a file that changed can still be read whole (`File`), and stepping keeps
/// working across the changed ones.
pub(super) fn open_project_file(app: &mut App, path: String) {
    let files = changed_files(app);
    app.files.reveal(&path);
    app.files.cursor = Some(path.clone());
    if files.iter().any(|file| file.path == path) {
        open(app, path);
        if let Some(view) = app.changes.as_mut() {
            view.view = crate::changes::PeekView::Diff;
        }
        super::files::sync(app);
        return;
    }
    match app.changes.as_mut() {
        Some(view) => {
            view.resync(files);
            if view.path != path {
                view.show(path);
            }
        }
        None => app.changes = Some(ChangesView::new(path, files)),
    }
    if let Some(view) = app.changes.as_mut() {
        view.view = crate::changes::PeekView::File;
    }
    request_text(app);
    super::files::sync(app);
}

/// Switches a changed file between its diff and its text, asking for whichever is not here yet.
pub(super) fn toggle_view(app: &mut App) {
    let Some(view) = app.changes.as_mut() else {
        return;
    };
    if !view.changed() {
        return;
    }
    view.scroll = 0;
    view.view = match view.view {
        crate::changes::PeekView::Diff => crate::changes::PeekView::File,
        crate::changes::PeekView::File => crate::changes::PeekView::Diff,
    };
    match view.view {
        crate::changes::PeekView::File if view.text == crate::changes::TextLoad::Idle => {
            request_text(app)
        }
        crate::changes::PeekView::Diff if view.request.is_none() && view.file().is_none() => {
            request_capture(app)
        }
        _ => {}
    }
}

/// Does what the action row under a pick, or its key, asked for.
pub(crate) fn selection_action(app: &mut App, action: crate::mouse::PeekSelectionAction) {
    use crate::mouse::PeekSelectionAction as A;
    let Some(view) = app.changes.as_ref() else {
        return;
    };
    match action {
        A::Ask => {
            let Some(mention) = view.selection_mention() else { return };
            // A pointer in the draft, where the question about it is about to be typed
            if !app.editor.current_input().is_empty() && !app.editor.current_input().ends_with(' ')
            {
                app.editor.insert_text(" ");
            }
            app.editor.insert_reference(&mention);
            app.editor.insert_text(" ");
            if let Some(view) = app.changes.as_mut() {
                view.selection = None;
                view.visual = false;
            }
        }
        A::Copy => {
            // Selected text is the more exact answer when there is some; whole lines otherwise
            let text = view.cells_text().filter(|text| !text.is_empty());
            if let Some(text) = text.or_else(|| view.selected_text()) {
                app.update(Msg::CopyText(text));
            }
        }
        A::OpenInEditor => {
            crate::app::commands::dispatch_action(
                app,
                luxe_core::keymap::UiAction::ChangesOpenInEditor,
            );
        }
        A::Clear => {
            if let Some(view) = app.changes.as_mut() {
                view.selection = None;
                view.visual = false;
            }
        }
    }
}

/// Asks the engine for the visible file's text.
fn request_text(app: &mut App) {
    let Some(view) = app.changes.as_mut() else {
        return;
    };
    let request_id = luxe_core::project_files::FileReadRequestId::new();
    let path = view.path.clone();
    view.text_request = Some(request_id.clone());
    view.text = crate::changes::TextLoad::Loading;
    if !app.send_action(Action::ReadProjectFile { request_id, path }, "read a file") {
        if let Some(view) = app.changes.as_mut() {
            view.text_request = None;
            view.text = crate::changes::TextLoad::Failed(
                luxe_core::project_files::ProjectFileFailure::Unreadable(
                    "luxe is not accepting requests".into(),
                ),
            );
        }
    }
}

/// Returns the column to the transcript.
pub(super) fn close(app: &mut App) {
    app.changes = None;
}

/// Steps to the next or previous changed file, reloading as it lands.
pub(super) fn step_file(app: &mut App, forward: bool) {
    let files = changed_files(app);
    let Some(view) = app.changes.as_mut() else {
        return;
    };
    view.resync(files.clone());
    let Some(next) = view.step(forward) else {
        return;
    };
    view.show(next.clone());
    let reading = view.view == crate::changes::PeekView::File;
    // Stepping onto a file is looking at it, the same as clicking it was.
    mark_seen(app, &files, &next);
    app.files.cursor = Some(next);
    request_capture(app);
    if reading {
        request_text(app);
    }
}

/// Leaves the pane and opens the full review, which will land on the same file once it loads.
///
/// A file outside the project carries no destination: a review is captured from the project's own
/// worktree, so it can never contain one, and asking it to land there would be asking for a file
/// it has no record of.
pub(super) fn open_review(app: &mut App) {
    let path = app.changes.as_ref().map(|view| view.path.clone());
    app.changes = None;
    app.review.pending_file = path.filter(|path| !is_external_path(path));
    crate::app::commands::dispatch_action(app, luxe_core::keymap::UiAction::ReviewOpen);
}

/// Asks the engine for the visible file's diff.
///
/// A refused send is reported in the pane rather than the transcript: the reader is looking at the
/// pane, and a system line behind it would be an answer delivered to an empty room.
fn request_capture(app: &mut App) {
    let Some(view) = app.changes.as_mut() else {
        return;
    };
    let request_id = FileDiffRequestId::new();
    let path = view.path.clone();
    view.request = Some(request_id.clone());
    if !app.send_action(Action::LoadFileDiff { request_id, path }, "load a file's changes") {
        if let Some(view) = app.changes.as_mut() {
            view.request = None;
            view.load = crate::changes::ChangesLoad::Failed(
                luxe_core::file_diff::FileDiffFailure::Git("luxe is not accepting requests".into()),
            );
        }
        app.push_convo(ConvoBlock::System("Could not read that file's changes.".to_string()));
    }
}

/// Every changed file in the dock's display order, the project's own first.
///
/// One ordering for both surfaces, derived from the same rows the dock draws: clicking a file and
/// then stepping has to walk the list the reader can see, not a second one sorted differently.
///
/// Work done elsewhere comes last and named by its absolute path, because that is where it sits in
/// the rail and what it has to be asked for by. Stepping runs off the end of the project's changes
/// into it rather than stopping there — the list is what this session changed, and the session did
/// change those too.
pub(crate) fn changed_files(app: &App) -> Vec<ChangedFile> {
    let Some(snapshot) = app.workspace.as_ref() else {
        return Vec::new();
    };
    let mut files: Vec<ChangedFile> = DockChanges::from_snapshot(snapshot)
        .map(|changes| {
            changes
                .rows(app.review.seen.as_ref(), FILE_LIST_MAX)
                .into_iter()
                .filter_map(|row| match row {
                    DockChangeRow::Directory(_) => None,
                    DockChangeRow::File { path, status, additions, deletions, .. } => {
                        Some(ChangedFile { path, status, additions, deletions })
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(external) = DockExternal::from_snapshot(snapshot) {
        for group in &external.groups {
            files.extend(group.files.iter().map(|file| ChangedFile {
                path: group.peek_path(file),
                status: file.status,
                additions: file.additions,
                deletions: file.deletions,
            }));
        }
    }
    files.truncate(FILE_LIST_MAX);
    files
}

/// Applies a landed capture, or the failure that replaced it.
pub(crate) fn apply_capture(app: &mut App, event: &luxe_core::event::Event) -> bool {
    use luxe_core::event::Event;
    let Some(view) = app.changes.as_mut() else {
        return false;
    };
    match event {
        Event::FileDiffLoaded { request_id, diff } => view.accept(request_id, (**diff).clone()),
        Event::FileDiffFailed { request_id, path, error } => {
            view.fail(request_id, path, error.clone())
        }
        Event::ProjectFileRead { request_id, file } => {
            view.accept_text(request_id, (**file).clone())
        }
        Event::ProjectFileReadFailed { request_id, path, error } => {
            view.fail_text(request_id, path, error.clone())
        }
        _ => false,
    }
}

/// Keeps an open pane's file list in step with a refreshed workspace snapshot.
pub(crate) fn resync(app: &mut App) {
    if app.changes.is_none() {
        return;
    }
    let files = changed_files(app);
    if let Some(view) = app.changes.as_mut() {
        view.resync(files);
    }
}

/// Folds or unfolds one ELSEWHERE place.
///
/// Records the opposite of what is *showing* rather than flipping a stored flag, so the first
/// click always changes something: a place that folded itself because everything under it had been
/// seen has no stored choice to invert.
fn toggle_place(app: &mut App, place: String, showing_folded: bool) {
    app.external_folds.insert(place, !showing_folded);
}

/// Reduces one Changes-pane message.
pub(super) fn reduce(app: &mut App, msg: Msg) {
    match msg {
        Msg::OpenChanges(path) => open(app, path),
        Msg::OpenProjectFile(path) => open_project_file(app, path),
        Msg::TogglePeekView => toggle_view(app),
        Msg::StartPeekPick(line) => {
            if let Some(view) = app.changes.as_mut() {
                view.selection = match view.selection {
                    Some((anchor, head)) if anchor == line && head == line => None,
                    _ => Some((line, line)),
                };
                // The keyboard picks up where the pointer left off, out of visual mode
                view.cursor = Some(line);
                view.visual = false;
            }
        }
        Msg::ExtendPeekPick(head) => {
            if let Some((anchor, _)) = app.changes.as_ref().and_then(|view| view.selection) {
                if let Some(view) = app.changes.as_mut() {
                    view.selection = Some((anchor, head));
                    view.cursor = Some(head);
                }
            }
        }
        Msg::PeekSelectionAction(action) => selection_action(app, action),
        Msg::CloseChanges => close(app),
        Msg::ToggleExternalPlace(place) => {
            let folded = crate::view::external_place_folded(app, &place);
            toggle_place(app, place, folded);
        }
        Msg::ChangesStepFile(forward) => step_file(app, forward),
        Msg::ChangesOpenReview => open_review(app),
        Msg::ScrollChanges { up, lines } => {
            if let Some(view) = app.changes.as_mut() {
                view.scroll_by(up, lines);
            }
        }
        Msg::PageChanges(up) => {
            if let Some(view) = app.changes.as_mut() {
                view.page(up);
            }
        }
        Msg::JumpChanges(top) => {
            if let Some(view) = app.changes.as_mut() {
                view.jump(top);
            }
        }
        Msg::ToggleChangesWordWrap => {
            if let Some(view) = app.changes.as_mut() {
                view.toggle_word_wrap();
            }
        }
        Msg::ToggleChangesSplit => {
            if let Some(view) = app.changes.as_mut() {
                view.toggle_split();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::ChangesView;

    fn blank_app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default())
    }

    /// Escalating a peek to a review carries the file — unless no review could contain it.
    ///
    /// The chord is bound whether or not the footer advertises it, so the refusal has to live
    /// where the escalation is decided rather than in what the footer chose to draw.
    #[test]
    fn a_peek_outside_the_project_escalates_to_no_destination_at_all() {
        let mut app = blank_app();
        app.changes = Some(ChangesView::new("src/lib.rs".into(), Vec::new()));
        open_review(&mut app);
        assert_eq!(app.review.pending_file.as_deref(), Some("src/lib.rs"));

        let mut app = blank_app();
        app.changes = Some(ChangesView::new("/work/sdk/src/client.ts".into(), Vec::new()));
        open_review(&mut app);
        assert_eq!(app.review.pending_file, None, "a review cannot land on a file it has not got");
    }

    /// A look at a file elsewhere is recorded with the ± it carried; a look at a project file is
    /// recorded nowhere, because FILES counts what a *review* has covered.
    #[test]
    fn only_a_look_outside_the_project_is_remembered() {
        let mut app = blank_app();
        let files = vec![
            ChangedFile {
                path: "src/lib.rs".into(),
                status: luxe_core::FileChangeStatus::Modified,
                additions: Some(2),
                deletions: Some(1),
            },
            ChangedFile {
                path: "/work/sdk/src/client.ts".into(),
                status: luxe_core::FileChangeStatus::Modified,
                additions: Some(18),
                deletions: Some(3),
            },
        ];

        mark_seen(&mut app, &files, "src/lib.rs");
        assert!(app.external_seen.is_empty(), "a glance is not a review");

        mark_seen(&mut app, &files, "/work/sdk/src/client.ts");
        assert_eq!(app.external_seen.get("/work/sdk/src/client.ts"), Some(&(18, 3)));
    }
}
