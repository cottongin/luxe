//! Open in editor — `/editor`, and the key that opens the file a diff is showing.
//!
//! Each asks the main loop to hand the terminal to the reader's editor
//! ([`crate::launcher::open_in_editor`]), which takes it back when the editor exits. The engine
//! keeps running underneath, so opening a file mid-turn costs the turn nothing.

use std::path::{Path, PathBuf};

use super::super::App;
use crate::launcher::OpenInEditor;

/// `/editor [path[:line]]`: the project with no argument, or that file or folder.
pub(in crate::app) fn editor_command(app: &mut App, args: Option<&str>) {
    let root = project_root(app);
    let home = luxe_core::user_dirs::home_dir();
    match target_for(&root, home.as_deref(), args) {
        Ok(target) => app.outbox.editor_open = Some(target),
        Err(message) => app.status.flash_toast(message),
    }
}

/// Opens the file the preview is showing, at the top line in view.
pub(in crate::app) fn open_peeked_file(app: &mut App) {
    let Some(view) = app.changes.as_ref() else {
        return;
    };
    let line = crate::view::changes::line_in_view(view);
    let path = Path::new(&view.path);
    let path = match path.is_absolute() {
        true => path.to_path_buf(),
        false => project_root(app).join(path),
    };
    open_existing(app, path, line);
}

/// Opens the file the review's cursor is in, at the cursor's line.
///
/// The working tree's copy, which is what an editor can change. For a review of a commit or of a
/// pull request that is not checked out, that may differ from the diff on screen, and a file the
/// change deleted is not there at all, which is said rather than opened empty.
pub(in crate::app) fn open_reviewed_file(app: &mut App) {
    let Some(review) = app.guided_review.as_ref() else {
        return;
    };
    let Some((file, line)) = app.guided_review_workspace.selected_new_line(&review.files) else {
        return;
    };
    let Some(path) = file.new_path.clone() else {
        app.status.flash_toast("This change deleted the file, so there is nothing to open");
        return;
    };
    let path = project_root(app).join(path);
    open_existing(app, path, line);
}

/// Opens a project-relative file or folder from the FILES tree.
pub(in crate::app) fn open_project_path(app: &mut App, relative: &str) {
    let path = project_root(app).join(relative);
    open_existing(app, path, None);
}

/// Opens a project-relative folder, or the folder holding a file, in a file manager.
///
/// A terminal one takes the screen the way the editor does; with none installed, the desktop's own
/// opens beside the terminal. A file manager can move and delete what the tree shows, so the tree
/// reads its folders again when the screen comes back.
pub(in crate::app) fn open_in_file_manager(app: &mut App, relative: &str) {
    let root = project_root(app);
    let Some(folder) = folder_for(&root, relative) else {
        app.status.flash_toast(format!("{relative} is not in the working tree"));
        return;
    };
    match luxe_core::program::file_manager() {
        Some((program, args)) => {
            app.outbox.file_manager =
                Some(crate::launcher::OpenInFileManager { program, args, folder });
        }
        None => {
            let label = luxe_core::paths::display(&folder);
            app.open_externally(folder.as_os_str(), &label);
        }
    }
}

/// The folder a file manager opens on for a tree row: the folder itself, or the one holding a
/// file. `None` when it is not on disk.
fn folder_for(root: &Path, relative: &str) -> Option<PathBuf> {
    let path = root.join(relative);
    let folder = match path.is_dir() {
        true => path,
        false => path.parent()?.to_path_buf(),
    };
    folder.is_dir().then_some(folder)
}

fn open_existing(app: &mut App, path: PathBuf, line: Option<u32>) {
    if !path.exists() {
        let shown =
            luxe_core::paths::display(path.strip_prefix(project_root(app)).unwrap_or(&path));
        app.status.flash_toast(format!("{shown} is not in the working tree"));
        return;
    }
    app.outbox.editor_open = Some(OpenInEditor { path, line });
}

