//! Guided Review loading — typed local-source failure states.
//!
//! Source loaders return structured failures rather than substituting a partial diff or a newer
//! workspace state. Front-ends can render these states directly while the loaders remain UI-neutral.

use super::source_paths::{is_safe_relative_path, parse_untracked_paths};
use super::untracked::{render_binary_addition, render_text_addition};
use super::{
    parse_unified_diff, GuidedReview, GuidedReviewBuildError, GuidedReviewParseError, ReviewBase,
    ReviewContext, ReviewId, ReviewRevisionId, ReviewSource, ReviewSourceArtifact, ReviewTarget,
    SourceRetentionPolicy, SourceStoreError, TurnBaselineArtifact, TurnBaselineId,
    TurnBaselineManifestError, TurnBaselineStoreError, SOURCE_CACHE_RELATIVE_DIR,
    TURN_BASELINE_CACHE_RELATIVE_DIR,
};
use std::{
    collections::HashSet,
    fs, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const MAX_GIT_STDERR_CHARS: usize = 1_000;
const DEFAULT_MAX_UNTRACKED_FILE_BYTES: u64 = 1_048_576;

/// Limits applied before an untracked file can enter a complete review source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UntrackedSourcePolicy {
    /// Maximum captured byte length for one untracked file.
    pub max_file_bytes: u64,
}

impl Default for UntrackedSourcePolicy {
    fn default() -> Self {
        Self { max_file_bytes: DEFAULT_MAX_UNTRACKED_FILE_BYTES }
    }
}

/// Conservative content class for an eligible untracked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UntrackedFileKind {
    /// Valid UTF-8 content without NUL bytes.
    Text,
    /// Content containing NUL bytes or invalid UTF-8.
    Binary,
}

/// File-level provenance for a canonical WorkingTree capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeProvenance {
    /// Whether the Git index differs from `HEAD`.
    pub includes_staged: bool,
    /// Whether the worktree differs from the Git index.
    pub includes_unstaged: bool,
}

/// One pinned `HEAD` WorkingTree source paired with exact capture provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingTreeCapture {
    /// Immutable artifact containing the canonical review source.
    pub artifact: ReviewSourceArtifact,
    /// Resolved comparison base and file-level inclusion metadata.
    pub source: ReviewSource,
}

#[derive(Debug)]
struct TurnSnapshot {
    artifact: TurnBaselineArtifact,
}

#[derive(Debug, PartialEq, Eq)]
struct CapturedWorktreeSource {
    provenance: WorktreeProvenance,
    canonical_source: String,
    included_untracked_files: u64,
    /// Untracked files the capture could not hold
    uncovered_paths: Vec<PathBuf>,
}

/// A working-tree source as assembled: the diff, how many untracked files it took in, and the
/// untracked files it had to leave out.
#[derive(Debug, Default)]
struct AssembledSource {
    text: String,
    included_untracked_files: u64,
    uncovered: Vec<PathBuf>,
}

/// Runs locally resolved Git commands for one project without invoking network operations.
#[derive(Debug, Clone)]
pub struct LocalGit {
    repo_path: PathBuf,
    source_retention: SourceRetentionPolicy,
    protected_revisions: HashSet<ReviewRevisionId>,
    exclusions: super::ReviewExclusions,
    pull_request: Option<PullRequestPin>,
    /// How long one Git command may take
    git_deadline: std::time::Duration,
}

/// What GitHub said about a pull request, resolved before a capture so git can pin it.
///
/// A [`ReviewTarget::PullRequest`] names a pull request; this is what it resolved to at one moment.
/// The engine reads it through GitHub, because this runner talks to git and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestPin {
    /// Number within its repository
    pub number: u64,
    /// `owner/name` of the repository the pull request is on
    pub repository: String,
    /// Where git fetches from: a remote name, or the repository's URL when no remote names it
    pub fetch_from: String,
    /// Branch it targets
    pub base_ref: String,
    /// Tip of the target branch
    pub base_oid: String,
    /// Branch it comes from
    pub head_ref: String,
    /// Tip of the pull request
    pub head_oid: String,
}

