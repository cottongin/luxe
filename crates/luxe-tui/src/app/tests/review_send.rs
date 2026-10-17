//! Review sends — what a submission carries, its outcome, and the finish and send dialogs.

use super::*;

#[test]
fn accepting_with_blocking_findings_requires_a_second_local_confirmation() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = luxe_core::guided_review::FindingState::Open;
    finding.severity = luxe_core::guided_review::FindingSeverity::Blocking;
    app.guided_review_workspace.open_finish_review(true);
    // Request fixes → Accept and commit → Accept.
    app.guided_review_workspace.move_finish_review_choice(false);
    app.guided_review_workspace.move_finish_review_choice(false);

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.guided_review_workspace.finish_review_accept_confirmed);
    assert!(engine_rx.try_recv().is_err());

    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(message))
            if message.contains("Recording acceptance with open comments")
                && !message.contains("Accepted with remaining")
    ));
    assert_eq!(
        sent_review_edit(&mut engine_rx).1,
        ReviewEdit::SetOutcome { outcome: luxe_core::guided_review::ReviewOutcome::Accepted }
    );
}

/// The review edit the app sent the engine, with the request it is answered under.
pub(super) fn sent_review_edit(
    engine_rx: &mut mpsc::Receiver<Action>,
) -> (ReviewSessionSaveRequestId, ReviewEdit) {
    match engine_rx.try_recv() {
        Ok(Action::EditReview { request_id, edit, .. }) => (request_id, edit),
        other => panic!("expected a review edit, got {other:?}"),
    }
}

#[test]
fn request_fixes_sends_with_a_changes_requested_outcome() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let finding_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .first()
        .expect("fixture finding")
        .id
        .clone();

    app.update(Msg::GuidedReviewQueueFeedback {
        mode: luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
        finding_ids: vec![finding_id.clone()],
        outcome: Some(luxe_core::guided_review::ReviewOutcome::ChangesRequested),
    });

    assert_eq!(
        sent_review_edit(&mut engine_rx).1,
        ReviewEdit::Send {
            finding_ids: vec![finding_id],
            mode: Some(luxe_core::guided_review::ReviewFeedbackMode::RequestFixes),
            outcome: Some(luxe_core::guided_review::ReviewOutcome::ChangesRequested),
        }
    );
}

/// A submission the restore marked retryable is offered for retry, and retried under its own id,
/// so its report, when it comes, still matches what was sent.
#[test]
fn a_restored_failed_submission_is_retried_under_its_own_id() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(2);
    app.engine_tx = engine_tx;
    let submission_id = luxe_core::guided_review::SubmissionId::new();
    let finding_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .first()
        .expect("fixture finding")
        .id
        .clone();
    let mut restored = app.review.session.clone().expect("review session");
    restored
        .queue_submission(
            submission_id.clone(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            0,
        )
        .expect("queue submission");
    restored.dispatch_submission(&submission_id).expect("dispatch submission");
    restored.fail_submission(&submission_id).expect("fail submission");
    restored.revision = 7;

    app.update(Msg::Engine(Event::ReviewSessionRestored { session: Box::new(restored) }));
    assert!(engine_rx.try_recv().is_err(), "a restore sends nothing by itself");
    assert_eq!(
        crate::app::input::retryable_submission_for_selected_finding(&app),
        Some(submission_id.clone())
    );

    app.update(Msg::GuidedReviewRetryFeedback(submission_id.clone()));
    assert_eq!(sent_review_edit(&mut engine_rx).1, ReviewEdit::RetrySend { submission_id });
}

