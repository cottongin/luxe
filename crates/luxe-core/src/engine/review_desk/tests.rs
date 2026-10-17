//! Review desk tests — the feedback lifecycle, headless, against a temp project.
//!
//! Each test drives [`ReviewDesk::handle`] directly and reads what it says back, plus what reached
//! the disk: the durable session is the outcome, so a test that only read the events could pass
//! over a change that was announced and never stored.

use super::*;
use crate::guided_review::test_support::{review_from, TWO_FILES};
use crate::guided_review::{
    DiffSide, GitHubThread, RemediationResult, RemediationStatus, ReviewFeedbackMode,
};

/// A desk keeping its sessions in a temp project, with `review` open and nothing saved yet.
pub(super) async fn desk_with(review: &GuidedReview) -> (tempfile::TempDir, ReviewDesk) {
    let project = tempfile::tempdir().expect("temp project");
    let mut desk = ReviewDesk::new(Some(project.path().to_path_buf()));
    desk.handle(DeskCommand::Publish {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    })
    .await;
    (project, desk)
}

/// A line anchor on the first added line of the review's first file.
pub(super) fn line_anchor(review: &GuidedReview) -> FindingAnchor {
    let file = &review.files[0];
    let hunk = &file.hunks[0];
    let line = hunk.lines.iter().find(|line| line.new_line.is_some()).expect("an added line");
    FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side: DiffSide::New,
        line: line.new_line.expect("new side"),
        context_hash: line.context_hash(),
    }
}

pub(super) fn draft(anchor: FindingAnchor, severity: FindingSeverity) -> CommentDraft {
    CommentDraft {
        anchor,
        severity,
        title: "Keep the old value".into(),
        body: "This changes what callers see.".into(),
        proposed_fix: None,
    }
}

/// Sends `edit` for the open review, answering what came back.
pub(super) async fn edit(
    desk: &mut ReviewDesk,
    edit: ReviewEdit,
) -> (ReviewSessionSaveRequestId, Vec<Effect>) {
    let request_id = ReviewSessionSaveRequestId::new();
    let review_id = desk.review.as_ref().expect("a review is open").id.clone();
    let effects =
        desk.handle(DeskCommand::Edit { request_id: request_id.clone(), review_id, edit }).await;
    (request_id, effects)
}

/// The session a `review_session_saved` in `effects` carries, answering `request_id`.
pub(super) fn saved_for(
    effects: &[Effect],
    request_id: &ReviewSessionSaveRequestId,
) -> ReviewSession {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Event(Event::ReviewSessionSaved { request_id: answered, session })
                if answered == request_id =>
            {
                Some((**session).clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no save answered the edit: {effects:#?}"))
}

/// The message of the `review_session_save_failed` answering `request_id`.
pub(super) fn refusal_for(effects: &[Effect], request_id: &ReviewSessionSaveRequestId) -> String {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Event(Event::ReviewSessionSaveFailed {
                request_id: answered, message, ..
            }) if answered == request_id => Some(message.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no refusal answered the edit: {effects:#?}"))
}

/// How many events in `effects` answer `request_id` at all.
pub(super) fn answers(effects: &[Effect], request_id: &ReviewSessionSaveRequestId) -> usize {
    effects
        .iter()
        .filter(|effect| match effect {
            Effect::Event(Event::ReviewSessionSaved { request_id: answered, .. })
            | Effect::Event(Event::ReviewSessionSaveFailed { request_id: answered, .. }) => {
                answered == request_id
            }
            _ => false,
        })
        .count()
}

/// The submission an engine action in `effects` asks to queue.
pub(super) fn submitted(effects: &[Effect]) -> Option<(SubmissionId, ReviewFeedbackMode)> {
    effects.iter().find_map(|effect| match effect {
        Effect::Engine(action) => match action.as_ref() {
            Action::SubmitReviewFeedback { submission_id, mode, .. } => {
                Some((submission_id.clone(), *mode))
            }
            _ => None,
        },
        _ => None,
    })
}

/// What the store holds for `review`, read fresh from disk.
pub(super) fn stored(project: &tempfile::TempDir, review: &GuidedReview) -> Option<ReviewSession> {
    ReviewSessionStore::new(project.path()).load(&review.id).expect("the store reads")
}

/// Adds one issue comment and returns its id and the session that stored it.
pub(super) async fn commented(
    desk: &mut ReviewDesk,
    review: &GuidedReview,
) -> (FindingId, ReviewSession) {
    let comment = ReviewEdit::Comment {
        draft: draft(line_anchor(review), FindingSeverity::Issue),
        send: false,
    };
    let (request_id, effects) = edit(desk, comment).await;
    let session = saved_for(&effects, &request_id);
    (session.findings[0].id.clone(), session)
}

mod edits;
mod lifecycle;
mod mutants;
mod progress_and_threads;
mod publishing;
