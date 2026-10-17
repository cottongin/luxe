//! Context section models — bounded display data derived from workspace snapshots.
//!
//! These values reduce typed runtime snapshots before rendering. They deliberately do not read
//! project state, inspect terminal geometry, or select a widget.

use luxe_core::{
    FileChangeStatus, FileChangeSummary, TodoSummaryItem, TodoSummaryStatus, WorkspaceSnapshot,
};

/// Display-safe data for Context's always-visible identity section.
///
/// Deliberately just project + branch. The session title is derived from the first user message, so
/// it told a reader nothing they didn't already know — it only crowded the identity block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockIdentity {
    /// Opened project display name.
    pub project_name: String,
    /// Git branch when the project is in a non-detached Git worktree.
    pub branch: Option<String>,
    /// Whether the working tree has tracked or untracked changes.
    pub dirty: bool,
    /// The branch's pull request, in the statusline's words, when it has one.
    pub pull_request: Option<String>,
}

impl DockIdentity {
    /// Derives identity rows from the current project-local workspace snapshot.
    pub fn from_snapshot(snapshot: &WorkspaceSnapshot) -> Self {
        Self {
            project_name: snapshot.project.display_name.clone(),
            branch: snapshot.project.git_branch.clone(),
            dirty: snapshot.project.git_dirty,
            pull_request: match &snapshot.pull_request {
                luxe_core::workspace::PullRequestPresence::Found(pr) => {
                    Some(crate::pull_request::label(pr))
                }
                _ => None,
            },
        }
    }
}

/// A display-safe todo row in Context's Plan section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockPlanItem {
    /// Bounded todo subject.
    pub subject: String,
    /// Current display status.
    pub status: TodoSummaryStatus,
}

impl From<&TodoSummaryItem> for DockPlanItem {
    fn from(item: &TodoSummaryItem) -> Self {
        Self { subject: item.subject.clone(), status: item.status }
    }
}

/// Bounded task progress for Context's optional Plan section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockPlan {
    /// Bounded snapshot rows. Completed rows stay available for expansion.
    pub items: Vec<DockPlanItem>,
    /// Number of pending rows.
    pub pending: usize,
    /// Number of in-progress rows.
    pub in_progress: usize,
    /// Number of completed rows.
    pub completed: usize,
    /// Exact total before snapshot row bounding.
    pub total: usize,
    /// Exact count not represented in [`DockPlan::items`], when bounded.
    pub remaining: Option<usize>,
}

impl DockPlan {
    /// Reduces a non-empty todo snapshot without reading the task store.
    pub fn from_snapshot(snapshot: &WorkspaceSnapshot) -> Option<Self> {
        let todos = &snapshot.todos;
        (todos.total > 0).then(|| Self {
            items: todos.items.iter().map(DockPlanItem::from).collect(),
            pending: todos.pending,
            in_progress: todos.in_progress,
            completed: todos.completed,
            total: todos.total,
            remaining: todos.truncated.then(|| todos.total.saturating_sub(todos.items.len())),
        })
    }
}

/// A display-safe file row in Context's Changes section.
///
/// A binary blob is not flagged: Git reports no counts for one, and the row renders that silence by
/// printing no counts at all rather than claiming `+0 −0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockChangeItem {
    /// Project-relative path, preserving the snapshot's stable source ordering.
    pub path: String,
    /// File lifecycle marker, also available without relying on color.
    pub status: FileChangeStatus,
    /// Text additions, or `None` when Git cannot provide them.
    pub additions: Option<u64>,
    /// Text deletions, or `None` when Git cannot provide them.
    pub deletions: Option<u64>,
}

impl From<&FileChangeSummary> for DockChangeItem {
    fn from(file: &FileChangeSummary) -> Self {
        Self {
            path: file.path.to_string_lossy().into_owned(),
            status: file.status,
            additions: file.additions,
            deletions: file.deletions,
        }
    }
}

