//! Pull request — resolving one into a pinned review source, and what the review says about it.
//!
//! A [`ReviewTarget::PullRequest`] names a pull request. [`resolve_pull_request`] asks GitHub
//! what that is right now and turns the answer into two things: a [`PullRequestPin`] that git can
//! capture, and a [`PullRequestContext`] the brief and the guide read. The engine drives it; git
//! never learns GitHub exists and the `github` module never learns about reviews.
//!
//! Every string that came from GitHub is a stranger's and passes through [`github::untrusted`] here,
//! once, before it is stored with the review or reaches a prompt.

use super::{PullRequestPin, ReviewTarget, MAX_CONTEXT_EXCERPT_CHARS, MAX_CONTEXT_TITLE_CHARS};
use crate::github::{
    self, Discovery, GitHubError, IssueRef, PullRequest, PullRequests, Repository, Threads,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How many issues a pull request closes before the rest are only counted.
pub const MAX_LINKED_ISSUES: usize = 3;

/// Longest excerpt of a linked issue's description, in characters.
const LINKED_ISSUE_EXCERPT_CHARS: usize = 600;

/// How a review of a pull request names it: `pull request #42 · luxe-dev`, with the owner only when
/// it is on another repository than the checkout's, where the owner is what tells them apart.
///
/// The same for someone else's pull request read in place and one checked out to fix: which it is
/// shows in what the review offers, not in its name. `None` for a review of anything else.
pub fn pull_request_label(review: &super::GuidedReview) -> Option<String> {
    let repository = review.context.pull_request.as_ref().map(|pr| pr.repository.as_str());
    label(&review.target, repository)
}

fn label(target: &ReviewTarget, repository: Option<&str>) -> Option<String> {
    let (number, elsewhere) = match target {
        ReviewTarget::PullRequest { number, repository } => (*number, repository.is_some()),
        ReviewTarget::OwnPullRequest { number } => (*number, false),
        _ => return None,
    };
    let Some(repository) = repository else {
        return Some(format!("pull request #{number}"));
    };
    let shown = match repository.split_once('/') {
        Some((_, name)) if !elsewhere => name,
        _ => repository,
    };
    Some(format!("pull request #{number} · {shown}"))
}

/// What a review knows about the pull request it is reading.
///
/// Stored with the review and sent to front-ends, so it is data a client can render without asking
/// GitHub anything. The text in it is scrubbed and bounded already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestContext {
    /// Number within its repository
    pub number: u64,
    /// `owner/name`
    pub repository: String,
    /// Web address
    pub url: String,
    /// Title, scrubbed and bounded
    pub title: String,
    /// Description, scrubbed and bounded; empty when the author wrote none
    pub body: String,
    /// Login of whoever opened it
    pub author: String,
    /// Whether that account is a bot
    pub author_is_bot: bool,
    /// Whether it is still a draft
    pub draft: bool,
    /// Open, closed or merged
    pub status: PullRequestStatus,
    /// The review verdict so far, when the repository asks for one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<PullRequestVerdict>,
    /// Checks on the head commit, counted by outcome
    pub checks: PullRequestChecks,
    /// Branch it targets
    pub base_ref: String,
    /// Branch it comes from
    pub head_ref: String,
    /// Issues it closes, the first few read in full
    #[serde(default)]
    pub closing_issues: Vec<LinkedIssue>,
    /// Closing issues past the ones read
    #[serde(default)]
    pub more_closing_issues: usize,
}

/// Where a pull request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    Open,
    Closed,
    Merged,
}

/// The review verdict GitHub reports for a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestVerdict {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// Checks on a pull request's head commit, counted by outcome.
///
/// A fact about the pull request, never about a hunk: a rollup cannot say what a check exercised.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestChecks {
    /// Finished and succeeded
    pub passed: usize,
    /// Failed, errored, timed out or cancelled
    pub failed: usize,
    /// Not finished yet
    pub pending: usize,
    /// Skipped or neutral
    pub skipped: usize,
}

