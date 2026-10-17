//! Reference providers — what the `@` picker's Files tab lists, and in what order.
//!
//! The list is read in process through the walk the search tools share, so it honors `.gitignore`
//! the way they do, and is kept sorted, with forward slashes on every platform. Ranking is
//! path-aware: a match in the filename outranks the same match spread through the folders, and
//! hidden files are findable but never lead.

use std::path::Path;

use luxe_core::workspace::FileChangeStatus;

use crate::fuzzy;

/// A candidate reference from a provider.
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    /// Display label (e.g., "src/main.rs").
    pub label: String,
    /// Insertion text (what gets put in the message).
    pub insert: String,
    /// Optional detail (e.g., file size, symbol kind).
    pub detail: Option<String>,
    /// Characters of `label` the query matched, by char index
    pub label_matched: Vec<usize>,
    /// Characters of `detail` the query matched, by char index
    pub detail_matched: Vec<usize>,
    /// Whether `label` is a path, drawn with its folder quiet and trimmed first when short of room
    pub path: bool,
    /// A changed file's Git status, for a surface that shows it
    pub status: Option<FileChangeStatus>,
}

/// Trait for reference providers.
pub trait RefProvider {
    /// The kind of references this provider yields.
    fn kind(&self) -> crate::mentions::RefKind;

    /// Get candidates filtered by query.
    fn candidates(&self, query: &str, repo_root: &Path) -> Vec<Candidate>;
}

/// The most rows the Files tab offers for one query; the picker scrolls a window over them.
const MAX_CANDIDATES: usize = 50;

/// What a match wholly inside the filename gains over one that reaches into the folders: about two
/// matched characters' worth, enough to lift `src/main.rs` over `domain/maintenance.rs` for
/// `main` without letting a scattered filename match beat a tight path match.
const FILENAME_BONUS: i64 = 32;

/// The project's files, and which of them are being worked on.
#[derive(Clone, Debug, Default)]
pub struct FileRefProvider {
    /// Relative paths, forward slashes, sorted
    files: Vec<String>,
    /// Whether each file sits under a hidden folder or is a dotfile, alongside `files`
    hidden: Vec<bool>,
    /// Changed files in the working tree, as the workspace snapshot reported them
    working: Vec<(String, FileChangeStatus)>,
    /// A list given rather than read, which a refresh must not replace
    given: bool,
}

impl FileRefProvider {
    /// Create a new file provider and enumerate files.
    pub fn new(repo_root: &Path) -> Self {
        let mut provider = Self::default();
        provider.refresh(repo_root);
        provider
    }

    /// Creates a provider with no project files for deterministic fixtures.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A provider over exactly `files`, for fixtures and tests that must not depend on a checkout.
    pub fn with_files(files: impl IntoIterator<Item = String>) -> Self {
        let mut provider = Self { given: true, ..Self::default() };
        provider.set_files(files.into_iter().collect());
        provider
    }

    /// Refresh the file list.
    pub fn refresh(&mut self, repo_root: &Path) {
        self.set_files(list_files(repo_root));
    }

    /// Takes a list read elsewhere ([`list_files`], off the render thread), unless this provider's
    /// list was given rather than read.
    ///
    /// Read again off the render thread whenever the picker opens, since a file created or moved
    /// outside Luxe never produces a workspace snapshot and would stay impossible to mention.
    pub fn set_listing(&mut self, files: Vec<String>) {
        if !self.given {
            self.set_files(files);
        }
    }

    /// The project's files, relative, forward slashes, sorted.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Takes the changed files the workspace snapshot reported, listed first with no query.
    pub fn set_working(&mut self, changes: &[luxe_core::workspace::FileChangeSummary]) {
        self.working = changes
            .iter()
            .filter(|change| change.status != FileChangeStatus::Deleted)
            .map(|change| (luxe_core::paths::display(&change.path), change.status))
            .collect();
    }

    fn set_files(&mut self, mut files: Vec<String>) {
        files.sort();
        files.dedup();
        self.hidden = files.iter().map(|file| is_hidden(file)).collect();
        self.files = files;
    }

    fn status_of(&self, path: &str) -> Option<FileChangeStatus> {
        self.working.iter().find(|(changed, _)| changed == path).map(|(_, status)| *status)
    }

