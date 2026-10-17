//! Guided Review keys — its dialogs, its comment composer, and the findings panel.
//!
//! Review has its own modal stack (finish, commit and send previews, the comment draft) and its
//! own panel navigation. The order here is the precedence: the innermost dialog answers first.

use crate::guided_review::FinishReviewChoice as FinishChoice;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use luxe_core::keymap::UiAction;

use super::super::{App, ConvoBlock, Msg, ViewMode};
use super::{
    has_actionable_findings, queued_submission_for_selected_finding,
    retryable_submission_for_selected_finding, unsent_finding_ids,
};

/// Answers whether this layer took the key. Its dialogs and panels are drawn only in the review,
/// so only there do they take keys: one left open by a way out that does not close it would
/// otherwise confirm or delete unseen from the conversation.
pub(super) fn review_layer_took(app: &mut App, key: KeyEvent) -> bool {
    app.current_view == ViewMode::GuidedReview && review_took(app, key)
}

fn review_took(app: &mut App, key: KeyEvent) -> bool {
    app.guided_review_workspace.hidden_preview_takes(key)
        || finish_review_key(app, key)
        || stack_opening_key(app, key)
        || pull_request_fix_key(app, key)
        || pull_request_opening_key(app, key)
        || thread_answers_key(app, key)
        || github_post_key(app, key)
        || commit_preview_key(app, key)
        || send_preview_key(app, key)
        || finding_draft_key(app, key)
        || findings_window_key(app, key)
}

/// The finish dialog: move between its choices, and act on the one Enter lands on.
fn finish_review_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(choice) = app.guided_review_workspace.finish_review_choice else {
        return false;
    };
    if !key.modifiers.is_empty() {
        return true;
    }
    match key.code {
        KeyCode::Up => app.guided_review_workspace.move_finish_review_choice(false),
        KeyCode::Down => app.guided_review_workspace.move_finish_review_choice(true),
        KeyCode::Esc => app.guided_review_workspace.close_finish_review(),
        KeyCode::Enter => finish_review(app, choice),
        _ => {}
    }
    true
}

fn finish_review(app: &mut App, choice: FinishChoice) {
    use luxe_core::guided_review::{ReviewFeedbackMode, ReviewOutcome};
    let then = match choice {
        FinishChoice::Cancel => None,
        FinishChoice::AnswerOnGitHub => Some(Msg::GuidedReviewOpenThreadAnswers),
        FinishChoice::OpenPullRequest => Some(Msg::GuidedReviewOpenPullRequest),
        FinishChoice::OpenStack => Some(Msg::GuidedReviewOpenStack),
        FinishChoice::Accept => return accept_review(app),
        FinishChoice::AcceptAndCommit => {
            // Record the outcome first, then read the messages before any history is written.
            app.guided_review_workspace.close_finish_review();
            app.update(Msg::GuidedReviewSetOutcome(ReviewOutcome::Accepted));
            app.update(Msg::GuidedReviewOpenCommitPreview);
            return;
        }
        FinishChoice::RequestFixes if !has_actionable_findings(app) => {
            app.push_convo(ConvoBlock::System(
                "[review feedback] Add a Suggestion, Issue, or Blocking comment before requesting fixes."
                    .into(),
            ));
            return;
        }
        // Same preview as a mid-review send; only the recorded outcome differs.
        FinishChoice::RequestFixes => {
            let (outcome, mode) =
                (ReviewOutcome::ChangesRequested, ReviewFeedbackMode::RequestFixes);
            app.guided_review_workspace.open_send_preview(Some(outcome), Some(mode));
            return;
        }
        FinishChoice::CommentOnly if unsent_finding_ids(app).is_empty() => {
            app.push_convo(ConvoBlock::System(
                "[review feedback] Add a comment before submitting.".into(),
            ));
            return;
        }
        FinishChoice::CommentOnly => {
            let (outcome, mode) = (ReviewOutcome::Commented, ReviewFeedbackMode::CommentOnly);
            app.guided_review_workspace.open_send_preview(Some(outcome), Some(mode));
            return;
        }
    };
    app.guided_review_workspace.close_finish_review();
    if let Some(msg) = then {
        app.update(msg);
    }
}

