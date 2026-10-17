//! Guided Review results — a review loaded or failed, its guide written, its session restored or
//! saved, and its chapters committed.

use crate::app::{App, ConvoBlock, ReviewEditPurpose};
use luxe_core::config::NotificationEvent;
use luxe_core::event::GuidedReviewFailure;
use luxe_core::guided_review::{
    GuideProvenance, GuidedReview, ReviewBrief, ReviewChapter, ReviewId, ReviewLoadRequestId,
    ReviewRevisionId, ReviewSession, ReviewSessionSaveRequestId, ReviewTarget,
};

pub(super) fn loaded(app: &mut App, request_id: ReviewLoadRequestId, review: Box<GuidedReview>) {
    // Ours, or the one the engine restored at startup, which arrives under a request
    // nobody here sent: taken only while there is no review and none on its way, so it can
    // never displace one the reader asked for
    let restored_at_startup =
        app.active_guided_review_request.is_none() && app.guided_review.is_none();
    if app.active_guided_review_request.as_ref() == Some(&request_id) || restored_at_startup {
        app.active_guided_review_request = None;
        // A fresh source starts on structural chapters; the Guide pass reports back.
        app.review.provenance = None;
        app.review.attention = luxe_core::guided_review::review_attention(&review);
        app.review.flags = luxe_core::guided_review::review_flags(&review);
        app.review.budget = luxe_core::guided_review::review_budget(&review, &app.review.flags);
        app.guided_review_workspace.reconcile_chapter(&review.chapters);
        app.guided_review_workspace.reconcile_position(&review.files);
        app.guided_review_workspace.reconcile_walked_chapters(&review.chapters);
        // The desk follows with the session itself; a refresh keeps the one on screen until
        // then, a new review gets a placeholder
        app.initialize_review_session(&review);
        app.guided_review = Some(review);
        app.guided_review_workspace.mark_ready();
        // Captured is not readable yet: the guide pass starts now, and it is the long part
        let now = app.frame_now();
        if let Some(queued) = app.review.queued.as_mut() {
            if queued.state == crate::app::QueuedReviewState::Capturing {
                queued.state = crate::app::QueuedReviewState::Writing { since: now };
                app.status.hold_review_cost();
            }
        }
        // Captured, but not yet readable: the guide is still being written, and a review
        // that rewrites its own prose while someone reads it is the thing this waiting is
        // for. The row turns into an offer when the guide lands.
        // Someone arriving at a review they have not read gets chapter 01 rather than
        // whatever file happened to sort first; someone resuming keeps their place.
        app.with_review(|workspace, review| workspace.sync_narrative(review));
        open_pending_review_file(app);
    } else {
        tracing::debug!(?request_id, "ignoring stale guided review result");
    }
}

pub(super) fn guide_updated(
    app: &mut App,
    review_id: ReviewId,
    revision_id: ReviewRevisionId,
    chapters: Vec<ReviewChapter>,
    provenance: GuideProvenance,
    brief: Option<ReviewBrief>,
) {
    // Applied only to the exact revision it describes: a Guide computed for a source the
    // reviewer has already moved past would narrate the wrong diff.
    let Some(review) = app
        .guided_review
        .as_mut()
        .filter(|review| review.id == review_id && review.revision_id == revision_id)
    else {
        tracing::debug!(?review_id, "ignoring a Guide for a revision no longer on screen");
        return;
    };
    review.chapters = chapters;
    // Only ever an upgrade: a degraded pass returns no brief, and overwriting a good one
    // with `None` would take the reviewer's summary away on a refresh that went worse.
    if brief.is_some() {
        review.brief = brief;
    }
    app.review.provenance = Some(provenance);
    // Everything the review will show now exists, so the tray stops waiting and offers.
    // A guide pass always reports an outcome — "no better than structural" included — so
    // this arrives even when the model had nothing to add, and the offer is never stranded.
    let mut became_ready = false;
    if let Some(queued) = app.review.queued.as_mut() {
        if matches!(
            queued.state,
            crate::app::QueuedReviewState::Capturing
                | crate::app::QueuedReviewState::Writing { .. }
        ) {
            queued.state = crate::app::QueuedReviewState::Ready;
            became_ready = true;
        }
    }
    // Asked for by leaving a session to fix a pull request, so there is nothing else on
    // screen to take it away from: it opens once whole
    if std::mem::take(&mut app.review.open_when_ready)
        && app
            .review
            .queued
            .as_ref()
            .is_some_and(|queued| queued.state == crate::app::QueuedReviewState::Ready)
    {
        app.review.queued = None;
        app.current_view = crate::app::ViewMode::GuidedReview;
        app.dock.scroll = 0;
    }
    app.status.animating = false;
    app.with_review(|workspace, review| {
        workspace.reconcile_chapter(&review.chapters);
        workspace.reconcile_walked_chapters(&review.chapters);
    });
    // Chapter roll-ups are keyed by chapter id, so new sections need a fresh budget.
    if let Some(review) = app.guided_review.as_ref() {
        app.review.budget = luxe_core::guided_review::review_budget(review, &app.review.flags);
    }
    // A guide takes minutes to write, long enough to have gone and done something else;
    // this is the moment it becomes worth coming back for
    if became_ready {
        // Said in the transcript too, so the conversation records when the review it goes
        // on to discuss was written, the way it records the review being posted
        if let Some(review) = app.guided_review.as_ref() {
            let chapters = match review.chapters.len() {
                1 => "1 chapter".to_owned(),
                count => format!("{count} chapters"),
            };
            let label = crate::view::guided_review::review_label(review);
            app.push_convo(ConvoBlock::System(format!(
                "[review] Review of {label} is ready \u{b7} {chapters}"
            )));
        }
        super::turn::alert(app, NotificationEvent::Review, "Review ready");
    }
}

