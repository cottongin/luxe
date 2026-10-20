//! Overview — what stands between a pull request and merged, and who is already on an issue.
//!
//! The detail `/github` and `/pr` show. [`super::GitHub`] reads the facts; [`merge_checks`] turns them into
//! rows a person can act on, here rather than in a front-end, because what blocks a merge is a fact
//! every front-end needs and none should recompute.
//!
//! Every check is something GitHub reported. Branch protection details need admin access on many
//! repositories and rulesets live elsewhere, so nothing here reads the rules: `mergeStateStatus` is
//! GitHub's own answer to "can this merge", and the rest refines it.

mod readiness;

pub use readiness::{blocking, merge_checks, CheckAction, CheckMark, MergeCheck};

use serde::{Deserialize, Serialize};

use super::{Author, CheckRollup, ConversationComment, PullRequestState, ReviewDecision, Stack};
use super::{ReviewThread, ThreadComment};

/// A label on a pull request or an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// What it says
    pub name: String,
    /// The repository's color for it, `rrggbb`
    pub color: String,
}

/// Whether a reviewer is a person, a team, or a bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    User,
    Team,
    Bot,
}

/// A reviewer's latest word on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    /// Asked to review and has not yet
    Requested,
}

/// One reviewer and where they stand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    /// Login, or `org/team` for a team
    pub login: String,
    /// Person, team or bot
    pub kind: ReviewerKind,
    /// Their latest verdict
    pub verdict: Verdict,
    /// When they gave it, or were asked, in seconds since the Unix epoch
    pub at: Option<u64>,
}

/// GitHub's own answer to whether a pull request can merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    /// Mergeable, checks passing
    Clean,
    /// Mergeable, with pre-receive hooks
    HasHooks,
    /// Mergeable, with checks that are not passing but not required either
    Unstable,
    /// The head is out of date with its base
    Behind,
    /// Something required has not happened yet
    Blocked,
    /// The merge commit cannot be made cleanly
    Dirty,
    /// A draft cannot merge
    Draft,
    /// GitHub is still working it out
    Unknown,
}

impl MergeStatus {
    /// Whether GitHub would take the merge now.
    pub const fn mergeable(self) -> bool {
        matches!(self, Self::Clean | Self::HasHooks | Self::Unstable)
    }
}

/// How a pull request can be merged, declared in the order GitHub offers them: a merge commit,
/// then squash, then rebase. The order is the type's, so every surface lists them the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// How GitHub's button says it.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Merge => "Create a merge commit",
            Self::Squash => "Squash and merge",
            Self::Rebase => "Rebase and merge",
        }
    }

    /// The same, as a phrase inside a sentence: `squash and merge`.
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::Merge => "merge commit",
            Self::Squash => "squash and merge",
            Self::Rebase => "rebase and merge",
        }
    }
}

/// A merge, exactly as the reader confirmed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequest {
    /// How it merges
    pub method: MergeMethod,
    /// The head the reader saw; GitHub refuses the merge if it has moved since
    pub head_oid: String,
    /// The head branch to delete once merged, when the reader asked and it is this repository's
    pub delete_branch: Option<String>,
}

/// A merge that landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merged {
    /// The commit the merge made on the base
    pub sha: String,
    /// Whether the head branch is gone, deleted now or already
    pub branch_deleted: bool,
}

/// A failing check, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailingCheckName {
    /// Its name, as GitHub lists it
    pub name: String,
    /// Whether branch protection requires it
    pub required: bool,
}

/// What the signed-in account may do with a pull request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    /// Whether it opened the pull request
    pub author: bool,
    /// Whether a push from here lands on its head
    pub can_push: bool,
    /// Whether it may merge into the base
    pub can_merge: bool,
}

