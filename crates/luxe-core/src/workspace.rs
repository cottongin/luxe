//! Workspace state — UI-neutral summaries for one opened project.
//!
//! The runtime collector owns project I/O and sends these bounded summaries to front-ends
//! through the core event stream. Rendering code must treat them as display state, not a
//! second source of truth.

mod tooling;

pub use tooling::*;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Opaque identifier for one project-open generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceScopeId(pub String);

impl WorkspaceScopeId {
    /// Creates a scope identifier from a runtime-generated value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// One coherent, revisioned project workspace summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Project-open generation that owns this revision sequence.
    pub scope_id: WorkspaceScopeId,
    /// Monotonic revision within [`WorkspaceSnapshot::scope_id`].
    pub revision: u64,
    /// Identity and lightweight Git state for the opened project.
    pub project: ProjectIdentity,
    /// Active session when one is open.
    pub session: Option<SessionSummary>,
    /// Bounded durable todo summary.
    pub todos: TodoSummary,
    /// Bounded working-tree change summary.
    pub changes: WorkspaceChangesSummary,
    /// Bounded summary of work this session did outside the project root.
    #[serde(default)]
    pub external: ExternalChangesSummary,
    /// Most recent review summary when available.
    pub review: Option<ReviewSummary>,
    /// Bounded roll-up of LSP diagnostics across recently touched files.
    pub diagnostics: DiagnosticsSummary,
    /// Merged language-server + debug-adapter status behind the `/lsp` and `/debug` views.
    pub tooling: ToolingSummary,
    /// The checked-out branch's pull request, as GitHub last described it
    #[serde(default)]
    pub pull_request: PullRequestPresence,
    /// Where the session's own worktree is, relative to the project, when it works in one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

impl WorkspaceSnapshot {
    /// Builds an empty snapshot for a newly opened project scope.
    pub fn empty(scope_id: WorkspaceScopeId, project: ProjectIdentity) -> Self {
        Self {
            scope_id,
            revision: 0,
            project,
            session: None,
            todos: TodoSummary::default(),
            changes: WorkspaceChangesSummary::default(),
            external: ExternalChangesSummary::default(),
            review: None,
            diagnostics: DiagnosticsSummary::default(),
            tooling: ToolingSummary::default(),
            pull_request: PullRequestPresence::default(),
            worktree: None,
        }
    }
}

/// What is known about the checked-out branch's pull request.
///
/// Read in the background by the workspace runtime, never by a render path. Everything but
/// [`PullRequestPresence::Found`] draws nothing on the bar, so a branch without a pull request, or
/// a machine without `gh`, looks like a bar with no pull request feature at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PullRequestPresence {
    /// Not asked yet
    #[default]
    Unknown,
    /// GitHub cannot be asked from here, and why, in one sentence
    Unavailable {
        /// Why, for `/pr` to say
        reason: String,
    },
    /// The branch has no pull request
    None,
    /// The branch's pull request
    Found(crate::github::PullRequestStatus),
}

/// Bounded LSP-diagnostics roll-up carried on the snapshot.
///
/// Aggregates the latest per-file diagnostics the runtime has observed (via the diagnostics-on-edit
/// hook) into workspace-wide severity counts plus a bounded, worst-first list of offending files.
///
/// **This covers only files edited during the session, on a project with a language server the
/// runtime could reach.** It is not a project-wide measurement, which is why the TUI draws no
/// section for it: on a surface read as fact, zero errors here would read as "the project is
/// clean". It stays on the snapshot because `Event::WorkspaceUpdated` is a published protocol and
/// an RPC front-end may present the counts with that caveat attached.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsSummary {
    /// Total error-severity diagnostics across all tracked files.
    pub errors: usize,
    /// Total warning-severity diagnostics.
    pub warnings: usize,
    /// Total information-severity diagnostics.
    pub infos: usize,
    /// Total hint-severity diagnostics.
    pub hints: usize,
    /// Bounded worst-first per-file rows (errors, then warnings).
    pub files: Vec<FileDiagnostics>,
    /// Exact number of files with diagnostics before the display bound is applied.
    pub total_files: usize,
    /// Whether rows were omitted from [`DiagnosticsSummary::files`].
    pub truncated: bool,
}

impl DiagnosticsSummary {
    /// Total diagnostics across every severity.
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.infos + self.hints
    }

    /// Whether any diagnostics are tracked.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Per-file diagnostic counts for [`DiagnosticsSummary::files`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiagnostics {
    /// Path relative to the project root.
    pub path: PathBuf,
    /// Error-severity diagnostics in this file.
    pub errors: usize,
    /// Warning-severity diagnostics in this file.
    pub warnings: usize,
    /// Information-severity diagnostics in this file.
    pub infos: usize,
    /// Hint-severity diagnostics in this file.
    pub hints: usize,
}

/// Stable identity and compact Git state for the opened project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    /// Absolute project root.
    pub root: PathBuf,
    /// Human-readable project name.
    pub display_name: String,
    /// Compact path suitable for display.
    pub display_path: String,
    /// Checked-out Git branch, if this is a Git project.
    pub git_branch: Option<String>,
    /// Whether the working tree has tracked or untracked changes.
    pub git_dirty: bool,
    /// Commit `HEAD` resolves to, when this is a Git project with any history.
    ///
    /// The only way to tell a commit from an undo. A working tree that empties looks identical
    /// either way in a snapshot, so a surface that reports "committed" off the emptying alone says
    /// it just as confidently when the agent created a scratch file and removed it again.
    #[serde(default)]
    pub git_head: Option<String>,
}