/// Finish's "comment only" goes through the send preview, then sends the notes as comments with a
/// commented outcome; a failed turn is said in the transcript and the retry keeps its id.
#[test]
fn comment_only_sends_notes_with_a_commented_outcome() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(2);
    app.engine_tx = engine_tx;
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = luxe_core::guided_review::FindingState::Open;
    finding.severity = luxe_core::guided_review::FindingSeverity::Note;
    let finding_id = finding.id.clone();
    app.guided_review_workspace.open_finish_review(true);
    app.guided_review_workspace.move_finish_review_choice(true);

    // Finish's send choices route through the same preview as a mid-review send, so nothing is
    // dispatched unseen; Enter there confirms it.
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.guided_review_workspace.send_preview.is_some(), "the send preview opens first");
    assert!(engine_rx.try_recv().is_err(), "nothing is dispatched before confirmation");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    assert!(app.guided_review_workspace.send_preview.is_none());
    let (_, edit) = sent_review_edit(&mut engine_rx);
    let ReviewEdit::Send { finding_ids, mode, outcome } = edit else {
        panic!("comment-only sends the comments, got {edit:?}");
    };
    assert!(finding_ids.contains(&finding_id));
    assert_eq!(mode, Some(luxe_core::guided_review::ReviewFeedbackMode::CommentOnly));
    assert_eq!(outcome, Some(luxe_core::guided_review::ReviewOutcome::Commented));
    assert!(app.review_edit_pending(|purpose| *purpose == crate::app::ReviewEditPurpose::Send));
    // Sending hands the screen to the transcript, and leaving records what was read
    assert!(matches!(engine_rx.try_recv(), Ok(Action::RecordReviewSeen { .. })));

    // A turn that fails is said where the reader is, with what to do about it
    let submission_id = luxe_core::guided_review::SubmissionId::new();
    app.update(Msg::Engine(Event::ReviewFeedbackTurnFailed {
        submission_id: submission_id.clone(),
        message: "provider unavailable".into(),
    }));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text))
            if text.contains("provider unavailable") && text.contains("sent again")
    ));
    assert!(engine_rx.try_recv().is_err(), "a failure retries nothing by itself");
}

/// Setting an outcome asks the desk and leaves the session on screen alone until it answers.
#[test]
fn setting_an_outcome_waits_for_the_desk_to_store_it() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let before = app.review.session.clone().expect("review session");

    app.update(Msg::GuidedReviewSetOutcome(luxe_core::guided_review::ReviewOutcome::Accepted));

    assert_eq!(app.review.session, Some(before.clone()), "nothing changes before the answer");
    let (request_id, edit) = sent_review_edit(&mut engine_rx);
    assert_eq!(
        edit,
        ReviewEdit::SetOutcome { outcome: luxe_core::guided_review::ReviewOutcome::Accepted }
    );
    assert!(app.review_edit_pending(|_| true), "the review says it is saving");

    let mut stored = before;
    stored.set_outcome(luxe_core::guided_review::ReviewOutcome::Accepted);
    stored.revision += 1;
    app.update(Msg::Engine(Event::ReviewSessionSaved {
        request_id,
        session: Box::new(stored.clone()),
    }));
    assert_eq!(app.review.session, Some(stored));
    assert!(!app.review_edit_pending(|_| true));
}

#[test]
fn stale_finding_reanchor_shortcut_has_findings_drawer_precedence() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = luxe_core::guided_review::FindingState::Stale;
    let finding_id = finding.id.clone();
    app.guided_review_workspace.selected_finding_id = Some(finding_id.clone());
    app.guided_review_workspace.findings_visible = true;

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT)));

    assert!(matches!(
        sent_review_edit(&mut engine_rx).1,
        ReviewEdit::Reanchor { finding_id: moved, .. } if moved == finding_id
    ));
}

/// Re-anchoring a stale comment sends where the cursor is, and leaves the session on screen alone
/// until the desk has stored it.
#[test]
fn stale_finding_reanchor_sends_the_cursors_anchor() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = luxe_core::guided_review::FindingState::Stale;
    let finding_id = finding.id.clone();
    app.guided_review_workspace.selected_finding_id = Some(finding_id.clone());
    let expected_anchor = app
        .guided_review_workspace
        .selected_finding_anchor(&app.guided_review.as_ref().expect("guided review").files);
    let before = app.review.session.clone().expect("review session");

    app.update(Msg::GuidedReviewReanchorSelectedFinding);

    assert_eq!(app.review.session, Some(before));
    assert_eq!(
        sent_review_edit(&mut engine_rx).1,
        ReviewEdit::Reanchor { finding_id, anchor: expected_anchor }
    );
}

