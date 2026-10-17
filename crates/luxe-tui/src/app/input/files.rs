//! FILES keys — the tree, while the keyboard is in it.
//!
//! `Ctrl+T` puts the keyboard in the tree and `Esc` gives it back to the composer. In between the
//! keys are the ones every file tree answers to: `j`/`k` or the arrows move, `l` opens, `h` closes
//! or climbs, `Enter` shows a file, and a handful of letters act on the row. `l` on a file, or
//! `Tab` while one is showing, takes the keyboard on into the preview. A key the tree does
//! not know hands the keyboard back and goes to the composer, so typing a message never needs
//! leaving the tree first.
//!
//! `/` opens a filter over the whole project (see [`crate::files::filter`]). While it is being
//! typed every printable key goes into it and the arrows move through the matches; `Enter` shows
//! the file and stops the typing, so the letters are the tree's again, and `Esc` drops the filter.

use crate::app::{App, Msg};
use crate::files::TreeRow;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Answers whether the tree took `key`.
pub(super) fn files_took(app: &mut App, key: KeyEvent) -> bool {
    if !app.files.focused {
        return false;
    }
    if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        // A chord is the app's, not the tree's; the tree lets it through and keeps the keyboard
        return false;
    }
    if app.files.filter.as_ref().is_some_and(|filter| filter.typing) && typed_into_filter(app, key)
    {
        return true;
    }
    let tree = Tree::read(app);
    if tree.moves(app, key.code) || tree.opens(app, key.code) || tree.acts_on_path(app, key.code) {
        return true;
    }
    switch(app, key.code)
}

/// The tree's visible rows and where the cursor sits among them.
struct Tree {
    rows: Vec<TreeRow>,
    at: Option<usize>,
}

impl Tree {
    fn read(app: &App) -> Self {
        let changes = crate::app::messages::files::changed(app);
        let rows: Vec<TreeRow> =
            app.files.rows(&changes).iter().filter(|row| row.path().is_some()).cloned().collect();
        let at = app
            .files
            .cursor
            .as_deref()
            .and_then(|cursor| rows.iter().position(|row| row.path() == Some(cursor)));
        Self { rows, at }
    }

    fn row(&self) -> Option<&TreeRow> {
        self.at.and_then(|at| self.rows.get(at))
    }

    fn path(&self) -> Option<String> {
        self.row().and_then(TreeRow::path).map(str::to_string)
    }

    /// Puts the cursor on row `index`, held to the rows there are.
    fn place(&self, app: &mut App, index: usize) {
        let last = self.rows.len().saturating_sub(1);
        if let Some(path) = self.rows.get(index.min(last)).and_then(TreeRow::path) {
            app.files.cursor = Some(path.to_string());
            app.dock.follow_list(crate::workspace_dock::DockList::Files);
        }
    }

    /// The keys that move the cursor.
    fn moves(&self, app: &mut App, code: KeyCode) -> bool {
        let at = self.at;
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.place(app, at.map_or(0, |at| at + 1)),
            KeyCode::Up | KeyCode::Char('k') => {
                self.place(app, at.map_or(0, |at| at.saturating_sub(1)))
            }
            KeyCode::Home | KeyCode::Char('g') => self.place(app, 0),
            KeyCode::End | KeyCode::Char('G') => self.place(app, self.rows.len().saturating_sub(1)),
            KeyCode::PageDown => self.place(app, at.unwrap_or(0) + 10),
            KeyCode::PageUp => self.place(app, at.unwrap_or(0).saturating_sub(10)),
            _ => return false,
        }
        true
    }

    /// The keys that open, close or step into the row under the cursor.
    fn opens(&self, app: &mut App, code: KeyCode) -> bool {
        let at = self.at;
        match code {
            KeyCode::Right | KeyCode::Char('l') => match self.row() {
                Some(TreeRow::Folder { open: false, path, .. }) => {
                    app.update(Msg::ToggleFolder(path.clone()))
                }
                Some(TreeRow::Folder { open: true, .. }) => self.place(app, at.unwrap_or(0) + 1),
                Some(TreeRow::File { path, .. }) => {
                    app.update(Msg::OpenProjectFile(path.clone()));
                    app.update(Msg::FocusPeek);
                }
                _ => self.place(app, 0),
            },
            KeyCode::Left | KeyCode::Char('h') => self.step_out(app),
            KeyCode::Enter | KeyCode::Char(' ') => match self.row() {
                Some(TreeRow::Folder { path, .. }) => app.update(Msg::ToggleFolder(path.clone())),
                Some(TreeRow::File { path, .. }) => app.update(Msg::OpenProjectFile(path.clone())),
                _ => {}
            },
            _ => return false,
        }
        true
    }

    /// Closes an open folder, or moves up to the folder that holds this row.
    fn step_out(&self, app: &mut App) {
        match self.row() {
            Some(TreeRow::Folder { open: true, path, .. }) => {
                app.update(Msg::ToggleFolder(path.clone()))
            }
            Some(TreeRow::Folder { depth, .. } | TreeRow::File { depth, .. }) if *depth > 0 => {
                let parent = self.rows[..self.at.unwrap_or(0)].iter().rposition(|candidate| {
                    matches!(candidate, TreeRow::Folder { depth: above, .. } if above < depth)
                });
                if let Some(parent) = parent {
                    self.place(app, parent);
                }
            }
            _ => {}
        }
    }

    /// The keys that do something with the path under the cursor.
    fn acts_on_path(&self, app: &mut App, code: KeyCode) -> bool {
        if !matches!(
            code,
            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char('y') | KeyCode::Char('@')
        ) {
            return false;
        }
        let Some(path) = self.path() else { return true };
        match code {
            KeyCode::Char('o') => crate::app::commands::editor::open_project_path(app, &path),
            KeyCode::Char('O') => crate::app::commands::editor::open_in_file_manager(app, &path),
            KeyCode::Char('y') => app.update(Msg::CopyText(path)),
            _ => {
                app.editor.insert_reference(&format!("@[file:{path}]"));
                app.editor.insert_text(" ");
                app.files.focused = false;
            }
        }
        true
    }
}