/// Bounded working-tree progress for Context's optional Changes section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockChanges {
    /// Bounded files in their stable snapshot order.
    pub files: Vec<DockChangeItem>,
    /// Exact changed-file total before snapshot row bounding.
    pub total_files: usize,
    /// Aggregate additions, or `None` when any count is unknown.
    pub additions: Option<u64>,
    /// Aggregate deletions, or `None` when any count is unknown.
    pub deletions: Option<u64>,
    /// Exact count not represented in [`DockChanges::files`], when bounded.
    pub remaining: Option<usize>,
}

/// One directory's share of a change, for the dock's grouped view.
///
/// The heading renders as a bare path; the ± numbers are the group's share, which the files listed
/// under it also carry individually.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockChangeGroup {
    /// Directory the files share, or `.` for the project root.
    pub directory: String,
    /// Text additions across those files.
    pub additions: u64,
    /// Text deletions across those files.
    pub deletions: u64,
    /// Whether every file under it is unchanged since the reviewer last looked.
    pub reviewed: bool,
}

/// Splits a change entry into the directory that heads it and the name shown under that heading.
///
/// Git collapses an untracked *directory* into one entry with a trailing slash (`docs/`), which is
/// not a file: naively splitting on the last slash gave it a heading of its own and a child with
/// no name at all. Such an entry is a leaf, named for the directory it stands in for.
fn split_entry(path: &str) -> (String, String) {
    match path.strip_suffix('/') {
        Some(directory) => {
            let (parent, name) = directory
                .rsplit_once('/')
                .map_or((".", directory), |(parent, name)| (parent, name));
            (parent.to_string(), format!("{name}/"))
        }
        None => path.rsplit_once('/').map_or_else(
            || (".".to_string(), path.to_string()),
            |(head, tail)| (head.to_string(), tail.to_string()),
        ),
    }
}

/// The directory a project-relative entry sits in, or `.` at the root.
fn directory_of(path: &str) -> String {
    split_entry(path).0
}

/// One rendered row of the changes, grouped by folder.
///
/// A flat sequence rather than a nested structure, because the dock draws rows and bounds them by
/// count: keeping the shape flat means truncation is a `take`, not a tree walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockChangeRow {
    /// A directory heading, with the share of the change underneath it.
    Directory(DockChangeGroup),
    /// One file, indented under the directory above it.
    File {
        /// Name shown — the path relative to its directory heading.
        name: String,
        /// Full project-relative path, for eliding and for tests.
        path: String,
        /// Lifecycle marker (`M`, `A`, `D`, `R`, `?`).
        status: FileChangeStatus,
        /// Text additions, or `None` when Git cannot say (a binary blob).
        additions: Option<u64>,
        /// Text deletions, or `None` when Git cannot say.
        deletions: Option<u64>,
        /// Whether it is unchanged since the reviewer last looked.
        reviewed: bool,
    },
}

impl DockChanges {
    /// Reduces a non-empty change snapshot without querying Git.
    pub fn from_snapshot(snapshot: &WorkspaceSnapshot) -> Option<Self> {
        let changes = &snapshot.changes;
        (changes.total_files > 0).then(|| Self {
            files: changes.files.iter().map(DockChangeItem::from).collect(),
            total_files: changes.total_files,
            additions: changes.additions,
            deletions: changes.deletions,
            remaining: changes
                .truncated
                .then(|| changes.total_files.saturating_sub(changes.files.len())),
        })
    }

    /// Files not covered by the reviewer's last look at this project.
    ///
    /// A file counts as covered only when its ± numbers still match what they were when it was
    /// reviewed — a file touched again since is new work, whatever was read before.
    pub fn unreviewed(&self, seen: Option<&luxe_core::guided_review::SeenReview>) -> usize {
        let Some(seen) = seen.filter(|seen| !seen.is_empty()) else {
            return self.total_files;
        };
        self.files
            .iter()
            .filter(|file| {
                !seen.covers_file(
                    &file.path,
                    file.additions.unwrap_or_default(),
                    file.deletions.unwrap_or_default(),
                )
            })
            .count()
    }

