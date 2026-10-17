//! Searches — each tab's query as the reader edits it, and the pages that answer it.
//!
//! The rewriting is `luxe_core::github::query`'s, so a scoped tab stays in the checkout's repository
//! whatever is typed. Here is only where the keys go and where the answers land: an answer is
//! matched by the id it was asked under, and one for a search since replaced is dropped rather than
//! put over the newer one.

use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::github::{query, ListedSection};

use super::{GithubKey, GithubPanel};

/// Request ids, unique for the process: a panel closed and opened again must not take an answer
/// meant for the last one's search.
static REQUESTS: AtomicU64 = AtomicU64::new(1);

/// A search asked for on its own and not yet answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// The tab it fills
    pub section: usize,
    /// Whether it brings the next page, rather than replacing the tab's rows
    pub page: bool,
}

impl GithubPanel {
    /// The tab's search as the line over its rows shows it, without the scope it never says.
    pub fn shown_query(&self, section: usize) -> String {
        self.searches.get(section).map(|search| query::shown(&search.query)).unwrap_or_default()
    }

    /// Whether tab `section` searches something other than what it was configured to.
    pub fn edited(&self, section: usize) -> bool {
        match (self.searches.get(section), self.configured.get(section)) {
            (Some(search), Some(configured)) => search.query != configured.query,
            _ => false,
        }
    }

    /// Whether tab `section` is being searched again, its old rows already gone.
    pub fn researching(&self, section: usize) -> bool {
        self.pending.values().any(|pending| pending.section == section && !pending.page)
    }

    /// Whether tab `section` is waiting on its next page.
    pub fn paging(&self, section: usize) -> bool {
        self.pending.values().any(|pending| pending.section == section && pending.page)
    }

    /// Whether anything asked of GitHub on its own is still out.
    pub fn searching(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The words typed beyond the tab's own search, lowercased: what the loaded rows are narrowed
    /// by while a search is being typed. Qualifiers wait for GitHub.
    pub(super) fn preview(&self) -> Vec<String> {
        let Some(editor) = &self.editing else {
            return Vec::new();
        };
        let shown = self.shown_query(self.section).to_lowercase();
        let had: Vec<&str> = shown.split_whitespace().collect();
        editor
            .current_input()
            .to_lowercase()
            .split_whitespace()
            .filter(|word| !word.contains(':') && !word.starts_with('-') && !had.contains(word))
            .map(str::to_owned)
            .collect()
    }

    pub(super) fn start_editing(&mut self) {
        let mut editor = crate::editor::Editor::new();
        let shown = self.shown_query(self.section);
        // A space after, so a word typed straight away is a word of its own
        editor.insert_text(&if shown.is_empty() { shown } else { format!("{shown} ") });
        self.editing = Some(editor);
    }

    /// A pasted line, into the search being typed; whether there was one to take it.
    pub fn paste(&mut self, text: &str) -> bool {
        let Some(editor) = self.editing.as_mut() else {
            return false;
        };
        editor.insert_text(&crate::editor::single_line_paste(text));
        self.selected = 0;
        true
    }

    pub(super) fn on_editing_key(&mut self, key: &KeyEvent) -> GithubKey {
        match key.code {
            KeyCode::Esc => {
                self.editing = None;
                self.selected = 0;
                GithubKey::Held
            }
            KeyCode::Enter => {
                let typed = self.editing.take().map(|editor| editor.current_input().to_owned());
                self.selected = 0;
                let typed = typed.unwrap_or_default();
                let configured = self.configured.get(self.section).map(|c| c.query.clone());
                let Some(configured) = configured else {
                    return GithubKey::Held;
                };
                // Emptied is the way back to the tab's own search
                let wanted = match typed.trim() {
                    "" => configured,
                    typed => query::edited(&configured, typed),
                };
                self.run(self.section, wanted)
            }
            _ => {
                if let Some(editor) = self.editing.as_mut() {
                    editor.handle_key(*key);
                }
                self.selected = 0;
                GithubKey::Held
            }
        }
    }

    /// Steps the tab on screen to the next state: open, closed, merged, all.
    pub(super) fn cycle_state(&mut self) -> GithubKey {
        let Some(current) = self.searches.get(self.section).map(|s| s.query.clone()) else {
            return GithubKey::Held;
        };
        let wanted = query::with_state(&current, query::next_state(&current));
        self.run(self.section, wanted)
    }

    /// Puts the tab on screen back to its configured search.
    pub(super) fn reset(&mut self) -> GithubKey {
        let Some(configured) = self.configured.get(self.section).map(|c| c.query.clone()) else {
            return GithubKey::Held;
        };
        self.run(self.section, configured)
    }

    /// Runs tab `section` with `wanted`, unless that is what it already ran. Whatever was still out
    /// for the tab is forgotten, so its answer cannot land over this one's.
    fn run(&mut self, section: usize, wanted: String) -> GithubKey {
        let Some(search) = self.searches.get_mut(section) else {
            return GithubKey::Held;
        };
        if search.query == wanted {
            return GithubKey::Held;
        }
        search.query = wanted;
        let search = search.clone();
        self.pending.retain(|_, pending| pending.section != section);
        let request = REQUESTS.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(request, super::Pending { section, page: false });
        GithubKey::Search { request, section: search, after: None }
    }

    /// The next page of the tab on screen, once the cursor is on its last row and there is one.
    pub(super) fn next_page_if_due(&mut self) -> GithubKey {
        let rows = self.rows().len();
        if rows == 0 || self.selected + 1 < rows {
            return GithubKey::Held;
        }
        if self.paging(self.section) {
            return GithubKey::Held;
        }
        let next = self
            .sections
            .as_ref()
            .and_then(|sections| sections.get(self.section))
            .and_then(|section| section.next.clone());
        let (Some(after), Some(search)) = (next, self.searches.get(self.section).cloned()) else {
            return GithubKey::Held;
        };
        let request = REQUESTS.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(request, Pending { section: self.section, page: true });
        GithubKey::Search { request, section: search, after: Some(after) }
    }

    /// Takes back a search the engine never got, so the tab is not left waiting on it.
    pub fn unasked(&mut self, request: u64) {
        self.pending.remove(&request);
    }

    /// Lands a search's answer in the tab that asked: a new search's rows in place of the old, or a
    /// page after them. Answers why a page failed, for the reader to be told, since its tab keeps
    /// the rows it has; an answer to a search since replaced is dropped.
    pub fn land_search(
        &mut self,
        request: u64,
        repository: Option<String>,
        found: ListedSection,
    ) -> Option<String> {
        let pending = self.pending.remove(&request)?;
        let section = self.sections.as_mut()?.get_mut(pending.section)?;
        if repository.is_some() {
            self.repository = repository;
        }
        if !pending.page {
            // The tab keeps its own name, whatever the answer echoes
            *section = ListedSection { name: std::mem::take(&mut section.name), ..found };
            if pending.section == self.section {
                self.selected = 0;
            }
            return None;
        }
        if let Some(error) = found.error {
            section.next = None;
            return Some(error);
        }
        for item in found.items {
            if !section.items.iter().any(|kept| kept.url == item.url) {
                section.items.push(item);
            }
        }
        section.total = found.total.or(section.total);
        section.next = found.next;
        None
    }
}