fn project_root(app: &App) -> PathBuf {
    app.project_root.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// What `/editor`'s argument names, resolved against the project.
///
/// Relative paths are the project's, `~/` is home, and a trailing `:N` is a line when the path
/// without it is a file. A path that does not exist yet is fine when its folder does — the editor
/// makes it on save, which is how a new file starts — and an error otherwise, since a typo in a
/// folder name opens nothing useful.
///
/// Args: `(project root, home, argument)`.
fn target_for(
    root: &Path,
    home: Option<&Path>,
    args: Option<&str>,
) -> Result<OpenInEditor, String> {
    let written = args.map(str::trim).filter(|args| !args.is_empty());
    let Some(written) = written else {
        return Ok(OpenInEditor { path: root.to_path_buf(), line: None });
    };
    let unquoted = written
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| written.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))
        .unwrap_or(written);
    let resolve = |text: &str| -> PathBuf {
        let expanded = match (text.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ if text == "~" => home.map_or_else(|| PathBuf::from(text), Path::to_path_buf),
            _ => PathBuf::from(text),
        };
        match expanded.is_absolute() {
            true => expanded,
            false => root.join(expanded),
        }
    };
    let path = resolve(unquoted);
    if path.exists() {
        return Ok(OpenInEditor { path, line: None });
    }
    if let Some((file, line)) = unquoted.rsplit_once(':') {
        if let Ok(line) = line.parse::<u32>() {
            let file = resolve(file);
            if file.is_file() {
                return Ok(OpenInEditor { path: file, line: Some(line) });
            }
        }
    }
    match path.parent().is_some_and(Path::is_dir) {
        true => Ok(OpenInEditor { path, line: None }),
        false => Err(format!("No such folder for {unquoted}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/install-dev.sh"), "#!/bin/sh\n").unwrap();
        dir
    }

    /// A file manager opens on a folder: the row's own, or the one holding the file, and nothing
    /// for a row that left the disk.
    #[test]
    fn a_file_manager_opens_on_the_folder_a_row_is_in() {
        let dir = project();
        let root = dir.path();
        assert_eq!(folder_for(root, "scripts"), Some(root.join("scripts")));
        assert_eq!(folder_for(root, "scripts/install-dev.sh"), Some(root.join("scripts")));
        assert_eq!(folder_for(root, "README.md"), Some(root.to_path_buf()), "a root file");
        assert_eq!(folder_for(root, "gone/file.rs"), None);
    }

    #[test]
    fn bare_editor_opens_the_project() {
        let dir = project();
        let target = target_for(dir.path(), None, None).unwrap();
        assert_eq!(target, OpenInEditor { path: dir.path().to_path_buf(), line: None });
        assert_eq!(target_for(dir.path(), None, Some("   ")).unwrap().path, dir.path());
    }

    #[test]
    fn a_path_is_the_projects_and_may_name_a_line() {
        let dir = project();
        let script = dir.path().join("scripts/install-dev.sh");
        let open = |args: &str| target_for(dir.path(), None, Some(args));
        assert_eq!(open("scripts/install-dev.sh").unwrap().path, script);
        assert_eq!(open("\"scripts/install-dev.sh\"").unwrap().path, script, "quoted");
        assert_eq!(open("scripts").unwrap().path, dir.path().join("scripts"), "a folder");
        assert_eq!(
            open("scripts/install-dev.sh:12").unwrap(),
            OpenInEditor { path: script.clone(), line: Some(12) }
        );
        assert_eq!(open(&script.display().to_string()).unwrap().path, script, "absolute");
    }

    #[test]
    fn home_is_tilde() {
        let dir = project();
        let home = dir.path().join("scripts");
        let target = target_for(Path::new("/elsewhere"), Some(&home), Some("~/install-dev.sh"));
        assert_eq!(target.unwrap().path, home.join("install-dev.sh"));
    }

    /// A new file in a folder that exists is how a file starts; a folder that does not exist is a
    /// typo, and the editor would open nothing useful.
    #[test]
    fn a_new_file_needs_its_folder() {
        let dir = project();
        let new = target_for(dir.path(), None, Some("scripts/new.sh")).unwrap();
        assert_eq!(new.path, dir.path().join("scripts/new.sh"));
        let typo = target_for(dir.path(), None, Some("scirpts/new.sh"));
        assert_eq!(typo, Err("No such folder for scirpts/new.sh".to_string()));
    }
}
