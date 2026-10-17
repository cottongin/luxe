//! Edits — a comment is stored and answered once, and what may not be edited.

use super::*;

/// A comment is stored, labeled, and answered exactly once under the caller's id.
#[tokio::test]
async fn a_comment_is_stored_and_answered_once() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let comment = ReviewEdit::Comment {
        draft: draft(line_anchor(&review), FindingSeverity::Issue),
        send: false,
    };
    let (request_id, effects) = edit(&mut desk, comment).await;
    assert_eq!(answers(&effects, &request_id), 1);
    let session = saved_for(&effects, &request_id);
    assert_eq!(session.findings.len(), 1);
    assert_eq!(session.findings[0].label, "R1");
    assert_eq!(session.findings[0].state, FindingState::Draft);
    assert_eq!(stored(&project, &review), Some(session));
    assert!(submitted(&effects).is_none(), "a comment waits until it is sent");
}

/// An edit that changes nothing is still answered, with the stored session, and writes nothing.
#[tokio::test]
async fn an_edit_that_changes_nothing_is_answered_without_a_write() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let resolve = || ReviewEdit::Resolve { finding_id: finding_id.clone() };
    let (_, effects) = edit(&mut desk, resolve()).await;
    let revision = stored(&project, &review).unwrap().revision;
    let (request_id, effects_again) = edit(&mut desk, resolve()).await;
    assert_eq!(answers(&effects_again, &request_id), 1, "a client waiting on it hears back");
    assert_eq!(saved_for(&effects_again, &request_id).revision, revision);
    assert_eq!(stored(&project, &review).unwrap().revision, revision, "and nothing was written");
    drop(effects);
}

/// A comment on a line the review does not have is refused, and nothing is stored.
///
/// The anchor names a real file but a hunk from a different diff: exactly what a client holding a
/// review from before a refresh would send.
#[tokio::test]
async fn a_comment_on_a_line_the_review_does_not_have_is_refused() {
    let review = review_from(TWO_FILES);
    let other = review_from(&TWO_FILES.replace("let a = 2;", "let a = 9;"));
    let (project, mut desk) = desk_with(&review).await;
    let comment = ReviewEdit::Comment {
        draft: draft(line_anchor(&other), FindingSeverity::Issue),
        send: true,
    };
    let (request_id, effects) = edit(&mut desk, comment).await;
    assert!(refusal_for(&effects, &request_id).contains("not in the review"));
    assert_eq!(answers(&effects, &request_id), 1);
    assert!(submitted(&effects).is_none());
    assert_eq!(stored(&project, &review), None);
}

/// An edit for a review that is not the open one is refused rather than applied to the wrong one.
#[tokio::test]
async fn an_edit_for_a_review_that_is_not_open_is_refused() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let request_id = ReviewSessionSaveRequestId::new();
    let effects = desk
        .handle(DeskCommand::Edit {
            request_id: request_id.clone(),
            review_id: ReviewId::new(),
            edit: ReviewEdit::SetOutcome { outcome: ReviewOutcome::Accepted },
        })
        .await;
    assert!(refusal_for(&effects, &request_id).contains("no longer open"));
    assert_eq!(stored(&project, &review), None);
}

/// A sent comment can no longer be edited or deleted; the refusal says what to do instead.
#[tokio::test]
async fn a_sent_comment_cannot_be_edited_or_deleted() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id.clone()], mode: None, outcome: None },
    )
    .await;
    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::DeleteComment { finding_id: finding_id.clone() }).await;
    assert!(refusal_for(&effects, &request_id).contains("reply to it"));
    let (request_id, effects) = edit(
        &mut desk,
        ReviewEdit::EditComment {
            finding_id,
            severity: FindingSeverity::Issue,
            title: "x".into(),
            body: "y".into(),
            proposed_fix: None,
        },
    )
    .await;
    assert!(refusal_for(&effects, &request_id).contains("reply to it"));
}

/// Sending stores the submission first, then asks the engine to queue its turn, asking for what
/// the strongest comment in it asks for.
#[tokio::test]
async fn sending_stores_the_submission_then_queues_its_turn() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (issue, _) = commented(&mut desk, &review).await;
    let question = ReviewEdit::Comment {
        draft: draft(FindingAnchor::Review, FindingSeverity::Question),
        send: false,
    };
    let (request_id, effects) = edit(&mut desk, question).await;
    let asked = saved_for(&effects, &request_id).findings[1].id.clone();

    let (request_id, effects) = edit(
        &mut desk,
        ReviewEdit::Send {
            finding_ids: vec![issue, asked],
            mode: None,
            outcome: Some(ReviewOutcome::ChangesRequested),
        },
    )
    .await;
    let session = saved_for(&effects, &request_id);
    let (submission_id, mode) = submitted(&effects).expect("the engine is asked to queue the turn");
    assert_eq!(
        mode,
        ReviewFeedbackMode::RequestFixes,
        "an issue beside a question asks for the fix"
    );
    assert_eq!(session.outcome, Some(ReviewOutcome::ChangesRequested));
    let position = |wanted: fn(&Effect) -> bool| effects.iter().position(wanted);
    assert!(
        position(|e| matches!(e, Effect::Event(Event::ReviewSessionSaved { .. })))
            < position(|e| matches!(e, Effect::Engine(action)
                if matches!(**action, Action::SubmitReviewFeedback { .. }))),
        "stored before it is sent, so a crash cannot send what was never recorded"
    );
    let durable = stored(&project, &review).unwrap();
    assert_eq!(durable.submissions[0].id, submission_id);
    assert_eq!(durable.submissions[0].dispatch, SubmissionDispatch::Queued);
}