pub(super) fn committed(
    app: &mut App,
    request_id: ReviewSessionSaveRequestId,
    commits: Vec<String>,
    skipped: Vec<String>,
) {
    if app.review.commit_request.as_ref() != Some(&request_id) {
        tracing::debug!(?request_id, "ignoring a stale review-commit result");
        return;
    }
    app.review.commit_request = None;
    app.guided_review_workspace.close_commit_preview();
    let short: Vec<String> = commits.iter().map(|oid| oid.chars().take(8).collect()).collect();
    let mut message = match short.len() {
        0 => "[review] Nothing was committed.".to_owned(),
        1 => format!("[review] Committed {}.", short[0]),
        count => format!("[review] Committed {count} chapters: {}.", short.join(", ")),
    };
    if !skipped.is_empty() {
        // Named, never dropped: a binary or submodule change cannot be rebuilt from the
        // review model, and pretending otherwise would leave it silently uncommitted.
        message.push_str(&format!(
            " Still uncommitted (not representable as a patch): {}.",
            skipped.join(", ")
        ));
    }
    app.push_convo(ConvoBlock::System(message));
}

pub(super) fn commit_failed(
    app: &mut App,
    request_id: ReviewSessionSaveRequestId,
    message: String,
) {
    if app.review.commit_request.as_ref() != Some(&request_id) {
        tracing::debug!(?request_id, "ignoring a stale review-commit failure");
        return;
    }
    app.review.commit_request = None;
    if let Some(preview) = app.guided_review_workspace.commit_preview.as_mut() {
        preview.running = false;
    }
    app.push_convo(ConvoBlock::System(format!("[review] {message}. Nothing was committed.")));
}

pub(super) fn session_restored(app: &mut App, session: Box<ReviewSession>) {
    let session = *session;
    let matches_review = app.guided_review.as_ref().is_some_and(|review| {
        review.id == session.id && review.source.content_digest == session.source.content_digest
    });
    if !matches_review {
        tracing::debug!(
            review_id = session.id.as_str(),
            "ignored a session for a review that is not on screen"
        );
        return;
    }
    // Where the reading stopped comes back with the session the first time it binds to this
    // review, filtered to the chapters the pinned source still has. Not after a refresh:
    // the reader is mid-review, and the walk on screen is newer than the last one saved.
    if !app.review.bound {
        app.with_review(|workspace, review| {
            workspace.restore_progress(&session.progress, &review.chapters);
        });
    }
    app.review.session = Some(session);
    app.review.bound = true;
    crate::app::messages::pull_request_fix::adopt_carried(app);
}

pub(super) fn session_saved(
    app: &mut App,
    request_id: ReviewSessionSaveRequestId,
    session: Box<ReviewSession>,
) {
    let on_screen = app.guided_review.as_ref().is_some_and(|review| review.id == session.id);
    let purpose = app.review.edits.remove(&request_id);
    if !on_screen {
        tracing::debug!(?request_id, "ignoring a save for a review no longer on screen");
        return;
    }
    let session = *session;
    match purpose {
        Some(ReviewEditPurpose::Draft) => app.guided_review_workspace.discard_finding_draft(),
        Some(ReviewEditPurpose::Carried(carried)) => {
            crate::app::messages::pull_request_fix::carried_saved(app, &session, &carried);
        }
        Some(ReviewEditPurpose::Send | ReviewEditPurpose::Other) | None => {}
    }
    app.review.session = Some(session);
}

pub(super) fn session_save_failed(
    app: &mut App,
    request_id: ReviewSessionSaveRequestId,
    message: String,
) {
    match app.review.edits.remove(&request_id) {
        Some(ReviewEditPurpose::Draft) => {
            match app.guided_review_workspace.finding_draft.as_mut() {
                Some(draft) => draft.error = Some(message),
                None => {
                    app.push_convo(ConvoBlock::System(format!("[review feedback error] {message}")))
                }
            }
        }
        _ => app.push_convo(ConvoBlock::System(format!("[review feedback error] {message}"))),
    }
}

pub(super) fn failed(
    app: &mut App,
    request_id: ReviewLoadRequestId,
    target: ReviewTarget,
    error: GuidedReviewFailure,
) {
    if app.active_guided_review_request.as_ref() == Some(&request_id) {
        if let ReviewTarget::LastTurn { session_id, turn_id } = &target {
            if app.review.last_turn.as_ref() == Some(&(session_id.clone(), turn_id.clone())) {
                app.review.last_turn = None;
            }
        }
        app.active_guided_review_request = None;
        app.guided_review_workspace.mark_failed(target, error.clone());
        app.push_convo(ConvoBlock::System(format!("[review error] {}", error.message)));
    } else {
        tracing::debug!(?request_id, "ignoring stale guided review failure");
    }
}

/// Opens a freshly loaded review on the file a glance was escalated from.
///
/// The reader pressed "show me this properly" while looking at one file, and landing them on
/// whatever sorted first would throw away the only thing they had said about where to start. The
/// walkthrough is the mode that shows a file's diff, so the escalation selects it rather than
/// leaving the Brief on screen with the selection made invisibly behind it.
fn open_pending_review_file(app: &mut App) {
    let Some(path) = app.review.pending_file.take() else {
        return;
    };
    let Some(review) = app.guided_review.as_ref() else {
        return;
    };
    let Some(file) = review
        .files
        .iter()
        .find(|file| {
            [file.new_path.as_deref(), file.old_path.as_deref()]
                .into_iter()
                .flatten()
                .any(|candidate| luxe_core::paths::display(candidate) == path)
        })
        .map(|file| file.id.clone())
    else {
        return;
    };
    app.with_review(|workspace, review| workspace.select_file_by_id(review, &file));
}