#[test]
fn one_submission_carries_every_unsent_comment_regardless_of_severity() {
    use luxe_core::guided_review::{FindingSeverity, FindingState};

    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;

    // Keep only the unsent comments, one per severity that asks for something different.
    {
        let session = app.review.session_mut().expect("review session");
        session
            .findings
            .retain(|finding| matches!(finding.state, FindingState::Draft | FindingState::Open));
        // The fixture ships one Draft and one Open comment; add a third so all three severities
        // that ask for something different are represented.
        let mut extra = session.findings[0].clone();
        extra.id = luxe_core::guided_review::FindingId::new();
        extra.label = String::new();
        extra.severity = FindingSeverity::Note;
        extra.title = "For the record".into();
        session.add_finding(extra).expect("add a third comment");
        session.findings[0].severity = FindingSeverity::Blocking;
        session.findings[1].severity = FindingSeverity::Suggestion;
        assert_eq!(session.findings.len(), 3, "three unsent comments, three severities");
    }

    let finding_ids = crate::app::input::unsent_finding_ids(&app);
    assert_eq!(finding_ids.len(), 3, "every unsent comment is a candidate");
    assert!(crate::app::input::has_actionable_findings(&app), "a blocking comment asks for work");

    app.update(Msg::GuidedReviewQueueFeedback {
        mode: luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
        finding_ids: finding_ids.clone(),
        outcome: None,
    });

    let ReviewEdit::Send { finding_ids: sent, .. } = sent_review_edit(&mut engine_rx).1 else {
        panic!("expected the comments to be sent");
    };
    assert_eq!(sent, finding_ids, "nothing was dropped by severity");
}

/// Progress used to live only in memory: comments survived a restart while it did not. What
/// persists now is which sections have been walked — not a per-hunk checklist.
#[test]
fn walked_chapters_persist_with_the_feedback_session() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;
    let review_id = app.guided_review.as_ref().expect("fixture review").id.clone();
    let chapter_id = app.guided_review.as_ref().expect("fixture review").chapters[0].id.clone();

    // Walking to the end of the only chapter records it, and the record is sent to be stored.
    app.update(Msg::GuidedReviewHunkNext);
    app.update(Msg::GuidedReviewHunkNext);
    assert!(app.guided_review_workspace.is_chapter_walked(&chapter_id));
    let Ok(Action::RecordReviewProgress { review_id: recorded, progress }) = engine_rx.try_recv()
    else {
        panic!("walking a chapter records the progress");
    };
    assert_eq!(recorded, review_id);
    assert_eq!(progress.walked_chapter_ids, vec![chapter_id]);
}

/// Restored progress keeps only the sections the pinned source still has.
#[test]
fn restored_progress_drops_chapters_that_are_gone() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    let (engine_tx, _engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;
    let surviving = app.guided_review.as_ref().expect("fixture review").chapters[0].id.clone();

    let mut restored = app.review.session.clone().expect("review session");
    restored.progress.walked_chapter_ids =
        vec![surviving.clone(), luxe_core::guided_review::ChapterId::from_items("core", &[])];

    app.update(Msg::Engine(Event::ReviewSessionRestored { session: Box::new(restored) }));

    assert!(app.guided_review_workspace.is_chapter_walked(&surviving));
    assert_eq!(
        app.guided_review_workspace.walked_chapters.len(),
        1,
        "the vanished chapter is dropped"
    );
}

/// Asking is a verb of its own: a question is sent with its save, in one edit, and never records
/// a review outcome. The desk sends it as a question, which forbids edits.
#[test]
fn a_question_is_sent_with_its_save() {
    use luxe_core::guided_review::FindingSeverity;

    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;

    app.update(Msg::GuidedReviewBeginQuestion);
    let draft = app.guided_review_workspace.finding_draft.as_mut().expect("open question");
    assert_eq!(draft.severity, FindingSeverity::Question);
    draft.set_body("Why an enum rather than a newtype here?");
    app.update(Msg::GuidedReviewSaveFinding);

    let ReviewEdit::Comment { draft, send } = sent_review_edit(&mut engine_rx).1 else {
        panic!("the question is a new comment");
    };
    assert_eq!(draft.severity, FindingSeverity::Question);
    assert!(send, "no separate send step for a question");
    assert!(engine_rx.try_recv().is_err(), "one edit, not a save and then a send");
}

