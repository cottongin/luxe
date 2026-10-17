//! Publishing — a review reaches the desk new, refreshed, restored or upgraded.

use super::*;

/// A review is announced, then always the session bound to it: new, and not written anywhere
/// until something is said in it.
#[tokio::test]
async fn a_new_review_is_followed_by_a_new_session_that_is_not_saved_yet() {
    let review = review_from(TWO_FILES);
    let project = tempfile::tempdir().unwrap();
    let mut desk = ReviewDesk::new(Some(project.path().to_path_buf()));
    let effects = desk
        .handle(DeskCommand::Publish {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(review.clone()),
        })
        .await;
    let kinds: Vec<String> = effects
        .iter()
        .map(|effect| match effect {
            Effect::Event(event) => serde_json::to_value(event).unwrap()["kind"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            Effect::Engine(_) => "engine".to_string(),
        })
        .collect();
    assert_eq!(kinds, ["guided_review_loaded", "review_session_restored"]);
    let Effect::Event(Event::ReviewSessionRestored { session }) = &effects[1] else {
        unreachable!()
    };
    assert_eq!(session.id, review.id);
    assert!(session.findings.is_empty());
    assert_eq!(stored(&project, &review), None, "an empty review writes nothing");
}

/// A refresh keeps the session and moves its comments onto the new source, saving that.
///
/// The break this catches: a refresh that starts a fresh session (every comment gone) or keeps the
/// old source (a later restore refuses the session as belonging to another revision).
#[tokio::test]
async fn a_refresh_keeps_the_comments_and_moves_them_onto_the_new_source() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;

    let mut refreshed = review_from(&TWO_FILES.replace("let a = 2;", "let a = 3;"));
    refreshed.id = review.id.clone();
    let effects = desk
        .handle(DeskCommand::Publish {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(refreshed.clone()),
        })
        .await;
    let restored = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Event(Event::ReviewSessionRestored { session }) => Some((**session).clone()),
            _ => None,
        })
        .expect("the session follows the review");
    assert_eq!(restored.source, refreshed.source);
    let finding = restored.findings.iter().find(|f| f.id == finding_id).expect("the comment stays");
    assert_eq!(finding.state, FindingState::Stale, "its line changed, so it waits to be re-placed");
    assert_eq!(stored(&project, &review), Some(restored), "and the move is what is stored");
}

/// After a restart the stored session comes back with the review, and a turn that was running
/// when the process died is retryable rather than "working" forever or sent again by itself.
#[tokio::test]
async fn a_restart_restores_the_comments_and_makes_an_interrupted_turn_retryable() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let (_, effects) = edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id], mode: None, outcome: None },
    )
    .await;
    let (submission_id, _) = submitted(&effects).expect("queued for the agent");
    desk.handle(DeskCommand::Dispatched(submission_id.clone())).await;
    drop(desk);

    let mut restarted = ReviewDesk::new(Some(project.path().to_path_buf()));
    let effects = restarted
        .handle(DeskCommand::Publish {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(review.clone()),
        })
        .await;
    assert!(
        effects.iter().any(|effect| matches!(effect, Effect::Event(Event::Notice(_)))),
        "the reader is told a turn was cut off"
    );
    assert!(submitted(&effects).is_none(), "nothing is sent again without being asked");
    let state = |session: &ReviewSession| {
        session.submissions.iter().find(|s| s.id == submission_id).map(|s| s.dispatch)
    };
    assert_eq!(
        stored(&project, &review).as_ref().and_then(state),
        Some(SubmissionDispatch::FailedRetryable)
    );
    let (request_id, effects) =
        edit(&mut restarted, ReviewEdit::RetrySend { submission_id: submission_id.clone() }).await;
    assert_eq!(state(&saved_for(&effects, &request_id)), Some(SubmissionDispatch::Dispatched));
    assert_eq!(submitted(&effects).map(|(id, _)| id), Some(submission_id), "same id, sent again");
}

/// A Guide upgrade reaches the review the desk holds, so a comment on one of the new chapters is
/// one the desk recognizes.
///
/// The break this catches: the stale copy the engine used to keep, which still had the structural
/// chapters and would refuse every comment written against the Guide on screen.
#[tokio::test]
async fn a_guide_upgrade_reaches_the_review_the_desk_holds() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let mut chapter = review.chapters[0].clone();
    chapter.id = crate::guided_review::ChapterId::from_items("guide", &[]);
    chapter.items = review.chapters.iter().flat_map(|c| c.items.clone()).collect();
    let effects = desk
        .handle(DeskCommand::GuideUpdated {
            request_id: ReviewLoadRequestId::new(),
            review_id: review.id.clone(),
            revision_id: review.revision_id.clone(),
            chapters: vec![chapter.clone()],
            provenance: GuideProvenance::Model { batches: 1 },
            brief: None,
        })
        .await;
    assert!(matches!(effects.as_slice(), [Effect::Event(Event::GuidedReviewGuideUpdated { .. })]));
    let on_chapter =
        draft(FindingAnchor::Chapter { chapter_id: chapter.id }, FindingSeverity::Issue);
    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::Comment { draft: on_chapter, send: false }).await;
    assert_eq!(saved_for(&effects, &request_id).findings.len(), 1);
}