/// A pull request as its detail shows it.
///
/// Text fields are raw: a stranger's words, sanitized where they are drawn and quoted as data
/// wherever they head for a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestOverview {
    /// Number within its repository
    pub number: u64,
    /// GitHub's id for it, which its mutations name
    #[serde(default)]
    pub node_id: String,
    /// Title, raw
    pub title: String,
    /// Description, raw
    pub body: String,
    /// Web address
    pub url: String,
    /// `owner/name`
    pub repository: String,
    /// Who opened it
    pub author: Author,
    /// Open, closed or merged
    pub state: PullRequestState,
    /// Whether it is still a draft
    pub draft: bool,
    /// Branch it targets
    pub base_ref: String,
    /// Branch it comes from
    pub head_ref: String,
    /// Tip of the pull request when this was read, which a merge or an update names so that it
    /// refuses rather than acting on a commit the reader never saw
    pub head_oid: String,
    /// Whether the head branch lives in a fork
    pub cross_repository: bool,
    /// Lines added
    pub additions: u64,
    /// Lines removed
    pub deletions: u64,
    /// Files changed
    pub changed_files: u64,
    /// Commits on it
    pub commits: u64,
    /// When it last changed, in seconds since the Unix epoch
    pub updated_at: u64,
    /// Its labels
    pub labels: Vec<Label>,
    /// GitHub's answer to whether it can merge
    pub merge_status: MergeStatus,
    /// The review verdict so far, when the repository asks for one
    pub review_decision: Option<ReviewDecision>,
    /// Everyone asked to review, and everyone who did
    pub reviewers: Vec<Reviewer>,
    /// What the checks on its head say
    pub checks: CheckRollup,
    /// The checks failing on its head, required ones first
    pub failing: Vec<FailingCheckName>,
    /// Review threads nobody has resolved
    pub unresolved_threads: usize,
    /// Where it sits in a stack, when it is in one
    pub stack: Option<Stack>,
    /// What the signed-in account may do with it
    pub viewer: Viewer,
    /// The merge methods the repository allows, in any order; [`Self::merge_options`] puts them
    /// in GitHub's
    pub merge_methods: Vec<MergeMethod>,
    /// What else the repository and the signed-in account bring to a merge
    #[serde(default)]
    pub merge_settings: MergeSettings,
    /// How many entries its conversation has
    pub conversation_count: usize,
}

/// What the repository and the signed-in account bring to a merge, beyond the methods allowed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeSettings {
    /// The method this account last merged with in the repository, which GitHub's own button
    /// opens on
    pub last_method: Option<MergeMethod>,
    /// Whether the repository deletes a merged head branch by itself
    pub deletes_branch: bool,
    /// Whether this account may turn on auto-merge now: the repository allows it, and something
    /// still stands between the pull request and a merge
    pub can_auto_merge: bool,
    /// Whether it may turn auto-merge off
    pub can_cancel_auto_merge: bool,
    /// The method auto-merge will use, while it is on
    pub auto_merge: Option<MergeMethod>,
}

impl PullRequestOverview {
    /// The method a merge starts on: the one this account used last in the repository, as
    /// GitHub's button does, when the repository still allows it; otherwise the first allowed.
    pub fn default_merge_method(&self) -> MergeMethod {
        let options = self.merge_options();
        self.merge_settings
            .last_method
            .filter(|method| options.contains(method))
            .unwrap_or(options[0])
    }

    /// The methods a merge can use, in GitHub's order — merge commit, squash, rebase — the first
    /// of them the default. A repository that reported none is offered a merge commit, which is
    /// what GitHub does when nothing has been turned off.
    pub fn merge_options(&self) -> Vec<MergeMethod> {
        let mut methods = self.merge_methods.clone();
        methods.sort();
        methods.dedup();
        if methods.is_empty() {
            methods.push(MergeMethod::Merge);
        }
        methods
    }
}