/// The keys that change what the tree shows or who has the keyboard; anything else is the
/// composer's, and so is the keyboard from here.
fn switch(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('/') => open_filter(app),
        KeyCode::Esc if app.files.filter.is_some() => app.files.close_filter(),
        KeyCode::Char('a') => app.update(Msg::ToggleFilesScope),
        KeyCode::Char('i') => app.update(Msg::ToggleIgnoredFiles),
        KeyCode::Char('R') => {
            app.files.refresh_all();
            crate::app::messages::files::sync(app);
        }
        KeyCode::Tab | KeyCode::BackTab if app.changes.is_some() => app.update(Msg::FocusPeek),
        KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => app.files.focused = false,
        _ => {
            app.files.focused = false;
            return false;
        }
    }
    true
}

/// Opens the filter, with the project's file list as it stands and a fresher one on its way.
fn open_filter(app: &mut App) {
    app.files.set_project_files(app.file_provider.files());
    app.file_listing.request();
    app.files.open_filter();
}

/// A key typed while the filter is: answers whether the filter took it, leaving the arrows and
/// `Enter` to the tree's own handling.
fn typed_into_filter(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(character) => app.files.edit_filter(Some(character)),
        KeyCode::Backspace if app.files.filtering().is_none() => app.files.close_filter(),
        KeyCode::Backspace => app.files.edit_filter(None),
        KeyCode::Esc => {
            app.files.close_filter();
            return true;
        }
        KeyCode::Enter | KeyCode::Tab => {
            if let Some(filter) = app.files.filter.as_mut() {
                filter.typing = false;
            }
            // Enter goes on to show the file under the cursor, the way it does in the tree
            return key.code == KeyCode::Tab;
        }
        // The arrows move through the matches as they would through the tree
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => return false,
        // and nothing else typed into a filter leaks out into the composer
        _ => return true,
    }
    let changes = crate::app::messages::files::changed(app);
    if let Some(best) = app.files.hits(&changes).best.clone() {
        app.files.cursor = Some(best);
        app.dock.follow_list(crate::workspace_dock::DockList::Files);
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::app::Msg;
    use crate::fixture::{seeded_app, FixtureScenario};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut crate::app::App, code: KeyCode) {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn browsing() -> crate::app::App {
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
        app.file_provider = crate::ref_provider::FileRefProvider::with_files(
            ["src/view/dock.rs", "src/app.rs", "README.md"].map(String::from),
        );
        app.update(Msg::FocusFiles);
        app
    }

    /// `/` takes every letter, `j` and `q` included, into the filter rather than the tree or the
    /// composer, and puts the cursor on the best match as it goes.
    #[test]
    fn slash_types_a_filter_and_the_cursor_follows_the_best_match() {
        let mut app = browsing();
        press(&mut app, KeyCode::Char('/'));
        for character in "dock".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(app.files.filtering(), Some("dock"));
        assert_eq!(app.files.cursor.as_deref(), Some("src/view/dock.rs"));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Delete);
        assert_eq!(app.files.filtering(), Some("dockj"), "a letter is a letter while typing");
        assert!(app.files.focused && app.editor.current_input().is_empty(), "nothing leaked");
    }

    /// `Enter` shows the file and hands the letters back to the tree; `Esc` then drops the
    /// filter and keeps the cursor on the file found.
    #[test]
    fn enter_shows_the_match_and_esc_puts_the_tree_back_around_it() {
        let mut app = browsing();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.changes.as_ref().map(|view| view.path.as_str()), Some("src/view/dock.rs"));
        assert!(app.files.filter.as_ref().is_some_and(|filter| !filter.typing));

        press(&mut app, KeyCode::Esc);
        assert!(app.files.filter.is_none() && app.files.focused, "Esc clears; a second leaves");
        assert_eq!(app.files.cursor.as_deref(), Some("src/view/dock.rs"));
    }

    /// Backspace past the start of the query closes the filter, as it does in a search box.
    #[test]
    fn backspace_on_an_empty_filter_closes_it() {
        let mut app = browsing();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Backspace);
        assert!(app.files.filter.is_some());
        press(&mut app, KeyCode::Backspace);
        assert!(app.files.filter.is_none());
    }
}
