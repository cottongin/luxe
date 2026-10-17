//! Guided Review message handling — the edits a reviewer makes to an open session.
//!
//! Every edit is an intent sent to the engine's review desk, which applies it to the stored
//! session and answers with `ReviewSessionSaved` or `ReviewSessionSaveFailed`. Nothing here changes
//! the session it renders: the mirror only ever moves to what the desk stored, so a refused edit
//! leaves nothing half-applied, and edits made while the agent works apply in the order they were
//! made.
//!
//! `messages` keeps the dispatch table; the bodies live here.

use luxe_core::engine::{CommentDraft, ReviewEdit};
use luxe_core::guided_review::{FindingState, ReviewTarget, SubmissionId};

use super::{App, ConvoBlock};
use crate::app::input::unsent_finding_ids;
use crate::app::ReviewEditPurpose;
use crate::guided_review::{ReviewSourcePickerSelection, ReviewSourceReferenceKind};

/// Starts a review against whatever the source picker has highlighted.
///
/// Base and commit need a reference typed in first, so choosing either opens the entry field
/// rather than starting a review against nothing.
pub(super) fn select_source(app: &mut App) {
    let target = match app.guided_review_workspace.selected_source_picker() {
        Some(ReviewSourcePickerSelection::WorkingTree) => Some(ReviewTarget::WorkingTree),
        Some(ReviewSourcePickerSelection::LastTurn) => {
            app.review.last_turn.as_ref().map(|(session_id, turn_id)| ReviewTarget::LastTurn {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
            })
        }
        Some(ReviewSourcePickerSelection::Base) => {
            app.guided_review_workspace
                .begin_source_reference_entry(ReviewSourceReferenceKind::Base);
            None
        }
        Some(ReviewSourcePickerSelection::Commit) => {
            app.guided_review_workspace
                .begin_source_reference_entry(ReviewSourceReferenceKind::Commit);
            None
        }
        None => None,
    };
    if let Some(target) = target {
        crate::app::commands::start_guided_review(app, target);
    }
}

/// Saves the open comment draft: a new finding, an edit to one, or a reply on a thread.
///
/// Failures are reported on the draft itself, not just the log — a save that only logged made
/// Ctrl+Enter look like a dead key. The draft closes when its save lands, and not before.
pub(super) fn save_finding(app: &mut App) {
    if app.review_edit_pending(|purpose| *purpose == ReviewEditPurpose::Draft) {
        tracing::debug!("guided review finding persistence is already pending");
        if let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() {
            draft.error = Some("Still saving the last change — press again in a moment.".into());
        }
        return;
    }
    let editing_id = app.guided_review_workspace.editing_finding_id.clone();
    let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() else { return };
    // Checked here as well as by the desk, so the reason lands on the draft before a round trip
    let finding = match draft.to_review_finding(editing_id.clone().unwrap_or_default()) {
        Ok(finding) => finding,
        Err(error) => {
            tracing::debug!(%error, "guided review finding draft is not ready to save");
            draft.error = Some(draft_rejection_reason(&error).into());
            return;
        }
    };
    if app.review.session.is_none() {
        if let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() {
            draft.error = Some("No review session is open to save into.".into());
        }
        tracing::warn!("guided review finding draft has no active review session");
        return;
    }

    let draft = app.guided_review_workspace.finding_draft.as_ref();
    let reply_to = draft.and_then(|draft| draft.reply_to.clone());
    // A reply opened as a fix request on a question carries a severity the question does not
    let asks_for_fix = draft.is_some_and(|draft| {
        draft.reply_to.is_some()
            && draft.severity != luxe_core::guided_review::FindingSeverity::Question
    });
    // A question or a reply is worthless sitting in a draft, so it travels in the same save
    let send = draft.is_some_and(|draft| draft.dispatch_on_save);
    let edit = match (reply_to, editing_id) {
        // A reply continues an existing thread rather than opening a second comment about the
        // same line — that is what makes feedback a conversation.
        (Some(finding_id), _) if asks_for_fix => {
            ReviewEdit::AskForFix { finding_id, body: finding.body, send }
        }
        (Some(finding_id), _) => ReviewEdit::Reply { finding_id, body: finding.body, send },
        (None, Some(finding_id)) => ReviewEdit::EditComment {
            finding_id,
            severity: finding.severity,
            title: finding.title,
            body: finding.body,
            proposed_fix: finding.proposed_fix,
        },
        (None, None) => ReviewEdit::Comment {
            draft: CommentDraft {
                anchor: finding.anchor,
                severity: finding.severity,
                title: finding.title,
                body: finding.body,
                proposed_fix: finding.proposed_fix,
            },
            send,
        },
    };
    if !app.send_review_edit(ReviewEditPurpose::Draft, edit) {
        if let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() {
            draft.error = Some("This comment could not be saved against the review.".into());
        }
    }
}

