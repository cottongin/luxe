//! Files — the project tree the Context Dock's FILES section draws.
//!
//! What changed comes from the workspace snapshot and the rest of the project from core, one folder
//! at a time (`Action::ListDirectory` → `Event::DirectoryListed`), so the tree never walks the disk
//! itself and never lists a folder nobody opened. Folders on the way to a changed file start open:
//! the tree took CHANGES' place, and the first thing it has to answer is still *what did it touch*.
//!
//! A reader's own opening and closing outlive the snapshot, so a folder closed by hand stays closed
//! however many changes land under it, and one opened by hand stays open after its changes are
//! committed.

pub(crate) mod filter;
mod listings;
mod projection;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use luxe_core::project_files::{DirectoryListing, EntryKind, ListingRequestId};

use crate::workspace_dock::DockChangeItem;

pub use filter::{FilterHits, TreeFilter};

/// What is known about one folder's entries.
#[derive(Debug, Clone, Default)]
struct Listing {
    /// The entries last listed, kept on screen while a fresher listing is on its way
    entries: Option<DirectoryListing>,
    /// The request a fresher listing will answer
    request: Option<ListingRequestId>,
    /// Why the last request failed, when it did
    failed: Option<String>,
    /// Whether the entries have been overtaken by a change and want asking for again
    stale: bool,
}

/// The tree's state: what has been listed, and what the reader opened, closed and chose to see.
#[derive(Debug, Clone, Default)]
pub struct FilesTree {
    listings: HashMap<String, Listing>,
    opened: HashSet<String>,
    closed: HashSet<String>,
    /// Only what changed, as the CHANGES section lists it
    pub changed_only: bool,
    /// What `.gitignore` ignores too, drawn faint
    pub show_ignored: bool,
    /// The row the keyboard is on, by path
    pub cursor: Option<String>,
    /// Whether the keyboard is in the tree rather than the composer
    pub focused: bool,
    /// What `/` narrowed the tree to, while it is open
    pub filter: Option<TreeFilter>,
    /// The whole project's files, which a filter matches against
    project_files: Arc<Vec<String>>,
    /// What the changes looked like when the listings were last refreshed
    changes_seen: u64,
    /// Bumped by every change to what the tree would draw, so the rows can be kept between frames
    generation: u64,
    /// The rows last drawn, what a filter matched in them, and what they were drawn from
    memo: std::cell::RefCell<Option<Memo>>,
}

/// The rows last drawn, kept between frames.
#[derive(Debug, Clone)]
struct Memo {
    key: u64,
    rows: Arc<Vec<TreeRow>>,
    hits: Arc<FilterHits>,
}

/// One row of the drawn tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// A folder, open or closed.
    Folder {
        /// Project-relative path
        path: String,
        /// Its own name
        name: String,
        /// How deep it sits under the project root
        depth: usize,
        /// Whether its entries are drawn under it
        open: bool,
        /// Whether a changed file sits anywhere under it
        holds_change: bool,
        /// Whether `.gitignore` ignores it
        ignored: bool,
    },
    /// A file, with what git says about it.
    File {
        /// Project-relative path
        path: String,
        /// Its own name
        name: String,
        /// How deep it sits under the project root
        depth: usize,
        /// Its change, when it has one
        change: Option<DockChangeItem>,
        /// Whether `.gitignore` ignores it
        ignored: bool,
    },
    /// What stands in for a folder's entries: still listing, could not be listed, or cut short.
    Note {
        /// Depth of the entries it stands in for
        depth: usize,
        /// What to say
        text: String,
    },
}

impl TreeRow {
    /// The row's path, for a folder or a file.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Folder { path, .. } | Self::File { path, .. } => Some(path),
            Self::Note { .. } => None,
        }
    }
}

/// A child of one folder, before it becomes a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    Folder,
    File,
}

impl FilesTree {
    /// The rows to draw, top to bottom.
    ///
    /// Until the project root is listed, and whenever the reader asked for changes only, the tree
    /// is built from the changes alone: a section that went blank while a folder was being read
    /// would hide exactly what CHANGES shows at once.
    ///
    /// Kept between frames: the dock draws every frame, and the tree only moves when a folder
    /// opens, a listing lands or the changes do.
    pub fn rows(&self, changes: &[DockChangeItem]) -> Arc<Vec<TreeRow>> {
        self.drawn(changes).rows
    }

    /// What the filter matched in the rows, for lighting the matched characters; empty while
    /// nothing is being filtered.
    pub fn hits(&self, changes: &[DockChangeItem]) -> Arc<FilterHits> {
        self.drawn(changes).hits
    }

