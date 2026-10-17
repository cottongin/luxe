//! Review drafts — the agent writes the summary of the reader's review, for them to edit and post.
//!
//! The one thing a reviewer asks the agent to write at the end of a review is the message that
//! goes with the verdict. `d` in the post preview runs a drafting turn — the session's own, so it
//! has the agent's reading of the change and its answers to the reader's questions, held to
//! reading — briefed with the verdict, the line comments that post with it and the guide's account
//! of the change. What comes back becomes the summary, shown whole in the preview, where `e` edits
//! it and Enter posts what the reader has just read.

use luxe_core::engine::{Action, DraftRequest, DraftTarget, ReviewDraft};
use luxe_core::github::ReviewEvent;
use luxe_core::guided_review::{FindingAnchor, FindingState, GuidedReview, ReviewBase};

use super::App;

/// Asks for a draft of the summary, carrying the reader's own summary when there is one.
pub(super) fn begin(app: &mut App) {
    let Some(review) = app.guided_review.as_ref() else { return };
    let ReviewBase::PullRequest { number, repository, .. } = &review.source.base else { return };
    let Some(post) = app.guided_review_workspace.github_post.as_ref() else { return };
    if post.posting || post.drafting.is_some() || post.prepared.is_none() {
        return;
    }
    let comments = match super::github_post::plan(app) {
        Some(Ok(plan)) => plan
            .review
            .comments
            .iter()
            .map(|comment| format!("{}:{} — {}", comment.path, comment.line, comment.body.trim()))
            .collect(),
        _ => Vec::new(),
    };
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request_id = format!("draft:review:{number}:{millis}");
    let request = DraftRequest {
        request_id: request_id.clone(),
        repository: Some(repository.clone()),
        number: *number,
        pull_request: true,
        target: DraftTarget::Review(ReviewDraft {
            verdict: verdict_label(post.verdict).to_string(),
            comments,
            guide: guide(review),
        }),
        conversation: Vec::new(),
        finding: None,
        note: None,
        current: summary(app).map(|finding| finding.body.clone()),
    };
    if !app.send_action(Action::DraftGithubReply(request), "draft a reply") {
        return;
    }
    let now = app.frame_now();
    app.status.hold_review_cost();
    if let Some(post) = app.guided_review_workspace.github_post.as_mut() {
        post.drafting = Some(request_id);
        post.drafting_since = Some(now);
        post.draft_note = None;
        // The summary is where the wait is drawn and where the draft lands
        post.selected = 0;
        post.scroll = 0;
    }
}

/// Makes a landed draft the summary, with what the agent said beside it, answering whether it was
/// this preview's.
///
/// Args: `(app, request_id, body, error, note)`.
pub(crate) fn landed(
    app: &mut App,
    request_id: &str,
    body: Option<String>,
    error: Option<String>,
    note: Option<String>,
) -> bool {
    let workspace = &mut app.guided_review_workspace;
    let Some(post) =
        workspace.github_post.as_mut().filter(|post| post.drafting.as_deref() == Some(request_id))
    else {
        return false;
    };
    post.drafting = None;
    post.drafting_since = None;
    post.draft_note = body.is_some().then_some(note).flatten();
    let Some(body) = body else {
        let reason = error.unwrap_or_else(|| "The agent wrote no summary.".to_string());
        app.status.flash_toast(reason);
        return true;
    };
    // Whatever the reader is typing is theirs; the draft waits for them to ask again
    if workspace.finding_draft.is_some() {
        app.status.flash_toast(
            "A summary is drafted; close what you are writing and press d".to_string(),
        );
        return true;
    }
    post.selected = 0;
    post.scroll = 0;
    // Through the composer's own save, so a drafted summary is stored like a written one
    let existing = summary(app).cloned();
    let workspace = &mut app.guided_review_workspace;
    let opened = match &existing {
        Some(finding) => workspace.begin_edit_finding_draft(finding),
        None => workspace.begin_review_summary(),
    };
    if let (true, Some(draft)) = (opened, workspace.finding_draft.as_mut()) {
        draft.set_body(body);
        app.update(super::Msg::GuidedReviewSaveFinding);
    }
    true
}

/// Opens the summary in its composer.
pub(super) fn edit(app: &mut App) {
    if app.guided_review_workspace.github_post.as_ref().is_some_and(|post| post.posting) {
        return;
    }
    if let Some(finding) = summary(app).cloned() {
        app.guided_review_workspace.begin_edit_finding_draft(&finding);
    }
}

/// The review's summary, while it can still change.
pub(crate) fn summary(app: &App) -> Option<&luxe_core::guided_review::ReviewFinding> {
    app.review.session()?.findings.iter().find(|finding| {
        finding.anchor == FindingAnchor::Review
            && finding.state == FindingState::Draft
            && finding.posted.is_none()
    })
}

/// The guide's account of the change: what it sets out to do, what it does, and its chapters.
fn guide(review: &GuidedReview) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(brief) = review.brief.as_ref() {
        lines.push(brief.intent.clone());
        lines.extend(brief.account.iter().map(|claim| format!("- {claim}")));
    }
    for chapter in &review.chapters {
        lines.push(format!("{}: {}", chapter.title, chapter.rationale));
    }
    let text = lines.join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// The verdict as GitHub's button says it.
fn verdict_label(verdict: ReviewEvent) -> &'static str {
    match verdict {
        ReviewEvent::Comment => "Comment",
        ReviewEvent::Approve => "Approve",
        ReviewEvent::RequestChanges => "Request changes",
    }
}
