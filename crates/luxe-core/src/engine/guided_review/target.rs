//! Target — resolving what a review is of: a pull request through GitHub, checked out where it
//! must be, with its review threads.

use super::*;

/// One sentence for why a pull request could not be read, and what to do about it.
///
/// Branches on the error, never on its text: `gh`'s wording is read once, in `luxe_core::github`.
pub(super) fn pull_request_failure(error: &PullRequestResolveError) -> GuidedReviewFailure {
    use crate::github::GitHubError;
    let PullRequestResolveError::GitHub { number, repository, error } = error else {
        return GuidedReviewFailure::unavailable(match error {
            PullRequestResolveError::NotLinked { repository } => format!(
                "{repository} is not this checkout's repository, and no local remote points at it. \
                 `git remote add upstream https://github.com/{repository}` makes its pull requests \
                 reviewable here."
            ),
            _ => "Reviewing a pull request needs the GitHub CLI, and this run cannot reach it."
                .into(),
        });
    };
    let on = repository.as_deref().map(|slug| format!(" in {slug}")).unwrap_or_default();
    match error {
        GitHubError::NotFound => GuidedReviewFailure {
            kind: crate::event::GuidedReviewFailureKind::InvalidTarget,
            message: format!("There is no pull request #{number}{on} that this account can see."),
        },
        GitHubError::NotInstalled => GuidedReviewFailure::unavailable(
            "Reviewing a pull request needs the GitHub CLI (`gh`). Install it from \
             https://cli.github.com, then run `gh auth login`.",
        ),
        GitHubError::NotAuthenticated { .. } => GuidedReviewFailure::unavailable(
            "The GitHub CLI is not signed in. Run `gh auth login` in a terminal, then try again.",
        ),
        GitHubError::NotGitHub => GuidedReviewFailure::unavailable(
            "This repository has no remote on GitHub, so it has no pull requests to review.",
        ),
        GitHubError::Timeout { after } => GuidedReviewFailure::unavailable(format!(
            "GitHub did not answer within {}s. Check the connection, then try again.",
            after.as_secs()
        )),
        GitHubError::Run { source, .. } => {
            GuidedReviewFailure::unavailable(format!("Could not run `gh`: {source}."))
        }
        GitHubError::Unreadable(_) => GuidedReviewFailure::unavailable(
            "`gh` answered with something luxe could not read. Updating the GitHub CLI may fix it.",
        ),
        GitHubError::RateLimited => GuidedReviewFailure::unavailable(
            "GitHub's rate limit for this account is spent. It refills within the hour.",
        ),
        GitHubError::Rejected { reasons } => GuidedReviewFailure::unavailable(format!(
            "GitHub refused to show pull request #{number}: {}.",
            reasons.join("; ")
        )),
        GitHubError::Failed { message } => GuidedReviewFailure::unavailable(format!(
            "`gh` could not read pull request #{number}: {message}"
        )),
    }
}

/// Places a pull request's review threads on the review built from it.
///
/// `threads` is `None` for every other target, which leaves the review without a conversation;
/// `Some(None)` is a pull request whose threads could not be read, which the review says.
pub(super) fn with_conversation(
    mut review: GuidedReview,
    threads: Option<Option<crate::github::ReviewThreads>>,
) -> GuidedReview {
    review.conversation = threads.map(|read| match read {
        Some(read) => crate::guided_review::earlier_conversation(&review.files, read),
        None => crate::guided_review::EarlierConversation::Unreadable,
    });
    review
}

/// Resolves a pull-request target through GitHub, or says why it cannot be.
pub(super) async fn resolve_target(
    github: Option<&Arc<dyn crate::github::GitHub>>,
    project_root: &std::path::Path,
    target: &ReviewTarget,
) -> Result<Option<ResolvedPullRequest>, GuidedReviewFailure> {
    let (number, repository, own) = match target {
        ReviewTarget::PullRequest { number, repository } => (*number, repository.as_deref(), false),
        ReviewTarget::OwnPullRequest { number } => (*number, None, true),
        _ => return Ok(None),
    };
    let github =
        github.ok_or_else(|| pull_request_failure(&PullRequestResolveError::Unreachable))?;
    let mut resolved = crate::guided_review::resolve_pull_request(
        github.as_ref(),
        project_root,
        number,
        repository,
    )
    .await
    .map_err(|error| pull_request_failure(&error))?;
    if own {
        on_its_branch(project_root, number, &resolved.pin.head_ref).await?;
        resolved.target = ReviewTarget::OwnPullRequest { number };
    }
    Ok(Some(resolved))
}

/// Refuses to review a pull request as your own unless its branch is the one checked out: the
/// working tree is what gets compared and fixed, and it has to be that pull request's.
async fn on_its_branch(
    project_root: &std::path::Path,
    number: u64,
    head_ref: &str,
) -> Result<(), GuidedReviewFailure> {
    let checked_out = crate::github::branch_head(project_root).await;
    match checked_out {
        Some(head) if head.name == head_ref => Ok(()),
        Some(head) => Err(GuidedReviewFailure::unavailable(format!(
            "Pull request #{number} comes from `{head_ref}`, and this checkout is on `{}`. Check out \
             `{head_ref}` to work on it here, or `/diff pr {number}` reads it without touching the \
             checkout.",
            head.name
        ))),
        None => Err(GuidedReviewFailure::unavailable(format!(
            "This checkout is not on a branch. Check out `{head_ref}` to work on pull request \
             #{number} here, or `/diff pr {number}` reads it without touching the checkout."
        ))),
    }
}