impl LocalGit {
    /// Creates a local Git runner rooted at `repo_path`.
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
            source_retention: SourceRetentionPolicy::default(),
            protected_revisions: HashSet::new(),
            exclusions: super::ReviewExclusions::default(),
            pull_request: None,
            git_deadline: command::GIT_DEADLINE,
        }
    }

    /// Holds this runner's Git commands to `deadline`.
    #[cfg(all(test, unix))]
    pub(crate) fn with_git_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.git_deadline = deadline;
        self
    }

    /// Gives this runner the pull request a [`ReviewTarget::PullRequest`] resolved to.
    pub fn with_pull_request(mut self, pin: PullRequestPin) -> Self {
        self.pull_request = Some(pin);
        self
    }

    /// Leaves configured paths out of the reviews this runner builds (`review.exclude`).
    pub fn with_exclusions(mut self, exclusions: super::ReviewExclusions) -> Self {
        self.exclusions = exclusions;
        self
    }

    /// What this runner leaves out of a review it presents.
    pub(super) fn exclusions(&self) -> &super::ReviewExclusions {
        &self.exclusions
    }

    /// Uses a bounded retention policy for immutable source artifacts captured by this runner.
    pub fn with_source_retention_policy(mut self, source_retention: SourceRetentionPolicy) -> Self {
        self.source_retention = source_retention;
        self
    }

    /// Retains `revision_id` while this runner captures and prunes a replacement source.
    pub fn preserving_revision(&self, revision_id: ReviewRevisionId) -> Self {
        let mut protected_revisions = self.protected_revisions.clone();
        protected_revisions.insert(revision_id);
        Self {
            repo_path: self.repo_path.clone(),
            source_retention: self.source_retention.clone(),
            protected_revisions,
            exclusions: self.exclusions.clone(),
            pull_request: self.pull_request.clone(),
            git_deadline: self.git_deadline,
        }
    }
}

fn is_review_cache_path(path: &std::path::Path) -> bool {
    path.starts_with(super::LUXE_PROJECT_DIR)
        || path.starts_with(SOURCE_CACHE_RELATIVE_DIR)
        || path.starts_with(TURN_BASELINE_CACHE_RELATIVE_DIR)
}

fn is_turn_baseline_cache_path(path: &std::path::Path) -> bool {
    path.starts_with(TURN_BASELINE_CACHE_RELATIVE_DIR)
}

fn capture_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
}

fn ensure_stable_worktree_capture(
    first: CapturedWorktreeSource,
    second: CapturedWorktreeSource,
) -> Result<CapturedWorktreeSource, GuidedReviewError> {
    if first == second {
        Ok(first)
    } else {
        Err(ReviewSourceUnavailable::StaleCapture.into())
    }
}

fn git_failed(operation: &'static str, exit_code: Option<i32>, stderr: &[u8]) -> GuidedReviewError {
    GuidedReviewError::GitFailed { operation, exit_code, stderr: bounded_git_stderr(stderr) }
}

fn is_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_resolved_oid(output: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(output).ok()?.trim();
    is_git_oid(value).then(|| value.into())
}

fn bounded_git_stderr(stderr: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(stderr)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(MAX_GIT_STDERR_CHARS)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        "no diagnostic output".into()
    } else {
        text.into()
    }
}

/// Reasons a requested review target cannot be represented as a complete pinned source.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReviewSourceUnavailable {
    /// The selected directory is not a non-bare Git worktree.
    #[error("working-tree review requires a non-bare Git worktree")]
    NotWorkingTree,
    /// No complete retained baseline exists for the requested agent turn.
    #[error("no complete baseline is retained for the requested agent turn")]
    MissingTurnBaseline,
    /// An untracked path changed but was excluded from the retained source.
    #[error("untracked path {path} was excluded: {constraint}")]
    ExcludedUntracked {
        /// Changed path that could not be retained.
        path: PathBuf,
        /// Applied size, binary, or ignore constraint.
        constraint: String,
    },
    /// A workspace changed before its complete captured source could be assembled.
    #[error("working tree changed while its review source was being captured")]
    StaleCapture,
    /// A pull-request target reached the capture without being resolved first.
    #[error("pull request #{number} was not resolved before its capture")]
    PullRequestNotResolved {
        /// The requested pull request.
        number: u64,
    },
    /// The pull request's commits could not be fetched.
    #[error(
        "could not fetch pull request #{number}: {detail} (for a private repository, \
         `gh auth setup-git` lets git use your GitHub sign-in)"
    )]
    PullRequestFetch {
        /// The requested pull request.
        number: u64,
        /// What git said, bounded.
        detail: String,
    },
    /// The pull request's head shares no history with its base.
    #[error("pull request #{number} shares no history with its base")]
    PullRequestNoMergeBase {
        /// The requested pull request.
        number: u64,
    },
    /// The requested Base target has no merge-base with the current `HEAD`.
    #[error("base target {reference} has no merge-base with the current HEAD")]
    NoMergeBase {
        /// User-requested local reference.
        reference: String,
    },
}