/// An issue a pull request closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedIssue {
    /// `owner/name` of the repository it lives in
    pub repository: String,
    /// Number within that repository
    pub number: u64,
    /// Title, scrubbed and bounded; absent when it could not be read
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Opening of its description, scrubbed and bounded
    #[serde(default)]
    pub excerpt: String,
}

impl PullRequestContext {
    /// What a review keeps from GitHub's answer, scrubbed on the way in.
    pub fn new(repository: &Repository, pr: &PullRequest, issues: Vec<LinkedIssue>) -> Self {
        let more_closing_issues = pr.closing_issues.len().saturating_sub(issues.len());
        Self {
            number: pr.number,
            repository: repository.slug(),
            url: pr.url.clone(),
            title: github::untrusted(&pr.title, MAX_CONTEXT_TITLE_CHARS),
            body: github::untrusted(pr.body.trim(), MAX_CONTEXT_EXCERPT_CHARS),
            author: pr.author.login.clone(),
            author_is_bot: pr.author.bot,
            draft: pr.draft,
            status: match pr.state {
                github::PullRequestState::Open => PullRequestStatus::Open,
                github::PullRequestState::Closed => PullRequestStatus::Closed,
                github::PullRequestState::Merged => PullRequestStatus::Merged,
            },
            verdict: pr.review_decision.map(|decision| match decision {
                github::ReviewDecision::Approved => PullRequestVerdict::Approved,
                github::ReviewDecision::ChangesRequested => PullRequestVerdict::ChangesRequested,
                github::ReviewDecision::ReviewRequired => PullRequestVerdict::ReviewRequired,
            }),
            checks: PullRequestChecks {
                passed: pr.checks.passed,
                failed: pr.checks.failed,
                pending: pr.checks.pending,
                skipped: pr.checks.skipped,
            },
            base_ref: pr.base_ref.clone(),
            head_ref: pr.head_ref.clone(),
            closing_issues: issues,
            more_closing_issues,
        }
    }

    /// The author's login as a person would write it: `app/dependabot` is `dependabot`.
    pub fn author_name(&self) -> &str {
        self.author.strip_prefix("app/").unwrap_or(&self.author)
    }
}

/// A pull request resolved far enough to capture and describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPullRequest {
    /// What git needs to pin it
    pub pin: PullRequestPin,
    /// What the review says about it
    pub context: PullRequestContext,
    /// The target in its one canonical spelling: no repository for the checkout's own
    pub target: ReviewTarget,
    /// Its review threads, or `None` when they could not be read; placed on the review once its
    /// files exist
    pub threads: Option<crate::github::ReviewThreads>,
}

/// Why a pull request could not be resolved.
#[derive(Debug)]
pub enum PullRequestResolveError {
    /// This engine has no way to reach a code host.
    Unreachable,
    /// The named repository is neither the checkout's nor one a local remote points at.
    NotLinked {
        /// `owner/name` as requested
        repository: String,
    },
    /// GitHub refused or failed.
    GitHub {
        /// The requested pull request
        number: u64,
        /// `owner/name` it was looked for on, when that far was reached
        repository: Option<String>,
        /// What went wrong
        error: GitHubError,
    },
}

impl std::fmt::Display for PullRequestResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => f.write_str("this run cannot reach GitHub"),
            Self::NotLinked { repository } => write!(
                f,
                "{repository} is not this checkout's repository, and no local remote points at it"
            ),
            Self::GitHub { error, .. } => error.fmt(f),
        }
    }
}

/// GitHub's error is this one's message, so what comes next in the chain is what GitHub's error
/// stands on (an OS error from running `gh`), not GitHub's error a second time.
impl std::error::Error for PullRequestResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GitHub { error, .. } => error.source(),
            Self::Unreachable | Self::NotLinked { .. } => None,
        }
    }
}