/// Finishing opens on what the review most likely ends in: accepting when nothing is waiting to
/// be sent, fixes when comments are.
#[test]
fn finishing_opens_on_what_the_feedback_calls_for() {
    use crate::guided_review::FinishReviewChoice;
    let finish = || Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    let mut quiet =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    quiet.update(finish());
    assert_eq!(
        quiet.guided_review_workspace.finish_review_choice,
        Some(FinishReviewChoice::Accept)
    );

    let mut waiting =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    waiting.guided_review_workspace.findings_visible = false;
    waiting.update(finish());
    assert_eq!(
        waiting.guided_review_workspace.finish_review_choice,
        Some(FinishReviewChoice::RequestFixes)
    );
}

/// Sending is not finishing: a mid-review send leaves the outcome unset, and hands the screen to
/// the transcript, where the agent's work on it shows.
#[test]
fn a_mid_review_send_records_no_outcome() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;

    app.update(Msg::GuidedReviewOpenSendPreview);
    let preview = app.guided_review_workspace.send_preview.as_ref().expect("preview opens");
    assert!(preview.outcome.is_none(), "a mid-review send records nothing");
    // Nothing has been dispatched merely by looking at what would be sent.
    assert!(engine_rx.try_recv().is_err());

    app.update(Msg::GuidedReviewConfirmSend);
    let ReviewEdit::Send { outcome, finding_ids, .. } = sent_review_edit(&mut engine_rx).1 else {
        panic!("confirming the preview sends the comments");
    };
    assert!(!finding_ids.is_empty());
    assert!(outcome.is_none(), "the review stays open");
    assert!(app.guided_review_workspace.send_preview.is_none());
    // The agent's work is the next thing to watch, and it shows in the transcript
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    assert_eq!(app.status.toast(), Some("Sent to the agent · ctrl+g returns to the review"));
}

/// The feedback window sends from inside itself: `u` opens the send preview over the list, and
/// Esc there comes back to it. On someone else's pull request, whose comments go to GitHub, `u`
/// sends nothing and the window does not offer it.
#[test]
fn u_in_the_feedback_window_sends() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    assert!(app.guided_review_workspace.findings_visible);
    assert_eq!(crate::app::window_sends(&app), Some(2));
    app.update(key(KeyCode::Char('u')));
    assert!(app.guided_review_workspace.send_preview.is_some(), "u opens the send preview");
    app.update(key(KeyCode::Esc));
    assert!(app.guided_review_workspace.send_preview.is_none());
    assert!(app.guided_review_workspace.findings_visible, "Esc comes back to the list");
    app.update(key(KeyCode::Char('u')));
    app.update(key(KeyCode::Enter));
    assert!(!app.guided_review_workspace.findings_visible, "a send closes the list with it");
    app.guided_review_workspace.findings_visible = true;

    let review = app.guided_review.as_mut().expect("fixture review");
    review.source.base = luxe_core::guided_review::ReviewBase::PullRequest {
        number: 42,
        repository: "acme/notify".into(),
        base_ref: "main".into(),
        head_ref: "topic".into(),
        merge_base: "4f1c2d9a7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d".into(),
        head_oid: "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983".into(),
    };
    assert_eq!(crate::app::window_sends(&app), None, "its comments go to GitHub");
    app.update(key(KeyCode::Char('u')));
    assert!(app.guided_review_workspace.send_preview.is_none());
    assert!(app.guided_review_workspace.pull_request_fix.is_none(), "and u offers nothing else");
}