/// Typed failures while resolving or loading an immutable Guided Review source.
#[derive(Debug, Error)]
pub enum GuidedReviewError {
    /// Git could not be launched locally.
    #[error("could not run git {operation} in {repo_path}: {error}")]
    GitSpawn {
        /// Git operation attempted.
        operation: &'static str,
        /// Repository directory used for the operation.
        repo_path: PathBuf,
        /// Underlying process-launch error.
        error: io::Error,
    },
    /// Git did not finish in time, and was stopped with everything it started.
    #[error("git {operation} did not finish within {after:?}")]
    GitTimeout {
        /// Git operation attempted.
        operation: &'static str,
        /// The deadline it ran past.
        after: std::time::Duration,
    },
    /// Git ran but rejected a locally resolved operation.
    #[error("git {operation} failed with exit {exit_code:?}: {stderr}")]
    GitFailed {
        /// Git operation attempted.
        operation: &'static str,
        /// Process exit code when available.
        exit_code: Option<i32>,
        /// Bounded, display-safe diagnostic text from Git.
        stderr: String,
    },
    /// An eligible path could not be read while forming a complete source.
    #[error("could not read review source path {path}: {error}")]
    ReadPath {
        /// Path requested from the project worktree.
        path: PathBuf,
        /// Underlying local filesystem error.
        error: io::Error,
    },
    /// Git returned output that cannot safely describe one local source.
    #[error("git {operation} returned malformed output: {reason}")]
    MalformedGitOutput {
        /// Git operation that produced the output.
        operation: &'static str,
        /// Validation failure without raw source content.
        reason: &'static str,
    },
    /// A requested ref or commit does not resolve to one unambiguous local object.
    #[error("invalid or ambiguous local Git reference: {reference}")]
    InvalidReference {
        /// User-supplied ref or commit identifier.
        reference: String,
    },
    /// The target lacks a complete trustworthy source.
    #[error(transparent)]
    Unavailable(#[from] ReviewSourceUnavailable),
    /// The captured canonical diff did not preserve complete review source coverage.
    #[error(transparent)]
    Parse(#[from] GuidedReviewParseError),
    /// A pinned source could not become a complete immutable review.
    #[error(transparent)]
    Build(#[from] GuidedReviewBuildError),
    /// Source persistence stopped before the immutable artifact was committed.
    #[error("review source persistence task stopped before finishing")]
    SourcePersistenceTask,
    /// Turn-baseline persistence stopped before its private artifact was committed.
    #[error("turn baseline persistence task stopped before finishing")]
    TurnBaselinePersistenceTask,
    /// Turn-baseline loading stopped before its private artifact was verified.
    #[error("turn baseline loading task stopped before finishing")]
    TurnBaselineLoadTask,
    /// The baseline manifest was unsafe or incomplete.
    #[error(transparent)]
    TurnBaselineManifest(#[from] TurnBaselineManifestError),
    /// The retained turn baseline could not be persisted or verified.
    #[error(transparent)]
    TurnBaselineStore(#[from] TurnBaselineStoreError),
    /// The pinned source artifact could not be written or verified.
    #[error(transparent)]
    SourceStore(#[from] SourceStoreError),
    /// The Last Turn selector could not be built or persisted after both snapshots were retained.
    #[error("could not persist Last Turn selector: {0}")]
    LastTurnRecord(String),
    /// Last Turn selector persistence stopped before its private metadata was committed.
    #[error("Last Turn selector persistence task stopped before finishing")]
    LastTurnRecordTask,
    /// A commit was refused because doing it would have been unsafe.
    #[error("cannot commit this review: {reason}")]
    CommitRefused {
        /// Reviewer-facing explanation.
        reason: &'static str,
    },
    /// A refresh request no longer matches the revision the caller has selected.
    #[error("review refresh expected a stale revision")]
    StaleRevision {
        /// Revision supplied by the refresh request.
        expected: ReviewRevisionId,
        /// Revision currently displayed by the caller.
        current: ReviewRevisionId,
    },
}

use snapshot_paths::normalize_no_index_snapshot_paths;

mod capture;
mod command;
mod dispatch;
mod git;
mod persistence;
mod pull_request;
mod snapshot_paths;
mod sources;
mod turn;

#[cfg(test)]
mod tests;
