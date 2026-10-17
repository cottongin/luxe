//! GitHub panel — what `/github` lists, and the detail of the row under the cursor.
//!
//! The sections are GitHub searches the engine ran, and the details are overviews it fetched; the
//! panel only reads them. A list beside a detail, the layout the session picker and the palette
//! use: on the left each pull request or issue on two lines, on the right what stands in its way and
//! what was said about it. Below the dock's breakpoint the detail takes the list's place instead.
//!
//! Each tab is a search, and the line over its rows says what it searched for. `/` edits that
//! search and runs the tab again with it, `s` steps it through open, closed, merged and all, and
//! `x` puts the configured search back; an edit lasts the session. The cursor reaching the last row
//! loads the next page, so a tab of every pull request really is every one.
//!
//! The actions are still where the value is. Enter reviews a pull request or starts a session on an
//! issue, `c` drops a reference into the composer — the same one the `@` picker makes — and `o`
//! opens it on GitHub. Modal: every key is the panel's while it is open.

mod keys;
mod reads;
mod render;
mod rows;
mod search;

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::config::GithubSection;
use luxe_core::github::{ListItem, ListedSection};

use crate::github_detail::{DetailKey, GithubDetail};
use crate::glyphs::Icon;

pub use reads::OverviewRead;
pub use render::{render, section_icon};
pub use search::Pending;

/// Inner width at which the detail sits beside the list rather than taking its place: the list's
/// floor and a detail wide enough for a merge check and its key on one line.
const SPLIT_MIN_WIDTH: u16 = 124;

/// Rows PageUp and PageDown move by.
const PAGE_ROWS: usize = 10;

/// Where the keys go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Detail,
}

/// What `/github` shows.
#[derive(Debug, Clone, Default)]
pub struct GithubPanel {
    /// The sections, once the engine answered
    pub sections: Option<Vec<ListedSection>>,
    /// The checkout's repository, `owner/name`, once known
    pub repository: Option<String>,
    /// The section on screen
    pub section: usize,
    /// The row under the cursor, among the rows on screen
    pub selected: usize,
    /// Each tab's search as configured, which `x` puts back
    pub configured: Vec<GithubSection>,
    /// Each tab's search as it runs now: the configured one, or the reader's edit of it
    pub searches: Vec<GithubSection>,
    /// The search being typed, while `/` has the keys
    pub editing: Option<crate::editor::Editor>,
    /// Searches asked for on their own and not yet answered, by request id
    pub pending: HashMap<u64, Pending>,
    /// The details read so far, by address
    pub details: HashMap<String, GithubDetail>,
    /// Where the keys go
    pub focus: Focus,
    /// The clock ages are measured against, in seconds since the Unix epoch; the wall clock when
    /// unset, which is every time but a fixture's
    pub clock: Option<u64>,
    /// Each section's icon, in order
    pub icons: Vec<Icon>,
    /// The row under the cursor and when it got there; a read waits for it to settle
    pub settling: Option<(String, std::time::Instant)>,
    /// Rows whose overview has been asked for and not yet answered, by address
    pub asked: std::collections::HashSet<String>,
    /// Why a row's overview could not be read, by address
    pub failures: HashMap<String, String>,
}

/// What a key asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubKey {
    /// Close the panel
    Close,
    /// The panel handled it
    Held,
    /// Review this pull request; `repository` when it is not the checkout's
    Review {
        /// Its number
        number: u64,
        /// `owner/name` when elsewhere
        repository: Option<String>,
    },
    /// Start a session on this issue
    TakeUpIssue(u64),
    /// Put a reference to this row into the composer
    Refer(Box<ListItem>),
    /// Open this address
    Open(String),
    /// Ask GitHub again
    Refresh,
    /// Run one tab's search: its new query, or the next page of it
    Search {
        /// The id its answer comes back under
        request: u64,
        /// The tab's name and query, `{repo}` still in it
        section: GithubSection,
        /// The cursor the page starts after; `None` for a new search
        after: Option<String>,
    },
    /// Why the row's action cannot be taken here
    Refused(String),
    /// Leave this session for another of yours, by id
    Resume(String),
    /// Read a pull request's or issue's conversation
    ReadConversation {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
    },
    /// Post a reply or comment the reader confirmed
    Post(luxe_core::engine::GithubPost),
    /// Hand the draft being read to the reader's editor
    EditExternally(String),
    /// Have the agent draft a reply on a pull request or issue
    Draft {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// Whether it is a pull request, rather than an issue
        pull_request: bool,
        /// What to draft
        ask: crate::github_detail::DraftAsk,
    },
    /// Do what a status row on a pull request offers
    Act {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// The pull request, as its detail read it
        overview: Box<luxe_core::github::overview::PullRequestOverview>,
        /// What its row offers
        action: luxe_core::github::overview::CheckAction,
    },
    /// Merge a pull request as its confirmation was left
    Merge {
        /// `owner/name` when it is not the checkout's repository
        repository: Option<String>,
        /// Its number
        number: u64,
        /// The merge, or auto-merge turned on
        write: luxe_core::engine::PullRequestWrite,
    },
}