/// Accepts the review, asking once more first when a blocking comment is still open, and saying
/// so when comments or chapters that need reading are left.
fn accept_review(app: &mut App) {
    use luxe_core::guided_review::{FindingSeverity, FindingState};
    let open = |state: FindingState| matches!(state, FindingState::Draft | FindingState::Open);
    let findings = app.review.session.iter().flat_map(|session| &session.findings);
    let has_open_blocking_finding = findings
        .clone()
        .any(|finding| open(finding.state) && finding.severity == FindingSeverity::Blocking);
    if has_open_blocking_finding && !app.guided_review_workspace.finish_review_accept_confirmed {
        app.guided_review_workspace.confirm_finish_review_accept();
        return;
    }
    let remaining_findings = findings.clone().any(|finding| open(finding.state));
    // Chapters that still ask something of the reviewer — not "unvisited hunks".
    // Accepting without reading the generated churn is a normal, correct review.
    let open_chapters = app.guided_review.as_ref().is_some_and(|review| {
        review.chapters.iter().any(|chapter| {
            luxe_core::guided_review::chapter_needs_review(&app.review.budget, chapter)
                && !app.guided_review_workspace.is_chapter_walked(&chapter.id)
        })
    });
    if remaining_findings || open_chapters {
        app.push_convo(ConvoBlock::System(
            "[review feedback] Recording acceptance with open comments or chapters you have not reviewed."
                .into(),
        ));
    }
    app.guided_review_workspace.close_finish_review();
    app.update(Msg::GuidedReviewSetOutcome(luxe_core::guided_review::ReviewOutcome::Accepted));
}

fn stack_opening_key(app: &mut App, key: KeyEvent) -> bool {
    if app.guided_review_workspace.stack_opening.is_none() {
        return false;
    }
    match key.code {
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelStack),
        KeyCode::Char('d') if key.modifiers.is_empty() => {
            app.update(Msg::GuidedReviewToggleStackDraft);
        }
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmStack),
        _ => {}
    }
    true
}

fn pull_request_fix_key(app: &mut App, key: KeyEvent) -> bool {
    if app.guided_review_workspace.pull_request_fix.is_none() {
        return false;
    }
    match key.code {
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelPullRequestFix),
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmPullRequestFix),
        _ => {}
    }
    true
}

/// The pull request preview, or the title or body field open in it.
fn pull_request_opening_key(app: &mut App, key: KeyEvent) -> bool {
    use crate::guided_review::OpeningField;
    let field_width = app.guided_review_workspace.field_width.get();
    let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() else {
        return false;
    };
    if let Some((field, editor)) = opening.editing.as_mut() {
        editor.set_viewport_width(field_width);
        match key.code {
            KeyCode::Enter if key.modifiers == KeyModifiers::CONTROL => {
                app.update(Msg::GuidedReviewSavePullRequestEdit);
            }
            // A title is one line, so Enter keeps it
            KeyCode::Enter if *field == OpeningField::Title => {
                app.update(Msg::GuidedReviewSavePullRequestEdit);
            }
            KeyCode::Esc => app.update(Msg::GuidedReviewCancelPullRequestEdit),
            KeyCode::Enter => editor.insert_newline(),
            _ => editor.handle_key(key),
        }
        return true;
    }
    let plain = key.modifiers.is_empty();
    match key.code {
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelPullRequest),
        KeyCode::Char('t') if plain => {
            app.update(Msg::GuidedReviewEditPullRequest(OpeningField::Title));
        }
        KeyCode::Char('e' | 'c') if plain => {
            app.update(Msg::GuidedReviewEditPullRequest(OpeningField::Body));
        }
        KeyCode::Char('d') if plain => app.update(Msg::GuidedReviewTogglePullRequestDraft),
        KeyCode::Char('E') => app.update(Msg::GuidedReviewEditPullRequestExternally),
        KeyCode::Up => app.update(Msg::GuidedReviewScrollPullRequest(-1)),
        KeyCode::Down => app.update(Msg::GuidedReviewScrollPullRequest(1)),
        KeyCode::PageUp => app.update(Msg::GuidedReviewScrollPullRequest(-10)),
        KeyCode::PageDown => app.update(Msg::GuidedReviewScrollPullRequest(10)),
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmPullRequest),
        _ => {}
    }
    true
}