/// Whether a `c` reply to `finding` asks for the fix rather than continuing the question.
///
/// A question's turn may only read, so once its answer says the code should change, replying is
/// how the reviewer asks for that, with the whole thread. Not on a pull request that is not checked
/// out: nothing here can fix it, and a reply there stays a question.
pub(crate) fn reply_asks_for_fix(
    app: &App,
    finding: &luxe_core::guided_review::ReviewFinding,
) -> bool {
    finding.severity == luxe_core::guided_review::FindingSeverity::Question
        && !super::github_post::is_pull_request(app)
}

/// Records the review's verdict (approve / request changes / comment).
pub(super) fn set_outcome(app: &mut App, outcome: luxe_core::guided_review::ReviewOutcome) {
    app.send_review_edit(ReviewEditPurpose::Other, ReviewEdit::SetOutcome { outcome });
}

/// Bundles findings into one submission for the agent, without ending the review.
///
/// Sending is not finishing: only the Finish dialog records an `outcome`, so firing off the fixes
/// one chapter needs does not stamp "changes requested" on a review that is two chapters in.
/// Answers whether the send went to the desk.
pub(super) fn queue_feedback(
    app: &mut App,
    mode: luxe_core::guided_review::ReviewFeedbackMode,
    finding_ids: Vec<luxe_core::guided_review::FindingId>,
    outcome: Option<luxe_core::guided_review::ReviewOutcome>,
) -> bool {
    app.send_review_edit(
        ReviewEditPurpose::Send,
        ReviewEdit::Send { finding_ids, mode: Some(mode), outcome },
    )
}

/// Withdraws a submission the agent has not started yet.
pub(super) fn cancel_queued_feedback(app: &mut App, submission_id: SubmissionId) {
    app.send_review_edit(ReviewEditPurpose::Other, ReviewEdit::CancelSend { submission_id });
}

/// Re-dispatches a submission whose agent turn failed.
pub(super) fn retry_feedback(app: &mut App, submission_id: SubmissionId) {
    app.send_review_edit(ReviewEditPurpose::Send, ReviewEdit::RetrySend { submission_id });
}

/// Flips the selected finding between open and resolved.
pub(super) fn toggle_selected_finding_resolved(app: &mut App) {
    let Some(finding) = app.selected_finding() else { return };
    let finding_id = finding.id.clone();
    let edit = match finding.state == FindingState::Resolved {
        true => ReviewEdit::Reopen { finding_id },
        false => ReviewEdit::Resolve { finding_id },
    };
    app.send_review_edit(ReviewEditPurpose::Other, edit);
}

/// Discards the selected finding, which must still be an unsent draft.
pub(super) fn delete_selected_finding_draft(app: &mut App) {
    let Some(finding_id) = app.guided_review_workspace.selected_finding_id.clone() else { return };
    app.send_review_edit(ReviewEditPurpose::Other, ReviewEdit::DeleteComment { finding_id });
}

/// Re-points a finding whose anchor a refresh invalidated at the cursor's current line.
pub(super) fn reanchor_selected_finding(app: &mut App) {
    let Some(finding_id) = app.guided_review_workspace.selected_finding_id.clone() else { return };
    let Some(review) = app.guided_review.as_ref() else {
        tracing::warn!("cannot reanchor a finding without an active guided review");
        return;
    };
    let anchor = app.guided_review_workspace.selected_finding_anchor(&review.files);
    app.send_review_edit(ReviewEditPurpose::Other, ReviewEdit::Reanchor { finding_id, anchor });
}

/// Why a comment draft could not be saved, in reviewer-facing words.
fn draft_rejection_reason(
    error: &luxe_core::guided_review::ReviewSessionValidationError,
) -> &'static str {
    use luxe_core::guided_review::ReviewSessionValidationError as Invalid;
    match error {
        Invalid::EmptyFindingBody | Invalid::EmptyFindingTitle => {
            "Write the comment before saving."
        }
        Invalid::InvalidLineAnchor | Invalid::MissingContextHash => {
            "This anchor can no longer be saved against the pinned review."
        }
        Invalid::UnsupportedSchema { .. } | Invalid::DuplicateId { .. } => {
            "This comment cannot be saved as written."
        }
    }
}

