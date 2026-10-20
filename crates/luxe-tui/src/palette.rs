//! palette — the command palette: every command that can run here, searchable from anywhere.
//!
//! The state machine is pure — no rendering, no engine calls — and the app maps
//! [`PaletteOutcome::Dispatch`] to the command's effect. It opens on a [`Menu`] the app arranged:
//! the recently used first, then the alphabet. A query ranks the whole menu against each command's
//! name, slash trigger and keywords; the rule under the recent lead is drawn only while the query
//! is empty, because once you type, order is the matcher's.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::{Command, Menu, MenuEntry};

mod render;
pub use render::render_palette;

/// Outcome of a palette keypress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteOutcome {
    /// Dispatch this command and close the palette.
    Dispatch(Box<Command>),
    /// Close the palette without dispatching.
    Close,
    /// Key consumed but palette stays open.
    Consumed,
}

/// Pure state machine for the command palette.
#[derive(Clone, Debug)]
pub struct PaletteState {
    query: String,
    menu: Menu,
    /// Indices into `menu.entries` surviving the query, in rank order
    filtered: Vec<usize>,
    /// Which haystack characters the query matched, for each of `filtered`
    matched: Vec<Vec<usize>>,
    /// Cursor position in `filtered`
    selected: usize,
    open: bool,
}

impl PaletteState {
    /// Opens the palette on `menu`, unfiltered and on its first row.
    pub fn open(menu: Menu) -> Self {
        let filtered = (0..menu.entries.len()).collect();
        let matched = vec![Vec::new(); menu.entries.len()];
        Self { query: String::new(), menu, filtered, matched, selected: 0, open: true }
    }

    /// Handle a key event. Returns the outcome (dispatch/close/consumed).
    pub fn on_key(&mut self, key: &KeyEvent) -> PaletteOutcome {
        if !self.open {
            return PaletteOutcome::Consumed;
        }

        match key.code {
            KeyCode::Esc => PaletteOutcome::Close,
            KeyCode::Enter => self.accept().map_or(PaletteOutcome::Consumed, |command| {
                PaletteOutcome::Dispatch(Box::new(command))
            }),
            KeyCode::Up => {
                self.move_up();
                PaletteOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_down();
                PaletteOutcome::Consumed
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_up();
                PaletteOutcome::Consumed
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_down();
                PaletteOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.backspace();
                PaletteOutcome::Consumed
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                self.on_char(c);
                PaletteOutcome::Consumed
            }
            _ => PaletteOutcome::Consumed,
        }
    }

    /// Accept the currently selected command (or None if no match).
    pub fn accept(&self) -> Option<Command> {
        self.selected_entry().map(|entry| entry.command.clone())
    }

    /// The row under the cursor, with its key.
    pub fn selected_entry(&self) -> Option<&MenuEntry> {
        self.menu.entries.get(*self.filtered.get(self.selected)?)
    }

    /// Close the palette.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Check if the palette is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Get the current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Get the filtered items.
    pub fn filtered_items(&self) -> &[usize] {
        &self.filtered
    }

    /// The entry at filtered position `index`.
    pub fn entry_at(&self, index: usize) -> Option<&MenuEntry> {
        self.menu.entries.get(*self.filtered.get(index)?)
    }

    /// Get the currently selected index.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// How many entries the query leaves.
    pub(crate) fn match_count(&self) -> usize {
        self.filtered.len()
    }

    /// The filtered position the rule under the recent lead follows, while one is drawn.
    ///
    /// Only unfiltered, and only with something on both sides of it.
    pub fn divider(&self) -> Option<usize> {
        let recent = self.menu.recent;
        (self.query.is_empty() && recent > 0 && recent < self.filtered.len()).then_some(recent)
    }

    /// Moves the selection to `filtered_index` (a mouse click on that row); clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        if !self.filtered.is_empty() {
            self.selected = filtered_index.min(self.filtered.len().saturating_sub(1));
        }
    }

    /// Filters by pasted text, the way typing it would.
    pub fn paste(&mut self, text: &str) {
        let text = crate::editor::single_line_paste(text);
        if text.is_empty() {
            return;
        }
        self.query.push_str(&text);
        self.refilter();
    }

    fn on_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn refilter(&mut self) {
        let commands: Vec<&Command> =
            self.menu.entries.iter().map(|entry| &entry.command).collect();
        // A blank query keeps the menu's own order, which is the one it deliberately arranged
        (self.filtered, self.matched) =
            crate::commands::rank(&self.query, &commands).into_iter().unzip();
        self.selected = 0;
    }

    fn move_up(&mut self) {
        let len = self.filtered.len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 { len - 1 } else { self.selected - 1 };
    }

    fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.filtered.len();
    }

    /// Which characters of the haystack at filtered position `index` the query matched.
    fn matched_at(&self, index: usize) -> std::collections::HashSet<usize> {
        self.matched
            .get(index)
            .map(|positions| positions.iter().copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