/// Answering the pull request's threads, or the reply open under one.
fn thread_answers_key(app: &mut App, key: KeyEvent) -> bool {
    let field_width = app.guided_review_workspace.field_width.get();
    let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() else {
        return false;
    };
    if let Some(editor) = answers.replying.as_mut() {
        editor.set_viewport_width(field_width);
        match key.code {
            KeyCode::Enter if key.modifiers == KeyModifiers::CONTROL => {
                app.update(Msg::GuidedReviewSaveThreadReply);
            }
            KeyCode::Esc => app.update(Msg::GuidedReviewCancelThreadReply),
            // A plain Enter is a new line in a reply, never a send
            KeyCode::Enter => editor.insert_newline(),
            _ => editor.handle_key(key),
        }
        return true;
    }
    match key.code {
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelThreadAnswers),
        KeyCode::Up => app.update(Msg::GuidedReviewMoveThreadAnswers(false)),
        KeyCode::Down => app.update(Msg::GuidedReviewMoveThreadAnswers(true)),
        KeyCode::Char(' ') => app.update(Msg::GuidedReviewToggleThreadResolve),
        KeyCode::Char('c' | 'r') if key.modifiers.is_empty() => {
            app.update(Msg::GuidedReviewBeginThreadReply);
        }
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmThreadAnswers),
        _ => {}
    }
    true
}

/// The preview of a review going to GitHub. A summary written from it opens its composer over
/// it, and that composer takes the keys.
fn github_post_key(app: &mut App, key: KeyEvent) -> bool {
    let workspace = &app.guided_review_workspace;
    if workspace.github_post.is_none() || workspace.finding_draft.is_some() {
        return false;
    }
    let plain = key.modifiers.is_empty();
    let msg = match key.code {
        KeyCode::Esc => Msg::GuidedReviewCancelGithubPost,
        // One summary: once there is one, `e` changes it rather than `c` starting another
        KeyCode::Char('c') if plain && crate::app::review_draft_summary(app).is_none() => {
            Msg::GuidedReviewBeginPostSummary
        }
        KeyCode::Char('d') if plain => Msg::GuidedReviewDraftPostSummary,
        KeyCode::Char('e') if plain => Msg::GuidedReviewEditPostSummary,
        KeyCode::Char('w') if plain => Msg::GuidedReviewOpenPullRequestFix,
        // Tab, the way the @ picker moves between its tabs
        KeyCode::BackTab => Msg::GuidedReviewMoveGithubVerdict(false),
        KeyCode::Tab => Msg::GuidedReviewMoveGithubVerdict(true),
        KeyCode::Up => Msg::GuidedReviewMoveGithubPostItem(false),
        KeyCode::Down => Msg::GuidedReviewMoveGithubPostItem(true),
        KeyCode::PageUp => Msg::GuidedReviewScrollGithubPost(-10),
        KeyCode::PageDown => Msg::GuidedReviewScrollGithubPost(10),
        KeyCode::Enter => Msg::GuidedReviewConfirmGithubPost,
        _ => return true,
    };
    app.update(msg);
    true
}

fn commit_preview_key(app: &mut App, key: KeyEvent) -> bool {
    if app.guided_review_workspace.commit_preview.is_none() {
        return false;
    }
    match key.code {
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelCommit),
        KeyCode::Char('s') => app.update(Msg::GuidedReviewToggleCommitSquash),
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmCommit),
        _ => {}
    }
    true
}

fn send_preview_key(app: &mut App, key: KeyEvent) -> bool {
    if app.guided_review_workspace.send_preview.is_none() {
        return false;
    }
    match key.code {
        KeyCode::PageUp => app.guided_review_workspace.scroll_feedback(-10),
        KeyCode::PageDown => app.guided_review_workspace.scroll_feedback(10),
        KeyCode::Esc => app.update(Msg::GuidedReviewCancelSend),
        KeyCode::Up => app.update(Msg::GuidedReviewMoveSendPreview(false)),
        KeyCode::Down => app.update(Msg::GuidedReviewMoveSendPreview(true)),
        KeyCode::Char(' ') => app.update(Msg::GuidedReviewToggleSendPreviewRow),
        KeyCode::Enter => app.update(Msg::GuidedReviewConfirmSend),
        _ => {}
    }
    true
}