    fn drawn(&self, changes: &[DockChangeItem]) -> Memo {
        let key = self.key(changes);
        if let Some(memo) = self.memo.borrow().as_ref().filter(|memo| memo.key == key) {
            return memo.clone();
        }
        let (rows, hits) = match self.filtering() {
            Some(query) => filter::build(query, &self.project_files, changes, &ancestors(changes)),
            None => (self.build(changes), FilterHits::default()),
        };
        let memo = Memo { key, rows: Arc::new(rows), hits: Arc::new(hits) };
        *self.memo.borrow_mut() = Some(memo.clone());
        memo
    }

    /// The query narrowing the tree, when one has been typed.
    pub fn filtering(&self) -> Option<&str> {
        self.filter.as_ref().map(|filter| filter.query.as_str()).filter(|query| !query.is_empty())
    }

    /// Opens the filter, or goes back to typing into the one that is open.
    pub fn open_filter(&mut self) {
        self.generation += 1;
        let filter = self.filter.get_or_insert_with(TreeFilter::default);
        filter.typing = true;
    }

    /// Closes the filter, and opens the way to the cursor's row in the whole tree, so the file
    /// found is still under the cursor once everything else is back.
    pub fn close_filter(&mut self) {
        self.generation += 1;
        self.filter = None;
        if let Some(cursor) = self.cursor.clone() {
            self.reveal(&cursor);
        }
    }

    /// Changes the query: a character typed, or the last one taken back when `typed` is `None`.
    pub fn edit_filter(&mut self, typed: Option<char>) {
        self.generation += 1;
        if let Some(filter) = self.filter.as_mut() {
            match typed {
                Some(character) => filter.query.push(character),
                None => {
                    filter.query.pop();
                }
            }
        }
    }

    /// Takes the project's file list, the one the `@` picker reads.
    pub fn set_project_files(&mut self, files: &[String]) {
        if self.project_files.as_slice() != files {
            self.generation += 1;
            self.project_files = Arc::new(files.to_vec());
        }
    }

    /// What the rows are drawn from, as one number.
    fn key(&self, changes: &[DockChangeItem]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.generation.hash(&mut hasher);
        self.changed_only.hash(&mut hasher);
        self.show_ignored.hash(&mut hasher);
        self.filtering().hash(&mut hasher);
        for item in changes {
            item.path.hash(&mut hasher);
            item.additions.hash(&mut hasher);
            item.deletions.hash(&mut hasher);
            (item.status as u8).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Whether `folder` is drawn open: the reader's word first, then the way to a change.
    fn is_open(&self, folder: &str, ancestors: &HashSet<String>, from_changes: bool) -> bool {
        if self.closed.contains(folder) {
            return false;
        }
        from_changes || self.opened.contains(folder) || ancestors.contains(folder)
    }

    /// The row standing in for a folder whose entries are not here.
    fn note_for(&self, folder: &str, depth: usize) -> TreeRow {
        let text = match self.listings.get(folder).and_then(|listing| listing.failed.as_ref()) {
            Some(error) => format!("could not list: {error}"),
            None => "reading…".to_string(),
        };
        TreeRow::Note { depth, text }
    }

    fn listed(&self, folder: &str) -> Option<&DirectoryListing> {
        self.listings.get(folder).and_then(|listing| listing.entries.as_ref())
    }

    /// Opens a closed folder or closes an open one, as it is drawn now.
    pub fn toggle(&mut self, folder: &str, changes: &[DockChangeItem]) {
        self.generation += 1;
        let showing = self
            .rows(changes)
            .iter()
            .any(|row| matches!(row, TreeRow::Folder { path, open: true, .. } if path == folder));
        if showing {
            self.opened.remove(folder);
            self.closed.insert(folder.to_string());
        } else {
            self.closed.remove(folder);
            self.opened.insert(folder.to_string());
        }
        // after the change, since telling which way to flip drew the rows as they were
        self.generation += 1;
    }

    /// Opens every folder on the way to `path`, so the row a preview is showing is in view.
    pub fn reveal(&mut self, path: &str) {
        self.generation += 1;
        let mut folder = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(part);
            self.closed.remove(&folder);
            self.opened.insert(folder.clone());
        }
    }
}

/// Every folder on the way to a changed file.
fn ancestors(changes: &[DockChangeItem]) -> HashSet<String> {
    let mut folders = HashSet::new();
    for item in changes {
        let mut end = 0;
        while let Some(slash) = item.path[end..].find('/') {
            end += slash;
            folders.insert(item.path[..end].to_string());
            end += 1;
        }
    }
    folders
}

#[cfg(test)]
mod tests;