    /// The change as a bounded tree: a directory heading, then the files under it.
    ///
    /// The dock is not navigable and this does not make it so — a static indented list is text,
    /// not a browser. Directories alone answered "where did it land" but withheld the one thing a
    /// glance actually wants, which is *which files*.
    ///
    /// Every directory keeps its heading, even over a single file. Collapsing it saves one row and
    /// costs the name: the file then has to carry the whole path, which in a forty-two cell rail
    /// elides to `crates/luxe-c…/src/usage.rs` — unreadable, where a heading over a bare
    /// `usage.rs` is not.
    ///
    /// `budget` bounds the rows; the renderer counts what its window leaves out rather than letting
    /// the tail drop silently.
    pub fn rows(
        &self,
        seen: Option<&luxe_core::guided_review::SeenReview>,
        budget: usize,
    ) -> Vec<DockChangeRow> {
        let mut rows = change_rows(&self.files, |file| {
            seen.is_some_and(|seen| {
                seen.covers_file(
                    &file.path,
                    file.additions.unwrap_or_default(),
                    file.deletions.unwrap_or_default(),
                )
            })
        });
        rows.truncate(budget);
        // Never end on a heading whose files were all cut: it promises rows that are not there.
        while matches!(rows.last(), Some(DockChangeRow::Directory(_))) {
            rows.pop();
        }
        rows
    }
}

/// Any set of changed files as a directory-grouped tree: a heading, then the files under it.
///
/// Shared by the changes list and by each ELSEWHERE place, so a folder reads the same wherever the rail
/// draws one. `reviewed` answers whether a file is unchanged since the reader last looked, which
/// the two sections decide differently — a review covers the project, a peek covers one file
/// somewhere else.
pub fn change_rows(
    files: &[DockChangeItem],
    reviewed: impl Fn(&DockChangeItem) -> bool,
) -> Vec<DockChangeRow> {
    let mut rows = Vec::new();
    for group in directory_groups(files, &reviewed) {
        let mut under: Vec<&DockChangeItem> =
            files.iter().filter(|file| directory_of(&file.path) == group.directory).collect();
        under.sort_by(|left, right| alphabetical(&left.path, &right.path));
        rows.push(DockChangeRow::Directory(group));
        rows.extend(under.into_iter().map(|file| DockChangeRow::File {
            name: split_entry(&file.path).1,
            path: file.path.clone(),
            status: file.status,
            additions: file.additions,
            deletions: file.deletions,
            reviewed: reviewed(file),
        }));
    }
    rows
}

/// Two entries in the order a reader would file them: A→Z, ignoring case.
///
/// Case-insensitive first because a byte comparison files `README.md` under a different letter
/// than `readme.md`, which is not what anyone means by alphabetical. The raw comparison breaks the
/// tie so the order is total and a redraw never shuffles two names that differ only in case.
fn alphabetical(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase()).then_with(|| left.cmp(right))
}

/// The change grouped by directory, A→Z.
///
/// Grouping survives truncation where a flat list does not — eight left-cut paths all lose the
/// crate name, which is the part that tells them apart.
///
/// Alphabetical rather than ranked, and deliberately so. The rail is redrawn on every tool call, so
/// any ordering derived from the work — largest share first, unreviewed first — rearranges itself
/// underneath a reader who is trying to follow one file: the row they were watching moves each time
/// its counts move, and moves again the moment they review it. A name does not change, so the list
/// a person learns on their first look is the list they get on their twentieth.
fn directory_groups(
    files: &[DockChangeItem],
    reviewed: &impl Fn(&DockChangeItem) -> bool,
) -> Vec<DockChangeGroup> {
    let mut groups: std::collections::BTreeMap<String, DockChangeGroup> =
        std::collections::BTreeMap::new();
    for file in files {
        let directory = directory_of(&file.path);
        let group = groups.entry(directory.clone()).or_insert(DockChangeGroup {
            directory,
            additions: 0,
            deletions: 0,
            reviewed: true,
        });
        group.additions += file.additions.unwrap_or_default();
        group.deletions += file.deletions.unwrap_or_default();
        group.reviewed &= reviewed(file);
    }
    let mut groups: Vec<DockChangeGroup> = groups.into_values().collect();
    groups.sort_by(|left, right| alphabetical(&left.directory, &right.directory));
    groups
}

