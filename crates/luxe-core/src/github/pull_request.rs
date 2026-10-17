//! Pull request — a pull request or an issue as GitHub describes it, and what opening one takes.

use super::*;

/// A pull request, as GitHub describes it.
///
/// Text fields are raw: they are a stranger's words and go through [`untrusted`] wherever they head
/// for a prompt or a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Number within its repository
    pub number: u64,
    /// Title, raw
    pub title: String,
    /// Description, raw; empty when the author wrote none
    pub body: String,
    /// Web address
    pub url: String,
    /// Who opened it
    pub author: Author,
    /// Whether it is still a draft
    pub draft: bool,
    /// Open, closed or merged
    pub state: PullRequestState,
    /// Branch it targets
    pub base_ref: String,
    /// Tip of the target branch when this was read
    pub base_oid: String,
    /// Branch it comes from
    pub head_ref: String,
    /// Tip of the pull request when this was read
    pub head_oid: String,
    /// Whether the head branch lives in a fork
    pub cross_repository: bool,
    /// The review verdict so far, when the repository asks for one
    pub review_decision: Option<ReviewDecision>,
    /// What the checks on the head commit say
    pub checks: CheckRollup,
    /// Issues this pull request closes when it merges
    pub closing_issues: Vec<IssueRef>,
}

/// Who opened a pull request or wrote a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// Account login
    pub login: String,
    /// Whether GitHub marks the account as a bot
    pub bot: bool,
}

/// Where a pull request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

impl PullRequestState {
    /// Open, for a serde default.
    pub fn open() -> Self {
        Self::Open
    }

    /// How a person says it: `open`, `closed`, `merged`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
}

/// The review verdict GitHub reports for a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// Checks on a pull request's head commit, counted by outcome.
///
/// A rollup is a fact about the pull request. It cannot say which hunk a check exercised, so it is
/// never evidence about one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRollup {
    /// Finished and succeeded
    pub passed: usize,
    /// Finished and failed, errored, timed out or was cancelled
    pub failed: usize,
    /// Queued, running, or expected and not yet reported
    pub pending: usize,
    /// Skipped or neutral
    pub skipped: usize,
}

impl CheckRollup {
    /// How many checks there are in all.
    pub const fn total(&self) -> usize {
        self.passed + self.failed + self.pending + self.skipped
    }
}

/// An issue named by a pull request, possibly in another repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    /// `owner/name` of the repository the issue lives in
    pub repository: String,
    /// Number within that repository
    pub number: u64,
}

/// An issue, as GitHub describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Number within its repository
    pub number: u64,
    /// Title, raw
    pub title: String,
    /// Description, raw
    pub body: String,
    /// Web address
    pub url: String,
    /// Who opened it; empty for a deleted account
    pub author: String,
    /// The latest comments, oldest first, leaving out ones GitHub hid as spam or abuse
    pub comments: Vec<IssueComment>,
    /// How many comments it has in all
    pub total_comments: usize,
}

/// One comment on an issue, raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueComment {
    /// Who wrote it; empty for a deleted account
    pub author: String,
    /// What they wrote, raw
    pub body: String,
}

/// A pull request to open, exactly as the reader confirmed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPullRequest {
    /// The branch it comes from, as `owner:branch` when that is a fork
    pub head: String,
    /// The branch it targets
    pub base: String,
    /// Title
    pub title: String,
    /// Description, in the reader's words
    pub body: String,
    /// Whether it opens as a draft
    pub draft: bool,
}

/// Whether a branch is on its remote, which decides whether opening its pull request pushes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Publication {
    /// Never pushed, or pushed nowhere git knows about
    Unpublished,
    /// Pushed, with this many commits since
    Ahead {
        /// Commits not on the remote yet
        commits: usize,
    },
    /// On the remote, every commit
    Pushed,
}

/// A pull request just opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPullRequest {
    /// Its number
    pub number: u64,
    /// Where it can be read
    pub url: String,
}
