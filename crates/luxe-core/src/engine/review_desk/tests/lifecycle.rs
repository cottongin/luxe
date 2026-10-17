//! Lifecycle — a turn is stored from dispatch to its answer, and what a report or a post marks.

use super::*;

/// A submission's turn: dispatched, answered, completed, each stored as it happens, with the
/// answer on the question it was asked about.
#[tokio::test]
async fn a_question_turn_is_stored_from_dispatch_to_its_answer() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let ask = ReviewEdit::Comment {
        draft: draft(FindingAnchor::Review, FindingSeverity::Question),
        send: true,
    };
    let (_, effects) = edit(&mut desk, ask).await;
    let (submission_id, _) = submitted(&effects).unwrap();
    let state = || {
        stored(&project, &review)
            .and_then(|session| session.submissions.first().map(|submission| submission.dispatch))
    };

    desk.handle(DeskCommand::Dispatched(submission_id.clone())).await;
    assert_eq!(state(), Some(SubmissionDispatch::Dispatched));
    let held = desk
        .handle(DeskCommand::Answered {
            submission_id: submission_id.clone(),
            answer: "It runs twice because the router repeats it.".into(),
        })
        .await;
    assert!(held.is_empty(), "an answer is held for its completion, not saved on its own");
    let effects = desk.handle(DeskCommand::Completed(submission_id)).await;
    assert!(effects.iter().any(|e| matches!(e, Effect::Event(Event::ReviewSessionSaved { .. }))));
    assert_eq!(state(), Some(SubmissionDispatch::Completed));
    let thread = &stored(&project, &review).unwrap().findings[0].thread;
    assert_eq!(
        thread.last().map(|message| message.body.as_str()),
        Some("It runs twice because the router repeats it.")
    );
}

/// A failed turn leaves its submission retryable, including one refused before it dispatched.
///
/// The second case is the pull-request fix refusal: it used to stay `Queued` for good, which no
/// retry could move.
#[tokio::test]
async fn a_failed_turn_is_retryable_even_when_it_never_dispatched() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let (_, effects) = edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id], mode: None, outcome: None },
    )
    .await;
    let (submission_id, _) = submitted(&effects).unwrap();

    desk.handle(DeskCommand::Failed(submission_id.clone())).await;
    let dispatch = stored(&project, &review).unwrap().submissions[0].dispatch;
    assert_eq!(dispatch, SubmissionDispatch::FailedRetryable, "refused while still queued");

    edit(&mut desk, ReviewEdit::RetrySend { submission_id: submission_id.clone() }).await;
    let dispatched = desk.handle(DeskCommand::Dispatched(submission_id.clone())).await;
    assert!(dispatched.is_empty(), "a retry dispatched it already; nothing new to store");
    desk.handle(DeskCommand::Failed(submission_id)).await;
    let dispatch = stored(&project, &review).unwrap().submissions[0].dispatch;
    assert_eq!(dispatch, SubmissionDispatch::FailedRetryable, "failed again after dispatch");
}

/// The agent's report goes on the comments it names, and a fix asks the reader to verify it.
#[tokio::test]
async fn a_remediation_report_lands_on_the_comments_it_names() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id.clone()], mode: None, outcome: None },
    )
    .await;
    let report = RemediationReport {
        review_id: review.id.clone(),
        revision_id: review.revision_id.clone(),
        findings: vec![RemediationResult {
            finding_id,
            status: RemediationStatus::Fixed,
            note: Some("Kept the old value.".into()),
        }],
    };
    desk.handle(DeskCommand::Remediation(report)).await;
    let finding = &stored(&project, &review).unwrap().findings[0];
    assert_eq!(finding.state, FindingState::NeedsVerification);
    assert!(finding.thread.iter().any(|message| message.body.contains("Kept the old value.")));
}

/// A report about some other review changes nothing and is nobody's error.
#[tokio::test]
async fn a_report_about_another_review_changes_nothing() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    commented(&mut desk, &review).await;
    let before = stored(&project, &review);
    let report = RemediationReport {
        review_id: ReviewId::new(),
        revision_id: review.revision_id.clone(),
        findings: Vec::new(),
    };
    assert!(desk.handle(DeskCommand::Remediation(report)).await.is_empty());
    assert_eq!(stored(&project, &review), before);
}

/// What went to GitHub is marked posted, with the verdict it carried.
#[tokio::test]
async fn a_posted_review_marks_its_comments_and_records_the_verdict() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let comment =
        PostedComment { review_id: 99, url: "https://github.com/acme/app/pull/1#r99".into() };
    desk.handle(DeskCommand::Posted {
        finding_ids: vec![finding_id],
        comment: comment.clone(),
        outcome: ReviewOutcome::ChangesRequested,
    })
    .await;
    let session = stored(&project, &review).unwrap();
    assert_eq!(session.findings[0].posted, Some(comment));
    assert_eq!(session.outcome, Some(ReviewOutcome::ChangesRequested));
    assert!(session.unsent_finding_ids().is_empty(), "a posted comment is not offered again");
}

/// A thread answered on GitHub records the reply and the resolve; one that failed records nothing.
#[tokio::test]
async fn an_answered_thread_records_what_landed_and_nothing_else() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let thread = |id: &str| ReviewFinding {
        id: FindingId::new(),
        label: String::new(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Issue,
        title: "Thread".into(),
        body: "From GitHub.".into(),
        proposed_fix: None,
        state: FindingState::Open,
        thread: Vec::new(),
        posted: None,
        github_thread: Some(GitHubThread {
            id: id.into(),
            url: format!("https://github.com/acme/app/pull/1#{id}"),
            author: "mara".into(),
            can_reply: true,
            can_resolve: true,
            reply_url: None,
            resolved: false,
            conversation: false,
        }),
    };
    let (answered, failed) = (thread("t1"), thread("t2"));
    edit(
        &mut desk,
        ReviewEdit::AdoptCarried { findings: vec![answered.clone(), failed.clone()], send: false },
    )
    .await;
    desk.handle(DeskCommand::ThreadsAnswered(vec![
        ThreadAnswerResult {
            finding_id: answered.id.clone(),
            reply_url: Some("https://github.com/acme/app/pull/1#reply".into()),
            resolved: true,
            error: None,
        },
        ThreadAnswerResult {
            finding_id: failed.id.clone(),
            reply_url: None,
            resolved: false,
            error: Some("GitHub refused".into()),
        },
    ]))
    .await;
    let session = stored(&project, &review).unwrap();
    let github = |id: &FindingId| {
        session.findings.iter().find(|f| &f.id == id).and_then(|f| f.github_thread.clone()).unwrap()
    };
    assert!(github(&answered.id).resolved);
    assert!(github(&answered.id).reply_url.is_some());
    assert!(!github(&failed.id).resolved && github(&failed.id).reply_url.is_none());
}
