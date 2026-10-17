//! Mention flyout — the `@` picker, over files, skills, GitHub, MCP servers and the reader's sources.
//!
//! Opens when `@` is typed and filters as you type. Tab and Shift+Tab move between its tabs — the
//! project's files, the skills installed, the pull requests and issues `/github` lists, the MCP
//! servers this session can reach when it has any, then every source in `mentions` — and Enter
//! inserts the row under the cursor as a reference. A tab that lists from elsewhere
//! asks the first time it opens and keeps the answer; one whose answer was a failure asks again
//! next time, since fixing the failure is usually what happened in between.

mod entry;
mod render;

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::config::MentionSource;
use luxe_core::github::ListItem;
use luxe_core::mention_source::MentionRow;

use crate::mentions::MentionRef;
use crate::ref_provider::{Candidate, FileRefProvider, RefProvider};
use entry::{entry_candidate, entry_words};
pub use render::render_mention_flyout;

/// One of the picker's tabs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionTab {
    /// The project's files
    Files,
    /// The skills installed, global and the project's
    Skills,
    /// Pull requests and issues on GitHub
    GitHub,
    /// The MCP servers this session can reach
    Mcp,
    /// A source from `mentions`
    Source(MentionSource),
}

impl MentionTab {
    /// The tab's name.
    fn name(&self) -> &str {
        match self {
            Self::Files => "Files",
            Self::Skills => "Skills",
            Self::GitHub => "GitHub",
            Self::Mcp => "MCP",
            Self::Source(source) => source.name.as_str(),
        }
    }
}

/// A row from a tab that lists from elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Skill(luxe_core::skills::SkillDoc),
    GitHub(ListItem),
    Mcp(luxe_core::McpServerStatus),
    Row(MentionRow),
}

/// What a tab that lists from elsewhere answered.
#[derive(Clone, Debug, Default)]
struct Listed {
    entries: Vec<Entry>,
    /// Why it listed nothing, when that is why: said in the tab rather than an empty list that
    /// reads as "nothing matches"
    error: Option<String>,
}

/// State for the mention flyout.
#[derive(Clone, Debug)]
pub struct MentionFlyoutState {
    /// Candidates from the tab on screen, already filtered for the current query.
    items: Vec<Candidate>,
    /// The row behind each candidate, when the tab lists from elsewhere
    shown: Vec<Entry>,
    /// Shared query + filtered-index + cursor state (same nav as the `/` picker).
    nav: crate::picker::PickerNav,
    /// The file provider (for refreshing candidates).
    provider: FileRefProvider,
    /// Repository root (for expanding references).
    repo_root: std::path::PathBuf,
    /// Files, skills, GitHub, MCP when there are servers, then the reader's sources
    tabs: Vec<MentionTab>,
    /// The tab on screen
    open: usize,
    /// Each tab's answer, once it has one; files never need one
    listed: Vec<Option<Listed>>,
    /// Whether a changed file shows its Git status beside it. Off: kept for a fixture that
    /// weighs the look before it ships
    show_status: bool,
}

impl MentionFlyoutState {
    /// Create a new flyout on the files tab, with a tab for each usable source after GitHub.
    ///
    /// Args: `(provider, repo_root, sources, skills, servers)` — the skills installed and the MCP
    /// servers from the latest snapshot, both listed at once: files on this machine and a status
    /// the runtime already read, so neither costs a request. The MCP tab exists only when a server
    /// does, and a server whose id is not [`luxe_core::mcp_client::is_plain_name`] is not listed.
    pub fn new(
        provider: FileRefProvider,
        repo_root: std::path::PathBuf,
        sources: &[MentionSource],
        skills: Vec<luxe_core::skills::SkillDoc>,
        servers: &[luxe_core::McpServerStatus],
    ) -> Self {
        let items = provider.candidates("", &repo_root);
        let nav = crate::picker::PickerNav::new(items.len());
        let mut tabs = vec![MentionTab::Files, MentionTab::Skills, MentionTab::GitHub];
        let servers: Vec<Entry> = servers
            .iter()
            .filter(|server| server.enabled && luxe_core::mcp_client::is_plain_name(&server.id))
            .cloned()
            .map(Entry::Mcp)
            .collect();
        if !servers.is_empty() {
            tabs.push(MentionTab::Mcp);
        }
        for source in sources.iter().filter(|source| source.usable()) {
            // Two sources claiming one kind would make their references ambiguous: first wins
            if !tabs
                .iter()
                .any(|tab| matches!(tab, MentionTab::Source(s) if s.kind() == source.kind()))
            {
                tabs.push(MentionTab::Source(source.clone()));
            }
        }
        let mut listed = vec![None; tabs.len()];
        listed[1] =
            Some(Listed { entries: skills.into_iter().map(Entry::Skill).collect(), error: None });
        if let Some(index) = tabs.iter().position(|tab| *tab == MentionTab::Mcp) {
            listed[index] = Some(Listed { entries: servers, error: None });
        }
        Self {
            items,
            shown: Vec::new(),
            nav,
            provider,
            repo_root,
            tabs,
            open: 0,
            listed,
            show_status: false,
        }
    }