/// One place outside the project this session changed something in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockExternalGroup {
    /// Where it is on disk, which is what a peek at a file under it is asked with.
    pub root: std::path::PathBuf,
    /// Where it is, written the way a person would say it.
    pub location: String,
    /// Branch, when the place is a Git worktree.
    pub branch: Option<String>,
    /// Whether Git reports this place, which decides what the listing can claim to cover.
    pub git: bool,
    /// Files under it, in the snapshot's stable order.
    pub files: Vec<DockChangeItem>,
    /// Exact count not represented in [`DockExternalGroup::files`], when bounded.
    pub remaining: Option<usize>,
}

impl DockExternalGroup {
    /// The path a peek at the file at `path` — relative to this place — is asked with.
    ///
    /// Absolute, which is what tells every surface it crosses — the click target, the engine's
    /// capture, the pane's header — that this file is not in the project and cannot be answered
    /// for by the project's Git.
    pub fn peek(&self, path: &str) -> String {
        luxe_core::paths::display(&self.root.join(path))
    }

    /// The same path, for a file this place is already holding.
    pub fn peek_path(&self, file: &DockChangeItem) -> String {
        self.peek(&file.path)
    }
}

/// Work this session did outside the project, for Context's optional Elsewhere section.
///
/// Kept apart from [`DockChanges`] because the two sections make different promises. Changes is
/// backed by the project's Git worktree and leads to a review; this is backed by whatever witness
/// each place happens to have and leads nowhere, because a review of the project cannot contain a
/// file that is not in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockExternal {
    /// Places, in the snapshot's stable order.
    pub groups: Vec<DockExternalGroup>,
    /// Exact changed-file total across every place.
    pub total_files: usize,
}

impl DockExternal {
    /// Reduces the snapshot's external section, or `None` when the session stayed home.
    pub fn from_snapshot(snapshot: &WorkspaceSnapshot) -> Option<Self> {
        let external = &snapshot.external;
        (!external.is_empty()).then(|| Self {
            groups: external
                .roots
                .iter()
                .map(|root| DockExternalGroup {
                    root: root.root.clone(),
                    location: root.display_path.clone(),
                    branch: root.git_branch.clone(),
                    git: root.git,
                    files: root.files.iter().map(DockChangeItem::from).collect(),
                    remaining: root
                        .truncated
                        .then(|| root.total_files.saturating_sub(root.files.len())),
                })
                .collect(),
            total_files: external.total_files,
        })
    }
}

/// Snapshot-backed data for Context's required and actionable sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockSectionModel {
    /// Stable project/session/task identity.
    pub identity: DockIdentity,
    /// Bounded task progress when durable todos exist.
    pub plan: Option<DockPlan>,
    /// Bounded working-tree progress when files have changed.
    pub changes: Option<DockChanges>,
    /// Work done outside the project, when the session has done any.
    pub external: Option<DockExternal>,
}

impl DockSectionModel {
    /// Builds bounded section data without filesystem or Git access.
    pub fn from_snapshot(snapshot: &WorkspaceSnapshot) -> Self {
        Self {
            identity: DockIdentity::from_snapshot(snapshot),
            plan: DockPlan::from_snapshot(snapshot),
            changes: DockChanges::from_snapshot(snapshot),
            external: DockExternal::from_snapshot(snapshot),
        }
    }
}

#[cfg(test)]
mod tests;