/// A pull request that closes an issue, or mentions it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedPullRequest {
    /// Number within its repository
    pub number: u64,
    /// Title, raw
    pub title: String,
    /// Open, closed or merged
    pub state: PullRequestState,
    /// Whether it is a draft
    pub draft: bool,
    /// Who opened it
    pub author: String,
    /// Whether it closes the issue when it merges, rather than only mentioning it
    pub closes: bool,
}

/// An issue as its detail shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueOverview {
    /// Number within its repository
    pub number: u64,
    /// Title, raw
    pub title: String,
    /// Description, raw
    pub body: String,
    /// Web address
    pub url: String,
    /// `owner/name`
    pub repository: String,
    /// Who opened it
    pub author: Author,
    /// Open or closed; an issue is never merged
    pub state: PullRequestState,
    /// When it last changed, in seconds since the Unix epoch
    pub updated_at: u64,
    /// Its labels
    pub labels: Vec<Label>,
    /// Logins it is assigned to
    pub assignees: Vec<String>,
    /// The milestone it is in
    pub milestone: Option<String>,
    /// Pull requests that close it or mention it
    pub linked: Vec<LinkedPullRequest>,
    /// How many entries its conversation has
    pub conversation_count: usize,
}

/// A session of yours that took an issue up, which only luxe knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSession {
    /// The session's id, which resuming it names
    pub session: String,
    /// The worktree branch it works on, when it has one
    pub branch: Option<String>,
    /// When it was last written to, in seconds since the Unix epoch
    pub last_used: u64,
}

/// One pull request's or issue's overview: what `/github`'s detail and `/pr` draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "overview", rename_all = "snake_case")]
pub enum Overview {
    PullRequest(Box<PullRequestOverview>),
    Issue(Box<IssueOverview>),
}

impl Overview {
    /// Its number within its repository.
    pub fn number(&self) -> u64 {
        match self {
            Self::PullRequest(pr) => pr.number,
            Self::Issue(issue) => issue.number,
        }
    }

    /// Its web address.
    pub fn url(&self) -> &str {
        match self {
            Self::PullRequest(pr) => &pr.url,
            Self::Issue(issue) => &issue.url,
        }
    }
}

/// What a `/github` row says under its title.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowFacts {
    /// Whether the author is a bot
    pub author_bot: bool,
    /// Lines added, for a pull request
    pub additions: u64,
    /// Lines removed, for a pull request
    pub deletions: u64,
    /// What the checks on its head say, for a pull request
    pub checks: CheckRollup,
    /// The review verdict so far, for a pull request
    pub review_decision: Option<ReviewDecision>,
    /// How many comments it has
    pub comments: usize,
    /// Its labels
    pub labels: Vec<Label>,
    /// The open pull request that closes it, for an issue
    pub linked_pull_request: Option<u64>,
}

/// One entry in a pull request's or an issue's conversation, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationEntry {
    /// The description, as its author opened with it
    Opening {
        /// Who wrote it
        author: Author,
        /// What they wrote, raw
        body: String,
        /// When, in seconds since the Unix epoch
        at: u64,
    },
    /// A comment on the whole pull request or issue
    Comment(ConversationComment),
    /// A review's summary and verdict
    Review {
        /// The summary, as a comment
        comment: ConversationComment,
        /// The verdict it was submitted with
        verdict: Verdict,
        /// Line comments submitted with it
        line_comments: usize,
    },
    /// A review thread on a line or a file
    Thread(ReviewThread),
}

impl ConversationEntry {
    /// Whether whoever started this entry is a bot.
    pub fn by_bot(&self) -> bool {
        match self {
            Self::Opening { author, .. } => author.bot,
            Self::Comment(comment) | Self::Review { comment, .. } => comment.bot,
            Self::Thread(thread) => thread.comments.first().is_some_and(by_bot_login),
        }
    }
}

/// Whether a thread comment's author looks like a bot: GitHub Apps post as `name[bot]`.
pub fn by_bot_login(comment: &ThreadComment) -> bool {
    comment.author.ends_with("[bot]")
}
