//! Mutants — the gaps `scripts/mutants.sh` found, and what a client may leave out.

use super::*;

/// A refresh while a fix turn is running keeps that turn running: only a session read back from
/// disk after a restart has turns that died with it.
///
/// The break this catches: telling a refresh from a restart wrong way round, which marks the live
/// turn retryable, says it was cut off, and invites sending the same feedback twice.
#[tokio::test]
async fn a_refresh_during_a_fix_turn_leaves_the_turn_running() {
    let review = review_from(TWO_FILES);
    let (project, mut desk) = desk_with(&review).await;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let (_, effects) = edit(
        &mut desk,
        ReviewEdit::Send { finding_ids: vec![finding_id], mode: None, outcome: None },
    )
    .await;
    let (submission_id, _) = submitted(&effects).unwrap();
    desk.handle(DeskCommand::Dispatched(submission_id)).await;

    let mut refreshed = review_from(&TWO_FILES.replace("+let c = 3;", "+let c = 4;"));
    refreshed.id = review.id.clone();
    let effects = desk
        .handle(DeskCommand::Publish {
            request_id: ReviewLoadRequestId::new(),
            review: Box::new(refreshed),
        })
        .await;
    assert!(
        !effects.iter().any(|effect| matches!(effect, Effect::Event(Event::Notice(_)))),
        "nothing was cut off"
    );
    let dispatch = stored(&project, &review).unwrap().submissions[0].dispatch;
    assert_eq!(dispatch, SubmissionDispatch::Dispatched);
}

/// A Guide for a revision the desk no longer holds changes nothing about the review it does hold,
/// though it is still passed on for the front end to discard.
#[tokio::test]
async fn a_guide_for_a_superseded_revision_leaves_the_held_chapters_alone() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let mut chapter = review.chapters[0].clone();
    chapter.id = crate::guided_review::ChapterId::from_items("stale guide", &[]);
    let effects = desk
        .handle(DeskCommand::GuideUpdated {
            request_id: ReviewLoadRequestId::new(),
            review_id: review.id.clone(),
            revision_id: ReviewRevisionId::from_canonical_bytes(b"an older revision"),
            chapters: vec![chapter.clone()],
            provenance: GuideProvenance::Model { batches: 1 },
            brief: None,
        })
        .await;
    assert!(matches!(effects.as_slice(), [Effect::Event(Event::GuidedReviewGuideUpdated { .. })]));
    let on_stale = draft(FindingAnchor::Chapter { chapter_id: chapter.id }, FindingSeverity::Issue);
    let (request_id, effects) =
        edit(&mut desk, ReviewEdit::Comment { draft: on_stale, send: false }).await;
    assert!(refusal_for(&effects, &request_id).contains("not in the review"));
}

/// A reply that went without a resolve, and a resolve without a reply, are each recorded: either
/// is something that landed on GitHub.
#[tokio::test]
async fn a_reply_alone_or_a_resolve_alone_is_recorded() {
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
    let (replied, resolved) = (thread("t1"), thread("t2"));
    edit(
        &mut desk,
        ReviewEdit::AdoptCarried { findings: vec![replied.clone(), resolved.clone()], send: false },
    )
    .await;
    desk.handle(DeskCommand::ThreadsAnswered(vec![
        ThreadAnswerResult {
            finding_id: replied.id.clone(),
            reply_url: Some("https://github.com/acme/app/pull/1#reply".into()),
            resolved: false,
            error: None,
        },
        ThreadAnswerResult {
            finding_id: resolved.id.clone(),
            reply_url: None,
            resolved: true,
            error: None,
        },
    ]))
    .await;
    let session = stored(&project, &review).unwrap();
    let github = |id: &FindingId| {
        session.findings.iter().find(|f| &f.id == id).and_then(|f| f.github_thread.clone()).unwrap()
    };
    assert!(github(&replied.id).reply_url.is_some());
    assert!(github(&resolved.id).resolved);
}

/// What the desk writes is stamped with the time it was written, not the epoch.
#[tokio::test]
async fn what_the_desk_writes_is_stamped_now() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    // Read from the clock, not from the desk's own reading of it
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis() as u64;
    let (finding_id, _) = commented(&mut desk, &review).await;
    let reply = ReviewEdit::Reply { finding_id, body: "Still wrong.".into(), send: true };
    let (request_id, effects) = edit(&mut desk, reply).await;
    let session = saved_for(&effects, &request_id);
    let stamped = session.findings[0].thread.last().expect("the reply").created_at_ms;
    assert!(stamped >= before, "{stamped} is before {before}");
    assert!(session.submissions[0].created_at_ms >= before);
    assert!(session.updated_at_ms >= before);
}

/// A client names a line by file, hunk, side and number; the desk fills in the digest and the
/// title. A number the hunk does not have is refused rather than guessed at.
#[tokio::test]
async fn a_line_named_without_its_digest_is_filled_in_from_the_review() {
    let review = review_from(TWO_FILES);
    let (_project, mut desk) = desk_with(&review).await;
    let FindingAnchor::Line { file_id, hunk_id, side, line, context_hash } = line_anchor(&review)
    else {
        unreachable!()
    };
    let named = |line: u32| FindingAnchor::Line {
        file_id: file_id.clone(),
        hunk_id: hunk_id.clone(),
        side,
        line,
        context_hash: String::new(),
    };
    let comment = |anchor| ReviewEdit::Comment {
        draft: CommentDraft {
            anchor,
            severity: FindingSeverity::Issue,
            title: String::new(),
            body: "\n  Keep the old value.\nIt is what callers expect.".into(),
            proposed_fix: None,
        },
        send: false,
    };
    let (request_id, effects) = edit(&mut desk, comment(named(line))).await;
    let stored = saved_for(&effects, &request_id).findings[0].clone();
    let FindingAnchor::Line { context_hash: filled, .. } = &stored.anchor else {
        panic!("still a line: {:?}", stored.anchor)
    };
    assert_eq!(filled, &context_hash, "the pinned review's digest for that line");
    assert_eq!(stored.title, "Keep the old value.", "the opening line, as the interface does");

    let (request_id, effects) = edit(&mut desk, comment(named(999))).await;
    assert!(refusal_for(&effects, &request_id).contains("not in the review"));
}
