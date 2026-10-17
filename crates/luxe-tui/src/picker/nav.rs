//! Picker navigation — the cursor, the scroll and the tabs a picker moves through.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Shared navigation state: the query, the indices surviving the current filter, and the cursor.
///
/// The owner computes *which* indices survive (its filtering is domain-specific) and hands them to
/// [`PickerNav::apply_filter`]; everything else — wrapping movement, mouse selection, and the
/// selected-item lookup — is uniform.
#[derive(Clone, Debug, Default)]
pub struct PickerNav {
    query: String,
    filtered_indices: Vec<usize>,
    selected: usize,
}

impl PickerNav {
    /// Start with every one of `total` items visible and the first selected.
    pub fn new(total: usize) -> Self {
        Self { query: String::new(), filtered_indices: (0..total).collect(), selected: 0 }
    }

    /// Store a new query + its resulting filtered index set, resetting the cursor to the top.
    pub fn apply_filter(&mut self, query: &str, filtered_indices: Vec<usize>) {
        self.query.clear();
        self.query.push_str(query);
        self.filtered_indices = filtered_indices;
        self.selected = 0;
    }

    /// Move the cursor up one row, wrapping to the bottom.
    pub fn move_up(&mut self) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 { len - 1 } else { self.selected - 1 };
    }

    /// Move the cursor down one row, wrapping to the top.
    pub fn move_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.filtered_indices.len();
    }

    /// Move the cursor to a clicked row (a filtered position), clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        if !self.filtered_indices.is_empty() {
            self.selected = filtered_index.min(self.filtered_indices.len().saturating_sub(1));
        }
    }

    /// The current query text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The filtered index set (positions into the owner's item list).
    pub fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }

    /// The cursor position within the filtered set.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The owner-item index under the cursor, if any (`filtered_indices[selected]`).
    pub fn selected_item_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Number of rows surviving the current filter.
    pub fn match_count(&self) -> usize {
        self.filtered_indices.len()
    }

    /// The owner-item index at filtered row `index` — what a mouse click on that row selected.
    pub fn item_index_at(&self, index: usize) -> Option<usize> {
        self.filtered_indices.get(index).copied()
    }

    /// Re-ranks against the current query and returns the cursor to the top.
    ///
    /// `haystacks[i]` is the searchable text for item `i` — usually its visible label plus whatever
    /// else a reader would reasonably type at it (a path, a provider prefix). A blank query
    /// restores the owner's original order, which is the one it deliberately sorted.
    pub fn refilter(&mut self, haystacks: &[impl AsRef<str>]) {
        let query = self.query.trim();
        self.filtered_indices = if query.is_empty() {
            (0..haystacks.len()).collect()
        } else {
            let borrowed: Vec<&str> = haystacks.iter().map(AsRef::as_ref).collect();
            crate::fuzzy::rank(query, &borrowed).into_iter().map(|matched| matched.idx).collect()
        };
        self.selected = 0;
    }

    /// Replaces the query wholesale and re-ranks.
    pub fn set_query(&mut self, query: &str, haystacks: &[impl AsRef<str>]) {
        self.query.clear();
        self.query.push_str(query);
        self.refilter(haystacks);
    }

    /// Appends pasted text to the query and re-ranks.
    ///
    /// A search box is a text field, so it takes a paste like any other — reduced to one line,
    /// because a query is one line.
    pub fn paste_query(&mut self, text: &str, haystacks: &[impl AsRef<str>]) {
        let text = crate::editor::single_line_paste(text);
        if text.is_empty() {
            return;
        }
        self.query.push_str(&text);
        self.refilter(haystacks);
    }

    /// Applies a query-editing key and re-ranks: Backspace deletes, Ctrl+U clears, a character
    /// typed with or without Shift appends. `true` when the key was one of those. `haystacks` is
    /// built only when the query changed, and the re-rank happens here, so no caller can edit the
    /// query and forget to re-filter.
    pub fn edit_query<Haystacks, Text>(&mut self, event: &KeyEvent, haystacks: Haystacks) -> bool
    where
        Haystacks: FnOnce() -> Vec<Text>,
        Text: AsRef<str>,
    {
        match event.code {
            KeyCode::Backspace if event.modifiers.is_empty() => {
                self.query.pop();
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
            }
            KeyCode::Char(c) if (event.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.query.push(c)
            }
            _ => return false,
        }
        self.refilter(&haystacks());
        true
    }
}