/// A pull request's review cannot send comments as fixes: the pull request is not checked out, so
/// the agent would change the working tree instead. Nothing reaches the agent, and the reader is
/// told why.
#[test]
fn a_pull_request_review_does_not_send_comments_as_fixes() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let review = app.guided_review.as_mut().expect("fixture review");
    review.source.base = luxe_core::guided_review::ReviewBase::PullRequest {
        number: 42,
        repository: "acme/notify".into(),
        base_ref: "main".into(),
        head_ref: "topic".into(),
        merge_base: "4f1c2d9a7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d".into(),
        head_oid: "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983".into(),
    };
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;

    // Alt+U offers a worktree, where the agent would have the code, before any preview opens
    app.update(Msg::GuidedReviewOpenSendPreview);
    assert!(app.guided_review_workspace.send_preview.is_none());
    assert!(matches!(engine_rx.try_recv(), Ok(luxe_core::Action::CheckPullRequestFix { .. })));
    // and a preview reached any other way is still refused at the send
    app.guided_review_workspace
        .open_send_preview(None, Some(luxe_core::guided_review::ReviewFeedbackMode::RequestFixes));
    app.update(Msg::GuidedReviewConfirmSend);

    assert!(engine_rx.try_recv().is_err(), "nothing is queued for the agent");
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview, "nothing went, so it stays");
    assert!(
        app.convo.iter().any(|item| matches!(item.block(), ConvoBlock::System(text)
            if text.contains("not checked out here"))),
        "the reader is told why"
    );
}

/// Excluding a row in the preview holds that comment back — and says so, rather than dropping it
/// silently the way severity routing used to.
#[test]
fn the_send_preview_holds_back_excluded_comments() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;
    let candidates = crate::app::input::unsent_finding_ids(&app);
    assert!(candidates.len() >= 2, "the fixture offers more than one unsent comment");

    app.update(Msg::GuidedReviewOpenSendPreview);
    app.update(Msg::GuidedReviewToggleSendPreviewRow);
    app.update(Msg::GuidedReviewConfirmSend);

    let ReviewEdit::Send { finding_ids, .. } = sent_review_edit(&mut engine_rx).1 else {
        panic!("the comments are sent");
    };
    assert_eq!(finding_ids, candidates[1..], "the excluded comment stayed behind");
}

/// A reviewer reply continues the thread of the comment it answers, rather than opening a second
/// comment about the same line, and travels at once. On a comment that already asks for work it
/// asks again with the thread; reopening a resolved one is the desk's to do.
#[test]
fn a_reply_continues_the_thread() {
    use luxe_core::guided_review::FindingState;

    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(4);
    app.engine_tx = engine_tx;
    let resolved = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == FindingState::Resolved)
        .expect("fixture resolved comment")
        .clone();
    app.guided_review_workspace.selected_finding_id = Some(resolved.id.clone());
    app.guided_review_workspace.findings_visible = true;

    app.update(Msg::GuidedReviewBeginReply);
    let draft = app.guided_review_workspace.finding_draft.as_mut().expect("reply composer");
    assert_eq!(draft.reply_to.as_ref(), Some(&resolved.id));
    let body = "This still looks wrong to me — the old path is unreachable now.";
    draft.set_body(body);
    app.update(Msg::GuidedReviewSaveFinding);

    assert_eq!(
        sent_review_edit(&mut engine_rx).1,
        ReviewEdit::AskForFix { finding_id: resolved.id, body: body.into(), send: true }
    );
}

/// A review's findings are said in the transcript, with where they went.
#[test]
fn a_review_s_findings_are_shown() {
    let (mut app, _rx) = mode_app();
    app.update(Msg::Engine(Event::ReviewFinished {
        verdict: luxe_core::event::RequestedReviewVerdict::Fix,
        findings: vec!["src/a.rs:3 unwraps".into()],
    }));
    let last = app.convo.last().map(|item| item.block().clone());
    assert!(
        matches!(&last, Some(ConvoBlock::System(text)) if text.contains("- src/a.rs:3 unwraps") && text.contains("agent has them")),
        "{last:?}"
    );
}