impl ProjectIdentity {
    /// Builds an identity for a non-Git project.
    pub fn non_git(
        root: PathBuf,
        display_name: impl Into<String>,
        display_path: impl Into<String>,
    ) -> Self {
        Self {
            root,
            display_name: display_name.into(),
            display_path: display_path.into(),
            git_branch: None,
            git_dirty: false,
            git_head: None,
        }
    }
}

/// Compact summary of the active session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: String,
    /// User-facing session title.
    pub title: String,
    /// Last activity timestamp in milliseconds since Unix epoch.
    pub updated_at_ms: u64,
}

/// Bounded summary of durable todos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSummary {
    /// Currently active todo, when one exists.
    pub current: Option<TodoSummaryItem>,
    /// Bounded todo rows.
    pub items: Vec<TodoSummaryItem>,
    /// Number of pending todos.
    pub pending: usize,
    /// Number of active todos.
    pub in_progress: usize,
    /// Number of completed todos.
    pub completed: usize,
    /// Number of blocked todos.
    pub blocked: usize,
    /// Exact total before the display bound is applied.
    pub total: usize,
    /// Whether rows were omitted from [`TodoSummary::items`].
    pub truncated: bool,
}

/// One display-safe durable todo row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSummaryItem {
    /// Todo identifier within its session.
    pub id: String,
    /// Bounded subject text.
    pub subject: String,
    /// Current display status.
    pub status: TodoSummaryStatus,
    /// Todo IDs that must complete before this row can proceed.
    pub blocked_by: Vec<String>,
}

/// Display state for a todo summary row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoSummaryStatus {
    /// Awaiting work.
    #[default]
    Pending,
    /// Being worked on.
    InProgress,
    /// Finished work.
    Completed,
    /// Waiting on another item.
    Blocked,
}

/// Bounded working-tree change summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceChangesSummary {
    /// Bounded changed-file rows.
    pub files: Vec<FileChangeSummary>,
    /// Exact changed-file total before the display bound is applied.
    pub total_files: usize,
    /// Text additions, or `None` when unavailable.
    pub additions: Option<u64>,
    /// Text deletions, or `None` when unavailable.
    pub deletions: Option<u64>,
    /// Whether rows were omitted from [`WorkspaceChangesSummary::files`].
    pub truncated: bool,
}

/// One changed file in the working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangeSummary {
    /// Path relative to the project root.
    pub path: PathBuf,
    /// File status reported by Git.
    pub status: FileChangeStatus,
    /// Text additions, or `None` for binary/unknown counts.
    pub additions: Option<u64>,
    /// Text deletions, or `None` for binary/unknown counts.
    pub deletions: Option<u64>,
    /// Whether Git reported this file as binary.
    pub binary: bool,
}

/// Bounded summary of the work a session did outside its project root.
///
/// Its own section rather than more rows in [`WorkspaceChangesSummary`], for two reasons that both
/// matter: those paths are documented project-relative and consumers rely on it, and the Changes
/// section leads to a review — which stays repository-scoped, so an external row must not offer to
/// open one it cannot contain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalChangesSummary {
    /// One group per external root, in stable path order.
    pub roots: Vec<ExternalRootSummary>,
    /// Exact changed-file total across every root, before display bounds.
    pub total_files: usize,
    /// Text additions across every root, or `None` when any count is unavailable.
    pub additions: Option<u64>,
    /// Text deletions across every root, or `None` when any count is unavailable.
    pub deletions: Option<u64>,
    /// Whether the underlying record stopped recording new paths.
    pub truncated: bool,
}

impl ExternalChangesSummary {
    /// Whether this session stayed inside its project.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

/// One place outside the project that a session changed something in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRootSummary {
    /// Absolute directory this group's paths are relative to.
    pub root: PathBuf,
    /// Compact path suitable for display.
    pub display_path: String,
    /// Checked-out branch when this root is a Git worktree.
    pub git_branch: Option<String>,
    /// Whether Git reports this root.
    ///
    /// A worktree's listing is everything Git sees there, including changes the shell made and
    /// changes that were already uncommitted — the same reading the project's own section gives.
    /// A non-Git group lists only the files Luxe's tools touched, which is all anything can know.
    pub git: bool,
    /// Bounded changed-file rows, relative to [`ExternalRootSummary::root`].
    pub files: Vec<FileChangeSummary>,
    /// Exact changed-file total for this root before the display bound.
    pub total_files: usize,
    /// Whether rows were omitted from [`ExternalRootSummary::files`].
    pub truncated: bool,
}

/// Git working-tree status for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    /// Newly added tracked file.
    Added,
    /// Existing file with changes.
    Modified,
    /// Deleted tracked file.
    Deleted,
    /// Renamed tracked file.
    Renamed,
    /// Untracked file.
    Untracked,
}

/// Compact generated review summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// Stable review identifier.
    pub id: String,
    /// User-facing review title.
    pub title: String,
    /// What the review compares against, e.g. `working tree` or `base origin/main`.
    #[serde(default)]
    pub source: String,
    /// Number of unresolved findings.
    pub unresolved_findings: usize,
    /// Completion timestamp in milliseconds since Unix epoch.
    pub completed_at_ms: u64,
}

#[cfg(test)]
mod tests;
