//! Github — shared pieces of the pull request scenarios.

use std::path::Path;

use luxe_core::github::{
    Author, CheckRollup, ConversationComment, GitHubError, Issue, IssueRef, PullRequest,
    PullRequestState, Repository, ReviewDecision, ReviewThread, ReviewThreads,
};

/// How a scripted GitHub double fails, when it does.
pub(crate) type Failure = Option<fn() -> GitHubError>;

/// What the scripted GitHub double says; `head` can move between loads, the way a push does.
pub(crate) struct ScriptedGitHub {
    pub(crate) base: String,
    pub(crate) head: std::sync::Mutex<String>,
    pub(crate) fail_with: Failure,
    pub(crate) threads_fail: bool,
    /// Who is signed in; the pull request's author is `app/copilot-swe-agent`
    pub(crate) viewer: String,
    /// How the next post goes
    pub(crate) posting: std::sync::Mutex<Posting>,
    /// Reviews on the pull request, which a landed post joins
    pub(crate) reviews: std::sync::Mutex<Vec<luxe_core::github::ExistingReview>>,
    /// Every review it was asked to post, exactly as asked
    pub(crate) submitted: std::sync::Mutex<Vec<luxe_core::github::SubmitReview>>,
    /// Every thread answer it was asked for, in order: `reply:<id>:<body>` or `resolve:<id>`
    pub(crate) answered: std::sync::Mutex<Vec<String>>,
    /// Every pull request it was asked to open, exactly as asked
    pub(crate) opened: std::sync::Mutex<Vec<luxe_core::github::NewPullRequest>>,
    /// Whether an opened pull request's answer never arrives
    pub(crate) opening_lost: std::sync::atomic::AtomicBool,
    /// The branch's pull request, as the status row would find it
    pub(crate) branch_pull_request: std::sync::Mutex<Option<luxe_core::github::PullRequestStatus>>,
    /// Who can push to the pull request's head
    pub(crate) push: std::sync::Mutex<luxe_core::github::PushAccess>,
    /// Every search it was asked, in order
    pub(crate) searched: std::sync::Mutex<Vec<String>>,
    /// The branch a pull request targets
    pub(crate) pull_request_base: std::sync::Mutex<String>,
    /// What merged below, answered to the base it merged out from under
    pub(crate) merged: std::sync::Mutex<Option<(String, luxe_core::restack::MergedBelow)>>,
}

/// How a scripted post goes.
#[derive(Clone)]
pub(crate) enum Posting {
    Lands,
    /// The review lands, but the answer never arrives
    LandsSilently,
    /// Nothing lands, and the answer never arrives
    Lost,
    /// Nothing lands and the answer never arrives, but this other review appears meanwhile
    LostWhileAnotherAppears(luxe_core::github::ExistingReview),
    Refused,
}

pub(crate) fn repository() -> Repository {
    Repository {
        host: "github.com".into(),
        owner: "acme".into(),
        name: "app".into(),
        default_branch: "main".into(),
        remote: Some("origin".into()),
    }
}

mod script;
