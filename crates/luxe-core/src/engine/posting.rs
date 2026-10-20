//! Engine posting — a pull request's review, prepared and posted through GitHub.
//!
//! Two steps, so what reaches GitHub is exactly what was on screen. Preparing reads what the
//! preview needs — GitHub's patches, whether the author pushed since the review was pinned, and
//! whether the reader is the author — and keeps it. The front end draws the preview with the pure
//! [`crate::guided_review::plan_post`]; posting plans with the same function over the same inputs,
//! refusing if the comments moved since the preview was drawn, and the review desk marks what it
//! carried as posted.

use super::*;
use crate::github::{GitHub, GitHubError, Repository, ReviewEvent, SubmitReview};
use crate::guided_review::{FindingId, PullRequestPostId, ReviewBase};

impl Engine {
    /// Reads what posting the open pull request review needs, and keeps it for the confirm.
    pub(crate) async fn handle_prepare_pull_request_post(
        &self,
        request_id: PullRequestPostId,
        review_id: crate::guided_review::ReviewId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, None).await {
            Ok((review, _)) => review,
            Err(message) => {
                let _ = event_tx.send(Event::PullRequestPostFailed { request_id, message }).await;
                return;
            }
        };
        let github = self.config.github.clone();
        let project_root = self.config.guided_review_project_root.clone();
        let previews = Arc::clone(&self.previews);
        super::previews::lock(&previews).post.begin(request_id.clone());
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match prepare(github, project_root, &review).await {
                Ok((patches, head_moved, own_pull_request)) => {
                    super::previews::lock(&previews).post.keep(
                        &request_id,
                        super::previews::PostPreview {
                            review_id: review.id.clone(),
                            revision_id: review.revision_id.clone(),
                            patches: patches.clone(),
                            own_pull_request,
                        },
                    );
                    Event::PullRequestPostPrepared {
                        request_id,
                        patches,
                        head_moved,
                        own_pull_request,
                    }
                }
                Err(message) => Event::PullRequestPostFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Posts the open pull request review with `verdict`, planned from what the preview read and
    /// the comments as they stood when the reader saw them.
    ///
    /// The plan is the same pure [`crate::guided_review::plan_post`] the preview draws, over the
    /// same inputs: the patches kept at prepare, the review and its comments, and the verdict. A
    /// session that moved past `session_revision` is refused, since its comments are not the ones
    /// that were shown. The preview stays until the post lands, so a failed one can be tried again.
    pub(crate) async fn handle_post_pull_request_review(
        &self,
        request_id: PullRequestPostId,
        verdict: ReviewEvent,
        session_revision: u64,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let planned = self.plan_review_post(&request_id, verdict, session_revision).await;
        let (review, submission, finding_ids) = match planned {
            Ok(planned) => planned,
            Err(message) => {
                let _ = event_tx.send(Event::PullRequestPostFailed { request_id, message }).await;
                return;
            }
        };
        let github = self.config.github.clone();
        let project_root = self.config.guided_review_project_root.clone();
        let previews = Arc::clone(&self.previews);
        let event_tx = event_tx.clone();
        let desk = self.review_desk();
        tokio::spawn(async move {
            let event = match post(github, project_root, &review, &submission).await {
                Ok((github_review_id, url)) => {
                    super::previews::lock(&previews).post.take(&request_id);
                    // Marked on the session before anyone is told, so a second post leaves them out
                    desk.send(super::review_desk::DeskCommand::Posted {
                        finding_ids: finding_ids.clone(),
                        comment: crate::guided_review::PostedComment {
                            review_id: github_review_id,
                            url: url.clone(),
                        },
                        outcome: outcome_of(submission.event),
                    });
                    Event::PullRequestReviewPosted {
                        request_id,
                        github_review_id,
                        url,
                        finding_ids,
                    }
                }
                Err(message) => Event::PullRequestPostFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// The review to post for `request_id`, its request, and the comments it carries; or why it
    /// cannot go, as a sentence.
    async fn plan_review_post(
        &self,
        request_id: &PullRequestPostId,
        verdict: ReviewEvent,
        session_revision: u64,
    ) -> Result<(GuidedReview, SubmitReview, Vec<FindingId>), String> {
        let preview = super::previews::lock(&self.previews).post.get(request_id).cloned();
        let preview = preview
            .ok_or("That preview is not the current one, so nothing was posted. Open it again.")?;
        let (review, session) =
            self.open_review(&preview.review_id, Some(&preview.revision_id)).await?;
        if session.revision != session_revision {
            return Err("The comments changed since the preview was drawn, so nothing was \
                        posted. Look at it again."
                .into());
        }
        if preview.own_pull_request && verdict != ReviewEvent::Comment {
            return Err("GitHub does not let you approve or request changes on your own pull \
                        request; post it as comments."
                .into());
        }
        let plan =
            crate::guided_review::plan_post(&review, &session.findings, &preview.patches, verdict)
                .map_err(crate::guided_review::PostPlanError::sentence)?;
        let finding_ids = plan.placed.iter().map(|placed| placed.finding_id.clone()).collect();
        Ok((review, plan.review, finding_ids))
    }
}

/// The review's outcome a verdict posted to GitHub records.
fn outcome_of(event: crate::github::ReviewEvent) -> crate::guided_review::ReviewOutcome {
    use crate::github::ReviewEvent;
    use crate::guided_review::ReviewOutcome;
    match event {
        ReviewEvent::Approve => ReviewOutcome::Accepted,
        ReviewEvent::RequestChanges => ReviewOutcome::ChangesRequested,
        ReviewEvent::Comment => ReviewOutcome::Commented,
    }
}

/// GitHub, the pull request's repository and its number, or why the review cannot be posted.
async fn target(
    github: Option<Arc<dyn GitHub>>,
    project_root: Option<std::path::PathBuf>,
    review: &GuidedReview,
) -> Result<(Arc<dyn GitHub>, Repository, u64), String> {
    let ReviewBase::PullRequest { number, repository, .. } = &review.source.base else {
        return Err("Only a pull request's review can be posted to GitHub.".into());
    };
    let github =
        github.ok_or("Posting a review needs the GitHub CLI, and this run cannot reach it.")?;
    let root = project_root.ok_or("Posting a review needs a project, and this run has none.")?;
    // The checkout's host, and the pull request's own owner and name: a fork's upstream included
    let checkout = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let (owner, name) = repository.split_once('/').ok_or("The review names no repository.")?;
    Ok((
        github,
        Repository {
            host: checkout.host,
            owner: owner.into(),
            name: name.into(),
            default_branch: String::new(),
            remote: None,
        },
        *number,
    ))
}

async fn prepare(
    github: Option<Arc<dyn GitHub>>,
    project_root: Option<std::path::PathBuf>,
    review: &GuidedReview,
) -> Result<(Vec<crate::github::FilePatch>, bool, bool), String> {
    let (github, repository, number) = target(github, project_root, review).await?;
    let ReviewBase::PullRequest { head_oid, .. } = &review.source.base else {
        unreachable!("target() accepts only a pull request")
    };
    let current =
        github.pull_request(&repository, number).await.map_err(|error| sentence(&error))?;
    let viewer = github.account(Some(&repository.host)).await.map_err(|error| sentence(&error))?;
    let patches =
        github.pull_request_patches(&repository, number).await.map_err(|error| sentence(&error))?;
    Ok((
        patches,
        &current.head_oid != head_oid,
        viewer.login.eq_ignore_ascii_case(&current.author.login),
    ))
}

/// Posts once, and never twice.
///
/// A post whose answer never arrived may still have landed, so it is not retried blind: the
/// reviews on the pull request are read before and after, and a new one by the reader on the same
/// commit with the same body is the post that landed.
async fn post(
    github: Option<Arc<dyn GitHub>>,
    project_root: Option<std::path::PathBuf>,
    review: &GuidedReview,
    submission: &SubmitReview,
) -> Result<(u64, String), String> {
    let (github, repository, number) = target(github, project_root, review).await?;
    let before: std::collections::HashSet<u64> = github
        .reviews(&repository, number)
        .await
        .map_err(|error| sentence(&error))?
        .into_iter()
        .map(|existing| existing.id)
        .collect();
    match github.submit_review(&repository, number, submission).await {
        Ok(posted) => Ok((posted.id, posted.url)),
        Err(error @ (GitHubError::Timeout { .. } | GitHubError::Run { .. })) => {
            let viewer = github.account(Some(&repository.host)).await.ok();
            let landed = github.reviews(&repository, number).await.ok().and_then(|after| {
                after.into_iter().find(|existing| {
                    !before.contains(&existing.id)
                        && existing.commit_id == submission.commit_id
                        && existing.body == submission.body
                        && viewer.as_ref().is_some_and(|viewer| {
                            viewer.login.eq_ignore_ascii_case(&existing.author)
                        })
                })
            });
            match landed {
                Some(existing) => Ok((existing.id, existing.url)),
                None => Err(format!(
                    "{} The review is not on the pull request, so nothing was posted; posting \
                     again is safe.",
                    sentence(&error)
                )),
            }
        }
        Err(error) => Err(sentence(&error)),
    }
}

/// One sentence for a GitHub failure while posting or answering, chosen by variant.
pub(super) fn sentence(error: &GitHubError) -> String {
    match error {
        GitHubError::NotInstalled => {
            "This needs the GitHub CLI (`gh`). Install it from https://cli.github.com, then run \
             `gh auth login`."
                .into()
        }
        GitHubError::NotAuthenticated { .. } => {
            "The GitHub CLI is not signed in. Run `gh auth login` in a terminal, then try again."
                .into()
        }
        GitHubError::NotGitHub => "This repository has no remote on GitHub.".into(),
        GitHubError::NotFound => {
            "GitHub has no such pull request that this account can see.".into()
        }
        GitHubError::Timeout { after } => {
            format!("GitHub did not answer within {}s.", after.as_secs())
        }
        GitHubError::Run { source, .. } => format!("Could not run `gh`: {source}."),
        GitHubError::Unreadable(_) => {
            "`gh` answered with something luxe could not read. Updating the GitHub CLI may fix it."
                .into()
        }
        GitHubError::RateLimited => {
            "GitHub's rate limit for this account is spent, so nothing was posted. It refills \
             within the hour."
                .into()
        }
        GitHubError::Rejected { reasons } => {
            format!("GitHub refused the review: {}.", reasons.join("; "))
        }
        GitHubError::Failed { message } => format!("`gh` failed: {message}"),
    }
}
