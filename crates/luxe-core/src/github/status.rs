//! Status — where a branch stands: its pull request, its stack, its checks and the budget left.

use super::*;

/// Where a branch is published: the name it has there, and who owns the repository it went to.
///
/// The owner is what tells your pull request from a fork's that happens to use the same branch
/// name, which on a popular repository is most of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchHead {
    /// Branch name on the remote it is pushed to
    pub name: String,
    /// Owner of the repository it is pushed to, when a push target is configured
    pub owner: Option<String>,
}

/// Where a pull request's head lives, the branch it gets here, and whether a push from here lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestHead {
    /// The branch on the head repository
    pub head_ref: String,
    /// Owner of the head repository
    pub owner: String,
    /// Name of the head repository, which a fork may have renamed
    pub name: String,
    /// Whether the head lives in a fork
    pub cross_repository: bool,
    /// The local branch it is checked out on: the head's own name, except a fork's branch named
    /// like this repository's default, which would take its place, and becomes `owner/branch`
    pub local_branch: String,
    /// Whether a push lands on the pull request
    pub push: PushAccess,
}

/// Whether the reader can push to a pull request's head, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushAccess {
    Allowed,
    /// The reader cannot write to the repository the head is in
    NoWriteAccess,
    /// A fork whose author did not allow edits by maintainers
    MaintainerEditsOff,
    /// The fork was deleted, so there is no branch to push to
    HeadRepositoryGone,
}

impl PullRequestHead {
    /// Why pull request `number` cannot be fixed from here, or `None` when a push lands on it.
    pub fn refusal(&self, number: u64) -> Option<String> {
        let fork = format!("{}/{}", self.owner, self.name);
        match self.push {
            PushAccess::Allowed => None,
            PushAccess::NoWriteAccess if self.cross_repository => Some(format!(
                "Pull request #{number} comes from {fork}, and this account can push neither to it \
                 nor, as a maintainer, to this repository, so a fix could not reach it. Comment \
                 on it instead: alt+enter posts a review."
            )),
            PushAccess::NoWriteAccess => Some(format!(
                "This account cannot push to this repository, so a fix to pull request #{number} \
                 could not reach it. Comment on it instead: alt+enter posts a review."
            )),
            PushAccess::MaintainerEditsOff => Some(format!(
                "Pull request #{number} comes from {fork}, whose author did not allow edits by \
                 maintainers, so a push to it would be refused. Ask them to allow it, or comment \
                 instead: alt+enter posts a review."
            )),
            PushAccess::HeadRepositoryGone => Some(format!(
                "The fork pull request #{number} came from was deleted, so there is no branch to \
                 push a fix to."
            )),
        }
    }
}

/// A branch's pull request, when it has one, and what asking cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchStatus {
    /// The branch's pull request: an open one if there is one, else the newest
    pub pull_request: Option<PullRequestStatus>,
    /// The GraphQL budget left after asking
    pub rate_limit: Option<RateLimit>,
}

/// A pull request as a status row shows it.
///
/// The title is a stranger's words on someone else's repository; it has been through
/// [`untrusted`] and is shown, never prompted with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestStatus {
    /// Number within its repository
    pub number: u64,
    /// Title, filtered and bounded
    pub title: String,
    /// Web address
    pub url: String,
    /// Open, closed or merged
    pub state: PullRequestState,
    /// Whether it is still a draft
    pub draft: bool,
    /// The review verdict so far, when the repository asks for one
    pub review_decision: Option<ReviewDecision>,
    /// What the checks on its head say
    pub checks: CheckRollup,
    /// Review threads nobody has resolved, among the first hundred
    pub unresolved_threads: usize,
    /// Tip of the pull request when this was read
    pub head_oid: String,
    /// Where it sits in a stack, when it is in one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<Stack>,
}

/// A stack: pull requests each targeting the branch of the one below, the bottom one on trunk.
///
/// Read from the repository's open pull requests, the hundred most recently updated, which is
/// where a stack being worked on is; a longer-idle one may show shorter than it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    /// The branch the bottom of the stack targets
    pub trunk: String,
    /// Whether that is the repository's default branch, rather than a branch with no pull request
    pub trunk_is_default: bool,
    /// The pull requests below this one, bottom first
    pub below: Vec<StackEntry>,
    /// The ones above it, nearest first
    pub above: Vec<StackEntry>,
}

/// One pull request in a stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackEntry {
    /// Number within its repository
    pub number: u64,
    /// Title, filtered and bounded
    pub title: String,
    /// Its branch
    pub head_ref: String,
    /// Whether it is still a draft
    pub draft: bool,
    /// Whether any check reported on its head
    pub checked: bool,
}

impl Stack {
    /// Where the pull request sits, counting from the bottom at 1.
    pub fn position(&self) -> usize {
        self.below.len() + 1
    }

    /// How many pull requests the stack has, this one included.
    pub fn height(&self) -> usize {
        self.below.len() + 1 + self.above.len()
    }

    /// Whether the checks missing from a pull request with `own` are held back because it does
    /// not target trunk: none reported here, and the bottom of the stack — the one that does —
    /// has them. The evidence is the repository's own behavior, not a reading of its workflows.
    pub fn checks_wait_for_trunk(&self, own: &CheckRollup) -> bool {
        own.total() == 0
            && self.trunk_is_default
            && self.below.first().is_some_and(|bottom| bottom.checked)
    }
}

/// What is left of the hourly GraphQL budget, which every tool on this sign-in shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    /// Points left this hour
    pub remaining: u32,
    /// When the budget refills, in seconds since the Unix epoch
    pub reset_at: u64,
}

/// A check that failed on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailingCheck {
    /// Check name, as GitHub lists it
    pub name: String,
    /// How it ended: `failure`, `timed_out`, `error`…
    pub outcome: String,
    /// Where it can be read
    pub url: String,
    /// The check's own one-line account, when it gives one
    pub summary: Option<String>,
    /// What its log says
    pub log: CheckLog,
}

/// What could be read of a failing check's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckLog {
    /// Not a GitHub Actions job, so there is no log to read here, only its page
    NotActions,
    /// A job past the few whose logs are read, to keep one call bounded
    NotRead,
    /// Each failing step, and the end of its log
    Steps(Vec<FailedStep>),
    /// The log could not be read: expired, or not visible to this account
    Unreadable(String),
}

/// One failing step of an Actions job, and the end of its log up to its error.
///
/// `gh` cannot always split a job's log by step; then the step is `UNKNOWN STEP` and the log is the
/// whole job's, cleanup included, which is why the tail ends at the last `##[error]` line rather
/// than at the last line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedStep {
    /// Step name, or `UNKNOWN STEP` when `gh` could not tell
    pub name: String,
    /// The lines up to and including the last error, timestamps and color codes removed
    pub tail: Vec<String>,
    /// Lines before the tail that were left out
    pub omitted: usize,
    /// Lines after the last error that were left out: the job cleaning up
    pub after: usize,
}