    /// Handles picker navigation while leaving text editing to the composer.
    pub fn on_key(&mut self, key: &KeyEvent) -> MentionFlyoutOutcome {
        use crossterm::event::KeyModifiers;

        match key.code {
            KeyCode::Esc => MentionFlyoutOutcome::Close,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                MentionFlyoutOutcome::Edit
            }
            KeyCode::Tab => self.switch(1),
            KeyCode::BackTab => self.switch(self.tabs.len() - 1),
            KeyCode::Enter => match (self.nav.selected_item_index(), &self.tabs[self.open]) {
                (Some(idx), MentionTab::Files) => {
                    let candidate = &self.items[idx];
                    MentionFlyoutOutcome::Insert(MentionRef {
                        kind: crate::mentions::RefKind::File,
                        target: candidate.insert.clone(),
                        span: 0..candidate.insert.len(),
                    })
                }
                (Some(idx), tab) => match (&self.shown[idx], tab) {
                    (Entry::Skill(skill), _) => {
                        MentionFlyoutOutcome::InsertSkill(skill.name.clone())
                    }
                    (Entry::GitHub(item), _) => {
                        MentionFlyoutOutcome::InsertGithub(Box::new(item.clone()))
                    }
                    (Entry::Mcp(server), _) => MentionFlyoutOutcome::InsertMcp(server.id.clone()),
                    (Entry::Row(row), MentionTab::Source(source)) => {
                        MentionFlyoutOutcome::InsertRow(Box::new((source.clone(), row.clone())))
                    }
                    (Entry::Row(_), _) => MentionFlyoutOutcome::Consumed,
                },
                (None, _) => MentionFlyoutOutcome::Consumed,
            },
            KeyCode::Up => {
                self.nav.move_up();
                MentionFlyoutOutcome::Consumed
            }
            KeyCode::Down => {
                self.nav.move_down();
                MentionFlyoutOutcome::Consumed
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.nav.move_up();
                MentionFlyoutOutcome::Consumed
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.nav.move_down();
                MentionFlyoutOutcome::Consumed
            }
            KeyCode::Char('r')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.tabs[self.open] == MentionTab::Files =>
            {
                MentionFlyoutOutcome::RefreshFiles
            }
            _ => MentionFlyoutOutcome::Edit,
        }
    }

    /// Takes a fresh read of the project's files, keeping the cursor on the file it was on.
    ///
    /// Arrives a moment after the picker opens, and every few seconds while it stays open, so the
    /// list a reader moves through can change under them; holding the row they were on is what
    /// keeps that from reading as the cursor jumping.
    pub fn set_files(&mut self, provider: FileRefProvider) {
        self.provider = provider;
        if self.tabs[self.open] != MentionTab::Files {
            return;
        }
        let held = self.selected_candidate().map(|candidate| candidate.insert.clone());
        let query = self.nav.query().to_owned();
        self.set_query(&query);
        let found = held.and_then(|held| self.items.iter().position(|item| item.insert == held));
        if let Some(position) = found {
            self.nav.select(position);
        }
    }

    /// Moves `step` tabs on, wrapping, and asks for the new tab's rows when it has none.
    fn switch(&mut self, step: usize) -> MentionFlyoutOutcome {
        self.open = (self.open + step) % self.tabs.len();
        // A failed answer is forgotten, so the tab asks again
        if self.listed[self.open].as_ref().is_some_and(|listed| listed.error.is_some()) {
            self.listed[self.open] = None;
        }
        let query = self.nav.query().to_owned();
        self.set_query(&query);
        match (&self.tabs[self.open], &self.listed[self.open]) {
            (MentionTab::Files, _) | (_, Some(_)) => MentionFlyoutOutcome::Consumed,
            (tab, None) => MentionFlyoutOutcome::Fetch(tab.clone()),
        }
    }

    /// Replaces the query after the composer edits its source buffer, refreshing candidates.
    pub fn set_query(&mut self, query: &str) {
        match &self.tabs[self.open] {
            // The provider does the matching and returns candidates already in display order
            MentionTab::Files => {
                self.items = self.provider.candidates(query, &self.repo_root);
                self.shown.clear();
            }
            _ => {
                let entries =
                    self.listed[self.open].as_ref().map_or(&[][..], |listed| &listed.entries[..]);
                let words: Vec<String> = entries.iter().map(entry_words).collect();
                let order: Vec<(usize, Vec<usize>)> = if query.is_empty() {
                    (0..entries.len()).map(|index| (index, Vec::new())).collect()
                } else {
                    let haystack: Vec<&str> = words.iter().map(String::as_str).collect();
                    crate::fuzzy::rank(query, &haystack)
                        .into_iter()
                        .map(|found| (found.idx, found.positions))
                        .collect()
                };
                self.shown = order.iter().map(|(index, _)| entries[*index].clone()).collect();
                self.items = order
                    .iter()
                    .map(|(index, positions)| entry_candidate(&entries[*index], positions))
                    .collect();
            }
        }
        let filtered = (0..self.items.len()).collect();
        self.nav.apply_filter(query, filtered);
    }

    /// Takes GitHub's rows, once listed, and shows them if their tab is open.
    ///
    /// Args: `(rows, error)` — the reason GitHub could not be listed, when that is why there are
    /// no rows.
    pub fn set_github(&mut self, rows: Vec<ListItem>, error: Option<String>) {
        let entries = rows.into_iter().map(Entry::GitHub).collect();
        self.set_listed(|tab| *tab == MentionTab::GitHub, entries, error);
    }

    /// Takes a source's rows, or why it listed none, and shows them if its tab is open.
    pub fn set_source(&mut self, kind: &str, answer: Result<Vec<MentionRow>, String>) {
        let (rows, error) = match answer {
            Ok(rows) => (rows, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let entries = rows.into_iter().map(Entry::Row).collect();
        let is_it = |tab: &MentionTab| matches!(tab, MentionTab::Source(s) if s.kind() == kind);
        self.set_listed(is_it, entries, error);
    }

    fn set_listed(
        &mut self,
        is_it: impl Fn(&MentionTab) -> bool,
        entries: Vec<Entry>,
        error: Option<String>,
    ) {
        let Some(index) = self.tabs.iter().position(is_it) else {
            return;
        };
        let error = error.filter(|_| entries.is_empty());
        self.listed[index] = Some(Listed { entries, error });
        if index == self.open {
            let query = self.nav.query().to_owned();
            self.set_query(&query);
        }
    }

    /// Shows a changed file's Git status beside it, the way the Context Dock marks it.
    pub fn show_status(&mut self) {
        self.show_status = true;
    }

    /// The tab on screen.
    pub fn tab(&self) -> &MentionTab {
        &self.tabs[self.open]
    }

    /// Whether the tab on screen is waiting on its rows.
    pub fn fetching(&self) -> bool {
        self.tabs[self.open] != MentionTab::Files && self.listed[self.open].is_none()
    }

    /// Moves the selection to `filtered_index` (a mouse click on that row); clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        self.nav.select(filtered_index);
    }

    /// Get the current query.
    pub fn query(&self) -> &str {
        self.nav.query()
    }

    /// Get the filtered items.
    pub fn filtered_items(&self) -> &[usize] {
        self.nav.filtered_indices()
    }

    /// Get the selected candidate (if any).
    pub fn selected_candidate(&self) -> Option<&Candidate> {
        self.items.get(self.nav.selected_item_index()?)
    }
}

/// Outcome of a mention flyout keypress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionFlyoutOutcome {
    /// Insert this file reference and close the flyout.
    Insert(MentionRef),
    /// Insert a reference to the skill of this name and close the flyout.
    InsertSkill(String),
    /// Insert a reference to this GitHub row and close the flyout.
    InsertGithub(Box<ListItem>),
    /// Insert a reference to the MCP server with this id and close the flyout.
    InsertMcp(String),
    /// Insert a reference to this row of a source and close the flyout.
    InsertRow(Box<(MentionSource, MentionRow)>),
    /// A tab opened with nothing listed yet: ask for its rows.
    Fetch(MentionTab),
    /// Read the project's files again, for a file made since the list was read.
    RefreshFiles,
    /// Close without inserting.
    Close,
    /// Key consumed but flyout stays open.
    Consumed,
    /// Let the composer edit the source buffer, then refresh the query.
    Edit,
}

#[cfg(test)]
mod tests;
