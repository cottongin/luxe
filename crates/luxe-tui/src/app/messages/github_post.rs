//! Posting a pull request's review — the preview, the verdict, and the answer.
//!
//! The preview and the post both come from [`plan`], computed from the same inputs, so what reaches
//! GitHub is exactly what the reader saw. The dialog is modal, so nothing between the two can
//! change the comments it maps.

use luxe_core::engine::Action;
use luxe_core::github::ReviewEvent;
use luxe_core::guided_review::{
    plan_post, PostPlan, PostPlanError, PostedComment, PullRequestPostId, ReviewBase,
};

use super::{App, ConvoBlock};
use crate::guided_review::{GithubPost, PreparedPost};

/// Whether the review on screen is a pull request's, which finishes on GitHub.
pub(crate) fn is_pull_request(app: &App) -> bool {
    app.guided_review
        .as_ref()
        .is_some_and(|review| matches!(review.source.base, ReviewBase::PullRequest { .. }))
}

/// Opens the preview and asks GitHub what it needs.
pub(super) fn open(app: &mut App) {
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    let request_id = PullRequestPostId::new();
    if !app.send_action(
        Action::PreparePullRequestPost { request_id: request_id.clone(), review_id },
        "prepare a review post",
    ) {
        return;
    }
    let workspace = &mut app.guided_review_workspace;
    workspace.help_visible = false;
    workspace.close_feedback_window();
    workspace.github_post = Some(GithubPost {
        request_id,
        verdict: ReviewEvent::Comment,
        prepared: None,
        posting: false,
        error: None,
        selected: 0,
        scroll: 0,
        drafting: None,
        drafting_since: None,
        draft_note: None,
    });
}

/// The review the preview shows and the post sends, once GitHub has said what it needs.
pub(crate) fn plan(app: &App) -> Option<Result<PostPlan, PostPlanError>> {
    let post = app.guided_review_workspace.github_post.as_ref()?;
    let prepared = post.prepared.as_ref()?;
    let review = app.guided_review.as_ref()?;
    let findings = app.review.session().map_or(&[][..], |session| &session.findings[..]);
    Some(plan_post(review, findings, &prepared.patches, post.verdict))
}

/// How many things the preview lists: the review itself, then each comment that sits on a line.
///
/// A comment placed in the review's body is part of the review's own text on GitHub, so it is read
/// there rather than listed twice.
pub(crate) fn item_count(app: &App) -> usize {
    match plan(app) {
        Some(Ok(plan)) => {
            1 + plan
                .placed
                .iter()
                .filter(|placed| {
                    !matches!(placed.place, luxe_core::guided_review::Placement::Body(_))
                })
                .count()
        }
        _ => 1,
    }
}

/// Posts what the preview shows, or asks GitHub again when reading the pull request failed.
pub(super) fn confirm(app: &mut App) {
    let Some(post) = app.guided_review_workspace.github_post.as_ref() else {
        return;
    };
    if post.posting {
        return;
    }
    if post.prepared.is_none() {
        if post.error.is_some() {
            open(app);
        }
        return;
    }
    // Said here, where the reader is; the engine plans the same way and refuses it too
    match plan(app) {
        Some(Ok(_)) => {}
        Some(Err(error)) => {
            app.status.flash_toast(refusal(error));
            return;
        }
        None => return,
    }
    let (request_id, verdict) = (post.request_id.clone(), post.verdict);
    // The comments the preview was drawn from, so a save landing in between is refused, not posted
    let session_revision = app.review.session().map_or(0, |session| session.revision);
    if !app.send_action(
        Action::PostPullRequestReview { request_id, verdict, session_revision },
        "post the review",
    ) {
        return;
    }
    if let Some(post) = app.guided_review_workspace.github_post.as_mut() {
        post.posting = true;
        post.error = None;
    }
}

/// GitHub answered what the preview needs.
pub(crate) fn prepared(app: &mut App, request_id: &PullRequestPostId, prepared: PreparedPost) {
    let Some(post) = current(app, request_id) else {
        return;
    };
    if prepared.own_pull_request {
        post.verdict = ReviewEvent::Comment;
    }
    post.prepared = Some(prepared);
    post.error = None;
}

/// The review landed: close the preview and say where it is. The engine marked what it carried
/// and recorded the verdict; the session that says so arrives as a save.
pub(crate) fn posted(app: &mut App, request_id: &PullRequestPostId, comment: PostedComment) {
    if current(app, request_id).is_none() {
        return;
    }
    app.guided_review_workspace.github_post = None;
    let url = comment.url;
    app.push_convo(ConvoBlock::System(format!("[review] Posted to GitHub: {url}")));
    app.status.flash_toast("Review posted to GitHub · o opens it");
    app.guided_review_workspace.posted_review_url = Some(url);
}

/// Reading the pull request or posting failed; the preview stays open to say why.
pub(crate) fn failed(app: &mut App, request_id: &PullRequestPostId, message: String) {
    if let Some(post) = current(app, request_id) {
        post.posting = false;
        post.error = Some(message);
    }
}

/// The open preview, when the answer is for the attempt it is waiting on.
fn current<'a>(app: &'a mut App, request_id: &PullRequestPostId) -> Option<&'a mut GithubPost> {
    let post = app
        .guided_review_workspace
        .github_post
        .as_mut()
        .filter(|post| &post.request_id == request_id);
    if post.is_none() {
        tracing::debug!(?request_id, "ignoring an answer for a post no longer waiting");
    }
    post
}

/// Why the review cannot be posted as it stands, as a sentence.
pub(crate) fn refusal(error: PostPlanError) -> String {
    error.sentence()
}