/// Asks GitHub what a pull request is right now, and resolves it into a pin and a context.
///
/// A named repository is accepted when it is the checkout's own or a local remote points at it,
/// which is how a fork reviews its upstream's pull requests. The fetch goes through that remote, so
/// it uses the transport the user configured; the checkout's own repository with no remote naming
/// it — a renamed or transferred repository, whose remote still says the old name — fetches from
/// the URL `gh` reports. Closing issues are read in full up to [`MAX_LINKED_ISSUES`]; one that
/// cannot be read is listed by number rather than failing the review.
pub async fn resolve_pull_request(
    github: &(impl Discovery + PullRequests + Threads + ?Sized),
    project_root: &Path,
    number: u64,
    requested: Option<&str>,
) -> Result<ResolvedPullRequest, PullRequestResolveError> {
    let failed = |repository: Option<String>| {
        move |error| PullRequestResolveError::GitHub { number, repository, error }
    };
    let checkout = github.repository(project_root).await.map_err(failed(None))?;
    let (repository, is_checkout) = match requested {
        None => (checkout, true),
        Some(slug) if checkout.is(slug) => (checkout, true),
        Some(slug) => {
            let (owner, name) = slug
                .split_once('/')
                .ok_or_else(|| PullRequestResolveError::NotLinked { repository: slug.into() })?;
            let remote = github::remote_pointing_at(project_root, &checkout.host, owner, name)
                .await
                .ok_or_else(|| PullRequestResolveError::NotLinked { repository: slug.into() })?;
            let repository = Repository {
                host: checkout.host.clone(),
                owner: owner.to_string(),
                name: name.to_string(),
                default_branch: String::new(),
                remote: Some(remote),
            };
            (repository, false)
        }
    };
    let pr =
        github.pull_request(&repository, number).await.map_err(failed(Some(repository.slug())))?;
    let issues = linked_issues(github, &repository, &pr.closing_issues).await;
    // Not worth failing the review over: without them the review is still whole, and it says the
    // conversation could not be read rather than implying there was none
    let threads = match github.review_threads(&repository, number).await {
        Ok(threads) => Some(threads),
        Err(error) => {
            tracing::warn!(%error, number, "review threads could not be read");
            None
        }
    };
    let fetch_from = repository
        .remote
        .clone()
        .unwrap_or_else(|| format!("https://{}/{}.git", repository.host, repository.slug()));
    Ok(ResolvedPullRequest {
        pin: PullRequestPin {
            number,
            repository: repository.slug(),
            fetch_from,
            base_ref: pr.base_ref.clone(),
            base_oid: pr.base_oid.clone(),
            head_ref: pr.head_ref.clone(),
            head_oid: pr.head_oid.clone(),
        },
        context: PullRequestContext::new(&repository, &pr, issues),
        target: ReviewTarget::PullRequest {
            number,
            repository: (!is_checkout).then(|| repository.slug()),
        },
        threads,
    })
}

/// The first few closing issues, read; any that cannot be read keep their number.
async fn linked_issues(
    github: &(impl PullRequests + ?Sized),
    repository: &Repository,
    closing: &[IssueRef],
) -> Vec<LinkedIssue> {
    let mut issues = Vec::new();
    for reference in closing.iter().take(MAX_LINKED_ISSUES) {
        let (owner, name) = reference.repository.split_once('/').unwrap_or_default();
        let home = Repository {
            host: repository.host.clone(),
            owner: owner.to_string(),
            name: name.to_string(),
            default_branch: String::new(),
            remote: None,
        };
        let read = github.issue(&home, reference.number).await.ok();
        issues.push(LinkedIssue {
            repository: reference.repository.clone(),
            number: reference.number,
            title: read
                .as_ref()
                .map(|issue| github::untrusted(&issue.title, MAX_CONTEXT_TITLE_CHARS)),
            excerpt: read
                .map(|issue| github::untrusted(issue.body.trim(), LINKED_ISSUE_EXCERPT_CHARS))
                .unwrap_or_default(),
        });
    }
    issues
}

#[cfg(test)]
mod tests;
