//! Progress and threads — what was seen, and your own pull request's threads across a refresh.

use super::*;

/// Progress is stored when it changes and is otherwise silent; the seen record covers exactly the
/// chapters it names.
#[tokio::test]
async fn progress_is_stored_when_it_changes_and_feeds_the_seen_record() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let walked = ReviewProgress {
        walked_chapter_ids: vec![review.chapters[0].id.clone()],
        ..ReviewProgress::default()
    };
    let effects = desk
        .handle(DeskCommand::RecordProgress {
            review_id: review.id.clone(),
            progress: walked.clone(),
        })
        .await;
    assert!(matches!(effects.as_slice(), [Effect::Event(Event::ReviewSessionSaved { .. })]));
    let again = desk
        .handle(DeskCommand::RecordProgress { review_id: review.id.clone(), progress: walked })
        .await;
    assert!(again.is_empty(), "stepping through a chapter must not write on every step");

    desk.handle(DeskCommand::RecordSeen { review_id: review.id.clone() }).await;
    let seen = SeenReviewStore::new(project.path()).load(&review.target).expect("a record");
    let expected = SeenReview::from_walked(&review, &[review.chapters[0].id.clone()], 0).unwrap();
    assert_eq!(seen.item_ids, expected.item_ids);
}

/// Progress for a review that is not open is dropped, not written over the open one's.
#[tokio::test]
async fn progress_for_another_review_is_dropped() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let effects = desk
        .handle(DeskCommand::RecordProgress {
            review_id: ReviewId::new(),
            progress: ReviewProgress {
                walked_chapter_ids: vec![review.chapters[0].id.clone()],
                ..ReviewProgress::default()
            },
        })
        .await;
    assert!(effects.is_empty());
    assert_eq!(stored(&project, &review), None);
}

/// Your own pull request's open threads join its session as findings, once: a refresh reads the
/// same threads again, and nothing is added twice.
#[tokio::test]
async fn your_own_pull_requests_threads_join_once_across_a_refresh() {
    use crate::github::{ReviewThread, ReviewThreads, ThreadComment, ThreadSide};
    let mut review = review_from(TWO_FILES);
    review.target = ReviewTarget::OwnPullRequest { number: 146 };
    let thread = |id: &str| ReviewThread {
        id: id.into(),
        path: "src/a.rs".into(),
        line: None,
        start_line: None,
        side: ThreadSide::Right,
        outdated: false,
        resolved: false,
        file_level: true,
        comments: vec![ThreadComment {
            author: "mara".into(),
            body: "Why does this change?".into(),
            created_at: String::new(),
            url: format!("https://github.com/acme/app/pull/146#discussion_{id}"),
        }],
        total_comments: 1,
        anchor_text: None,
        can_reply: true,
        can_resolve: true,
    };
    review.conversation = Some(crate::guided_review::earlier_conversation(
        &review.files,
        ReviewThreads { threads: vec![thread("t1"), thread("t2")], total: 2, ..Default::default() },
    ));
    let threads = |effects: &[Effect]| {
        effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Event(Event::ReviewSessionRestored { session }) => {
                    Some(session.findings.iter().filter(|f| f.github_thread.is_some()).count())
                }
                _ => None,
            })
            .expect("the session follows the review")
    };
    let project = tempfile::tempdir().unwrap();
    let mut desk = ReviewDesk::new(Some(project.path().to_path_buf()));
    let publish = |review: &GuidedReview| DeskCommand::Publish {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    };
    assert_eq!(threads(&desk.handle(publish(&review)).await), 2);
    // Saved with the first edit, then read again on refresh
    edit(&mut desk, ReviewEdit::SetOutcome { outcome: ReviewOutcome::Commented }).await;
    assert_eq!(threads(&desk.handle(publish(&review)).await), 2, "a refresh adds nothing twice");
    assert_eq!(
        stored(&project, &review)
            .unwrap()
            .findings
            .iter()
            .filter(|f| f.github_thread.is_some())
            .count(),
        2
    );
}

/// A fix the agent reported still waits for the reader after a refresh that left its line alone.
///
/// The break this catches: reconciling a refreshed source by resetting every finding, which would
/// throw away the "check this fix" the reader has not done yet.
#[tokio::test]
async fn a_reported_fix_still_waits_to_be_verified_after_a_refresh() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id.clone()], mode: None, outcome: None },
    )
    .await;
    desk.handle(DeskCommand::Remediation(RemediationReport {
        review_id: review.id.clone(),
        revision_id: review.revision_id.clone(),
        findings: vec![RemediationResult {
            finding_id: finding_id.clone(),
            status: RemediationStatus::Fixed,
            note: None,
        }],
    }))
    .await;
    // The other file changed; the commented line did not
    let mut refreshed = review_from(&TWO_FILES.replace("+let c = 3;", "+let c = 4;"));
    refreshed.id = review.id.clone();
    desk.handle(DeskCommand::Publish {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(refreshed.clone()),
    })
    .await;
    let session = stored(&project, &review).unwrap();
    assert_eq!(session.source, refreshed.source, "the refresh was stored");
    assert_eq!(session.findings[0].state, FindingState::NeedsVerification);
}

/// A reply on a resolved comment opens it again: saying something about it makes it live.
#[tokio::test]
async fn a_reply_reopens_a_resolved_comment() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    edit(&mut desk, ReviewEdit::Resolve { finding_id: finding_id.clone() }).await;
    let reply = ReviewEdit::Reply {
        finding_id: finding_id.clone(),
        body: "Still wrong: the old path is unreachable.".into(),
        send: false,
    };
    let (request_id, effects) = edit(&mut desk, reply).await;
    let finding = saved_for(&effects, &request_id).findings[0].clone();
    assert_eq!(finding.state, FindingState::Open);
    assert_eq!(
        finding.thread.last().map(|m| m.body.as_str()),
        Some("Still wrong: the old path is unreachable.")
    );
    assert!(submitted(&effects).is_none(), "a reply without send stays in the review");
}
