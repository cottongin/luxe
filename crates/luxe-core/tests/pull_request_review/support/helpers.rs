//! Helpers — what more than one scenario group needs to set up or read back.

use std::sync::Arc;

use luxe_core::engine::Action;
use luxe_core::event::Event;
use luxe_core::github::{CheckRollup, GitHub, PullRequestState};
use luxe_core::guided_review::{GuidedReview, ReviewTarget};

use super::*;

/// Your own pull request, checked out on its branch at the head GitHub reports.
pub(crate) async fn own_review(
    remote: &Remote,
    github: Arc<ScriptedGitHub>,
) -> (Harness, GuidedReview) {
    git(&remote.project, &["fetch", "--quiet", "origin", "refs/pull/1/head:fix-parser"]);
    git(&remote.project, &["switch", "--quiet", "fix-parser"]);
    let mut h = harness(&remote.project, Some(github as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::OwnPullRequest { number: 1 }).await.unwrap();
    (h, review)
}

/// Commits every chapter of `review`, one commit or one per chapter, answering what was made.
pub(crate) async fn commit_review(
    h: &mut Harness,
    review: &GuidedReview,
    squash: bool,
) -> Result<Vec<String>, String> {
    let request_id = luxe_core::guided_review::ReviewSessionSaveRequestId::new();
    h.actions
        .send(Action::CommitReviewChapters {
            request_id: request_id.clone(),
            review_id: review.id.clone(),
            expected_revision: review.revision_id.clone(),
            chapter_ids: Vec::new(),
            squash,
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            match h.events.recv().await {
                Some(Event::ReviewCommitted { request_id: id, commits, .. })
                    if id == request_id =>
                {
                    break Ok(commits);
                }
                Some(Event::ReviewCommitFailed { request_id: id, message }) if id == request_id => {
                    break Err(message);
                }
                Some(_) => {}
                None => panic!("engine stopped"),
            }
        }
    })
    .await
    .unwrap()
}

/// A branch pushed, with a commit on it, and its review loaded.
pub(crate) async fn published_branch(
    remote: &Remote,
    github: Arc<ScriptedGitHub>,
) -> (Harness, GuidedReview) {
    git(&remote.project, &["switch", "--quiet", "-c", "keep-token"]);
    commit(&remote.project, "parser.rs", "fn parse() { keep_last_token(); }\n", "Keep it");
    git(&remote.project, &["push", "--quiet", "--set-upstream", "origin", "keep-token"]);
    let mut h = harness(&remote.project, Some(github as Arc<dyn GitHub>)).await;
    let review = h.load(ReviewTarget::Base { reference: "origin/main".into() }).await.unwrap();
    (h, review)
}

pub(crate) fn branch_pull_request(state: PullRequestState) -> luxe_core::github::PullRequestStatus {
    luxe_core::github::PullRequestStatus {
        number: 40,
        title: "Keep the token".into(),
        url: "https://github.com/acme/app/pull/40".into(),
        state,
        draft: false,
        review_decision: None,
        checks: CheckRollup::default(),
        unresolved_threads: 0,
        head_oid: String::new(),
        stack: None,
    }
}

pub(crate) fn finding(
    severity: luxe_core::guided_review::FindingSeverity,
) -> luxe_core::guided_review::SubmittedFinding {
    use luxe_core::guided_review::{FindingAnchor, FindingEvidence, FindingId, SubmittedFinding};
    SubmittedFinding {
        finding_id: FindingId::new(),
        label: "R1".into(),
        anchor: FindingAnchor::Review,
        evidence: FindingEvidence::default(),
        severity,
        title: "Why keep the last token?".into(),
        body: "What breaks without it?".into(),
        proposed_fix: None,
        thread: Vec::new(),
        github_author: None,
    }
}