/// `send` on a new comment queues it in the same save, as a question when it is one.
#[tokio::test]
async fn a_question_asked_with_send_goes_in_the_same_save() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let ask = ReviewEdit::Comment {
        draft: draft(line_anchor(&review), FindingSeverity::Question),
        send: true,
    };
    let (request_id, effects) = edit(&mut desk, ask).await;
    let session = saved_for(&effects, &request_id);
    assert_eq!(session.revision, 0, "one save, not a comment then a submission");
    assert_eq!(submitted(&effects).map(|(_, mode)| mode), Some(ReviewFeedbackMode::Question));
    assert_eq!(
        session.findings[0].state,
        FindingState::Open,
        "a sent comment is no longer a draft"
    );
    assert_eq!(stored(&project, &review).unwrap().submissions.len(), 1);
}

/// A reply sent on a question's thread turns it into an issue and asks for the fix with the thread.
#[tokio::test]
async fn asking_for_a_fix_on_a_question_sends_it_as_an_issue() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let ask = ReviewEdit::Comment {
        draft: draft(FindingAnchor::Review, FindingSeverity::Question),
        send: false,
    };
    let (request_id, effects) = edit(&mut desk, ask).await;
    let finding_id = saved_for(&effects, &request_id).findings[0].id.clone();
    let fix =
        ReviewEdit::AskForFix { finding_id, body: "Fix it in the router.".into(), send: true };
    let (request_id, effects) = edit(&mut desk, fix).await;
    assert_eq!(saved_for(&effects, &request_id).findings[0].severity, FindingSeverity::Issue);
    assert_eq!(submitted(&effects).map(|(_, mode)| mode), Some(ReviewFeedbackMode::RequestFixes));
}

/// Cancelling a queued submission is stored, then the engine is told to drop it.
#[tokio::test]
async fn cancelling_a_queued_submission_tells_the_engine_after_it_is_stored() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let (_, effects) = edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id], mode: None, outcome: None },
    )
    .await;
    let (submission_id, _) = submitted(&effects).unwrap();
    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::CancelSend { submission_id: submission_id.clone() }).await;
    assert!(saved_for(&effects, &request_id).submissions.is_empty());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Engine(action) if matches!(
            action.as_ref(),
            Action::CancelQueuedReviewFeedback { submission_id: dropped } if dropped == &submission_id
        )
    )));
    assert!(stored(&project, &review).unwrap().submissions.is_empty());
}

/// Carried comments join the review; the ones still unsent go to the agent and a stale one waits.
#[tokio::test]
async fn carried_comments_join_and_only_the_ones_still_placed_are_sent() {
    let review = review_from(TWO_FILES);
    let elsewhere = review_from(&TWO_FILES.replace("let a = 2;", "let a = 8;"));
    let (_project, mut desk) = desk_with(&review).await;
    let carried = |anchor: FindingAnchor, title: &str| ReviewFinding {
        id: FindingId::new(),
        label: "R1".into(),
        anchor,
        severity: FindingSeverity::Issue,
        title: title.into(),
        body: "Carried from the pull request.".into(),
        proposed_fix: None,
        state: FindingState::Draft,
        thread: Vec::new(),
        posted: None,
        github_thread: None,
    };
    let placed = carried(FindingAnchor::Review, "placed");
    let gone = carried(line_anchor(&elsewhere), "gone");
    let (request_id, effects) = edit(
        &mut desk,
        ReviewEdit::AdoptCarried { findings: vec![placed.clone(), gone.clone()], send: true },
    )
    .await;
    let session = saved_for(&effects, &request_id);
    assert_eq!(session.findings.len(), 2);
    let state_of = |id: &FindingId| session.findings.iter().find(|f| &f.id == id).unwrap().state;
    assert_eq!(state_of(&gone.id), FindingState::Stale);
    let sent: Vec<FindingId> =
        session.submissions[0].findings.iter().map(|f| f.finding_id.clone()).collect();
    assert_eq!(sent, vec![placed.id], "the stale one waits for the reader");
    assert!(submitted(&effects).is_some());
}

/// A revision conflict (another Luxe wrote first) refuses the edit, takes the stored session,
/// and lets the next edit through.
///
/// The break this catches: holding on to the older revision, which refuses every edit after it too.
#[tokio::test]
async fn a_revision_conflict_takes_the_stored_session_and_recovers() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (_, session) = commented(&mut desk, &review).await;
    let store = ReviewSessionStore::new(project.path());
    let mut theirs = session.clone();
    theirs.set_outcome(ReviewOutcome::Commented);
    store.save(Some(session.revision), &theirs, 1).expect("another writer saves");

    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::SetOutcome { outcome: ReviewOutcome::Accepted }).await;
    assert!(refusal_for(&effects, &request_id).contains("Another Luxe"));
    let restored = effects.iter().find_map(|effect| match effect {
        Effect::Event(Event::ReviewSessionRestored { session }) => Some(session.outcome),
        _ => None,
    });
    assert_eq!(restored, Some(Some(ReviewOutcome::Commented)), "the front end sees theirs");

    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::SetOutcome { outcome: ReviewOutcome::Accepted }).await;
    assert_eq!(saved_for(&effects, &request_id).outcome, Some(ReviewOutcome::Accepted));
}

/// With no project there is nowhere to keep comments, and every edit says so.
#[tokio::test]
async fn without_a_project_an_edit_is_refused_and_says_why() {
    let review = review_from(TWO_FILES);
    let mut desk = ReviewDesk::new(None);
    desk.handle(DeskCommand::Publish {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    })
    .await;
    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::SetOutcome { outcome: ReviewOutcome::Accepted }).await;
    assert!(refusal_for(&effects, &request_id).contains("without a project"));
}