/// The comment composer. It owns two chords, save and discard; everything else goes to the real
/// editor — cursor, arrows, word operations, selection and Unicode included. There is one field,
/// so `Tab` is a tab, and nothing to pick, so `Shift+↑/↓` selects text like it does anywhere else.
fn finding_draft_key(app: &mut App, key: KeyEvent) -> bool {
    let field_width = app.guided_review_workspace.field_width.get();
    let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Enter if key.modifiers == KeyModifiers::CONTROL => {
            app.update(Msg::GuidedReviewSaveFinding);
        }
        KeyCode::Esc if key.modifiers.is_empty() => {
            app.guided_review_workspace.discard_finding_draft();
        }
        _ => {
            draft.set_width(field_width);
            draft.handle_key(key);
        }
    }
    true
}

/// The feedback window: its list, the selected comment's actions, and Esc backing out one layer
/// at a time.
fn findings_window_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.guided_review_workspace.findings_visible {
        return false;
    }
    if key.modifiers.is_empty() && finding_action_key(app, key.code) {
        return true;
    }
    if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::ALT {
        app.dispatch_action(UiAction::ReviewReanchorFinding);
        return true;
    }
    if key.code == KeyCode::Esc && key.modifiers.is_empty() {
        let workspace = &mut app.guided_review_workspace;
        if workspace.finding_fix_visible {
            workspace.finding_fix_visible = false;
        } else if workspace.finding_details_visible {
            workspace.finding_details_visible = false;
        } else {
            app.update(Msg::GuidedReviewToggleFindings);
        }
        return true;
    }
    // The window covers the diff, so a plain key it does not use would move something unseen
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}

/// A plain key the feedback window acts on. Answers whether it was one.
fn finding_action_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::PageUp => app.guided_review_workspace.scroll_feedback(-10),
        KeyCode::PageDown => app.guided_review_workspace.scroll_feedback(10),
        KeyCode::Up | KeyCode::Down => {
            if let Some(session) = app.review.session() {
                app.guided_review_workspace
                    .move_finding_selection(&session.findings, code == KeyCode::Down);
            }
        }
        KeyCode::Char('d') => app.guided_review_workspace.toggle_finding_details(),
        KeyCode::Char('v') => app.guided_review_workspace.toggle_finding_fix(),
        KeyCode::Char('e') => {
            if let Some(finding) = app.selected_finding().cloned() {
                app.guided_review_workspace.begin_edit_finding_draft(&finding);
            }
        }
        KeyCode::Char('r') => match retryable_submission_for_selected_finding(app)
            .or_else(|| queued_submission_for_selected_finding(app))
        {
            Some(submission_id) => app.update(Msg::GuidedReviewRetryFeedback(submission_id)),
            None => app.update(Msg::GuidedReviewToggleSelectedFindingResolved),
        },
        KeyCode::Char('x') => match queued_submission_for_selected_finding(app) {
            Some(submission_id) => app.update(Msg::GuidedReviewCancelQueuedFeedback(submission_id)),
            None => app.update(Msg::GuidedReviewDeleteSelectedFindingDraft),
        },
        KeyCode::Char('c') => app.update(Msg::GuidedReviewBeginReply),
        KeyCode::Char('a') => app.update(Msg::GuidedReviewBeginFollowUp),
        // Sending lives with the comments it sends; the preview opens over the list, and Esc
        // there comes back to it
        KeyCode::Char('u') if crate::app::window_sends(app).is_some() => {
            app.update(Msg::GuidedReviewOpenSendPreview);
            app.guided_review_workspace.findings_visible = true;
        }
        KeyCode::Enter => {
            if let Some(finding) = app.selected_finding().cloned() {
                app.with_review(|workspace, review| {
                    if workspace.jump_to_finding(&finding, review) {
                        workspace.close_feedback_window();
                    }
                });
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod modal_keys;
#[cfg(test)]
mod question_keys;
#[cfg(test)]
mod review_draft_keys;