/// Dispatches exactly what the preview shows, deriving the mode from what is included.
pub(super) fn confirm_send(app: &mut App) {
    let Some(preview) = app.guided_review_workspace.send_preview.clone() else {
        return;
    };
    let included: Vec<_> = unsent_finding_ids(app)
        .into_iter()
        .filter(|finding_id| !preview.excluded.contains(finding_id))
        .collect();
    if included.is_empty() {
        app.push_convo(ConvoBlock::System("[review feedback] Nothing is selected to send.".into()));
        return;
    }
    app.guided_review_workspace.close_send_preview();
    // Opened from the feedback window, which waits underneath for a cancel, not a send
    app.guided_review_workspace.close_feedback_window();
    // The agent's work on it is the next thing worth watching, and it shows in the transcript
    if send_findings(app, included, preview.mode, preview.outcome) {
        crate::app::navigation::exit_guided_review(app);
        app.status.flash_toast("Sent to the agent · ctrl+g returns to the review");
    }
}

/// Sends `included` to the agent as one submission, asking for what the strongest of them asks
/// for unless `mode` says otherwise. Answers whether it went to the engine.
fn send_findings(
    app: &mut App,
    included: Vec<luxe_core::guided_review::FindingId>,
    mode: Option<luxe_core::guided_review::ReviewFeedbackMode>,
    outcome: Option<luxe_core::guided_review::ReviewOutcome>,
) -> bool {
    use luxe_core::guided_review::ReviewFeedbackMode;
    let mode = mode.unwrap_or_else(|| {
        ReviewFeedbackMode::asked_by(
            app.review
                .session
                .iter()
                .flat_map(|session| &session.findings)
                .filter(|finding| included.contains(&finding.id))
                .map(|finding| finding.severity),
        )
    });
    // Said here, before anything is queued; the engine refuses it too, since this is where the
    // reader is and the engine is where the rule holds
    let pull_request = app.guided_review.as_ref().is_some_and(|review| {
        matches!(review.source.base, luxe_core::guided_review::ReviewBase::PullRequest { .. })
    });
    if pull_request && mode == ReviewFeedbackMode::RequestFixes {
        app.push_convo(ConvoBlock::System(format!(
            "[review feedback] {}",
            luxe_core::engine::PULL_REQUEST_FIXES_REFUSED
        )));
        return false;
    }
    queue_feedback(app, mode, included, outcome)
}

/// Writes the commits the preview shows, through the engine so git never runs on the UI thread.
pub(super) fn confirm_commit(app: &mut App) {
    // the review first: a preview marked running with nothing to commit would stay running
    let Some((review_id, expected_revision)) =
        app.guided_review.as_ref().map(|review| (review.id.clone(), review.revision_id.clone()))
    else {
        return;
    };
    let Some(preview) = app.guided_review_workspace.commit_preview.as_mut() else {
        return;
    };
    if preview.running {
        return;
    }
    let squash = preview.squash;
    preview.running = true;
    let request_id = luxe_core::guided_review::ReviewSessionSaveRequestId::new();
    if !app.send_action(
        luxe_core::engine::Action::CommitReviewChapters {
            request_id: request_id.clone(),
            review_id,
            expected_revision,
            chapter_ids: Vec::new(),
            squash,
        },
        "commit the review",
    ) {
        app.guided_review_workspace.close_commit_preview();
        app.push_convo(ConvoBlock::System(
            "[review] Could not start the commit. Nothing was changed.".into(),
        ));
        return;
    }
    app.review.commit_request = Some(request_id);
}

/// Opens the review's page on GitHub: the review just posted, or else the pull request.
pub(super) fn open_in_browser(app: &mut App) {
    if let Some(url) = app.guided_review_workspace.posted_review_url.clone() {
        app.open_externally(std::ffi::OsStr::new(&url), &url);
        return;
    }
    match app.guided_review.as_ref().and_then(|review| review.context.pull_request.as_ref()) {
        Some(pr) if !pr.url.is_empty() => {
            let url = pr.url.clone();
            app.open_externally(std::ffi::OsStr::new(&url), &url);
        }
        _ => app.status.flash_toast("Only a pull request review has a page to open".to_string()),
    }
}

/// How many comments the feedback window's `u` would send, or `None` where it sends nothing: on
/// someone else's pull request, whose comments go to GitHub instead, or with nothing unsent.
pub(crate) fn window_sends(app: &App) -> Option<usize> {
    let unsent = unsent_finding_ids(app).len();
    (!super::github_post::is_pull_request(app) && unsent > 0).then_some(unsent)
}