impl GithubPanel {
    /// A panel waiting on `configured`, each tab's icon read from its search, every tab searching
    /// what it was configured to.
    pub fn new(configured: &[GithubSection]) -> Self {
        Self::with_searches(configured, configured.to_vec())
    }

    /// A panel waiting on `searches`, the reader's edits of `configured` this session, one each.
    pub fn with_searches(configured: &[GithubSection], searches: Vec<GithubSection>) -> Self {
        let icons = configured.iter().map(|section| section_icon(&section.query)).collect();
        Self { icons, configured: configured.to_vec(), searches, ..Self::default() }
    }

    /// The rows of the section on screen, less those a search being typed already rules out; none
    /// while the section is being searched again.
    pub fn rows(&self) -> Vec<&ListItem> {
        let Some(section) = self.sections.as_ref().and_then(|sections| sections.get(self.section))
        else {
            return Vec::new();
        };
        if self.researching(self.section) {
            return Vec::new();
        }
        let wanted = self.preview();
        section
            .items
            .iter()
            .filter(|item| {
                let title = item.title.to_lowercase();
                let author = item.author.to_lowercase();
                let number = format!("#{}", item.number);
                wanted.iter().all(|word| {
                    title.contains(word) || author.contains(word) || number.contains(word)
                })
            })
            .collect()
    }

    fn elsewhere(&self, item: &ListItem) -> bool {
        self.repository.as_deref().is_some_and(|here| !here.eq_ignore_ascii_case(&item.repository))
    }

    /// The selected row's number, and its repository the way a request names one: `owner/name`
    /// when it is not the checkout's, `None` when it is. From the row itself, never by number,
    /// since a search across repositories can list two pull requests with one number.
    fn selected_target(&self) -> Option<(u64, Option<String>)> {
        let item = *self.rows().get(self.selected)?;
        Some((item.number, self.elsewhere(item).then(|| item.repository.clone())))
    }

    /// The detail of the row under the cursor, once read.
    pub fn detail(&self) -> Option<&GithubDetail> {
        self.rows().get(self.selected).and_then(|item| self.details.get(&item.url))
    }

    fn detail_mut(&mut self) -> Option<&mut GithubDetail> {
        let url = self.rows().get(self.selected).map(|item| item.url.clone())?;
        self.details.get_mut(&url)
    }

    fn now(&self) -> u64 {
        self.clock.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs())
        })
    }

    /// What `key` does. Every key is the panel's while it is open.
    pub fn on_key(&mut self, key: &KeyEvent) -> GithubKey {
        if self.focus == Focus::Detail {
            return self.detail_key_outcome(key);
        }
        self.list_key_outcome(key)
    }

    /// The hint along the bottom, which names what Enter does where the keys are. Keys drop off
    /// its end when the footer is narrow, so the ones a reader needs come first.
    pub fn hint(&self) -> String {
        if self.editing.is_some() {
            return "enter search GitHub · esc cancel".into();
        }
        if self.focus == Focus::Detail {
            if let Some(detail) = self.detail() {
                return detail.hint("← list");
            }
        }
        let details = if self.detail().is_some() { " · → details" } else { "" };
        let reset = if self.edited(self.section) { " · x reset" } else { "" };
        let search = format!("tab section · / search · s state{reset}");
        match self.rows().get(self.selected) {
            Some(item) if item.pull_request => {
                format!("enter review{details} · {search} · c reference · o open · esc close")
            }
            Some(_) => {
                let resume = self.detail().is_some_and(|detail| detail.local.is_some());
                let enter = if resume { "enter resume" } else { "enter start a session" };
                format!("{enter}{details} · {search} · c reference · o open · esc close")
            }
            None if self.sections.is_some() => format!("{search} · r refresh · esc close"),
            None => "esc close".into(),
        }
    }
}

#[cfg(test)]
mod tests;