    fn candidate(&self, index: usize, label_matched: Vec<usize>) -> Candidate {
        let path = &self.files[index];
        Candidate {
            label: path.clone(),
            insert: path.clone(),
            label_matched,
            path: true,
            status: self.status_of(path),
            ..Default::default()
        }
    }

    /// With no query: the files being worked on, then the rest alphabetically, hidden ones left
    /// out. `.` sorts before every letter, which would open the list on `.cargo/`, `.config/` and
    /// `.github/` before a single file of the project itself.
    fn unfiltered(&self) -> Vec<Candidate> {
        let working = self
            .working
            .iter()
            .filter_map(|(changed, _)| self.files.iter().position(|file| file == changed));
        let rest = (0..self.files.len())
            .filter(|&index| !self.hidden[index] && self.status_of(&self.files[index]).is_none());
        working
            .chain(rest)
            .take(MAX_CANDIDATES)
            .map(|index| self.candidate(index, Vec::new()))
            .collect()
    }

    /// Best first: the matcher's path-aware score, plus [`FILENAME_BONUS`] when the whole match is
    /// in the filename; then a visible file before a hidden one — unless the query starts with
    /// `.`, which is asking for them — then the shorter path, then alphabetical.
    fn ranked(&self, query: &str) -> Vec<Candidate> {
        let paths: Vec<&str> = self.files.iter().map(String::as_str).collect();
        let asks_for_hidden = query.starts_with('.');
        let mut found: Vec<(fuzzy::Match, i64, bool)> = fuzzy::rank_paths(query, &paths)
            .into_iter()
            .map(|matched| {
                let path = paths[matched.idx];
                let filename_from =
                    path.rfind('/').map_or(0, |slash| path[..=slash].chars().count());
                let in_filename = matched.positions.iter().all(|&at| at >= filename_from);
                let score = matched.score + if in_filename { FILENAME_BONUS } else { 0 };
                let demoted = self.hidden[matched.idx] && !asks_for_hidden;
                (matched, score, demoted)
            })
            .collect();
        found.sort_by(|(left, left_score, left_demoted), (right, right_score, right_demoted)| {
            right_score
                .cmp(left_score)
                .then(left_demoted.cmp(right_demoted))
                .then(paths[left.idx].len().cmp(&paths[right.idx].len()))
                .then(paths[left.idx].cmp(paths[right.idx]))
        });
        found
            .into_iter()
            .take(MAX_CANDIDATES)
            .map(|(matched, ..)| self.candidate(matched.idx, matched.positions))
            .collect()
    }
}

impl RefProvider for FileRefProvider {
    fn kind(&self) -> crate::mentions::RefKind {
        crate::mentions::RefKind::File
    }

    fn candidates(&self, query: &str, _repo_root: &Path) -> Vec<Candidate> {
        match query.is_empty() {
            true => self.unfiltered(),
            false => self.ranked(query),
        }
    }
}

/// Whether any part of `path` is hidden: a dotfile, or anything under a dot-folder.
fn is_hidden(path: &str) -> bool {
    path.split('/').any(|part| part.starts_with('.'))
}

/// Folders never worth listing, ignored or not.
const SKIPPED: [&str; 4] = [".git", "target", "node_modules", ".cache"];

/// Every file under `repo_root` that `.gitignore` does not exclude, dotfiles included, relative and
/// with forward slashes: what the Files tab lists.
///
/// Through the walk the search tools share ([`luxe_core::walk::listing`]), in process, so the tab
/// agrees with them about what `.gitignore` excludes, needs no program on the machine, and stops at
/// the same ceiling on a tree too big to list. A large tree takes a while, so a refresh calls this
/// off the render thread and hands the result to [`FileRefProvider::set_listing`].
pub fn list_files(repo_root: &Path) -> Vec<String> {
    let walked = luxe_core::walk::listing(repo_root, &SKIPPED);
    // Once: the list is re-read every few seconds while the picker is open
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if walked.capped && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(root = %repo_root.display(), "the file list stopped at its ceiling");
    }
    walked.files.iter().map(|path| luxe_core::paths::display(path)).collect()
}

/// Expand a mention reference into attached context.
pub fn expand_reference(r#ref: &crate::mentions::MentionRef, repo_root: &Path) -> Option<String> {
    match &r#ref.kind {
        crate::mentions::RefKind::File => {
            let path = repo_root.join(&r#ref.target);
            std::fs::read_to_string(&path).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
