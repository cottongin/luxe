//! Answering your own pull request's threads — the preview, the replies, and the results.
//!
//! Only the reader answers: from this preview, with replies in their own words and nothing added.
//! It asks first whether the fix is on GitHub, so a thread is never marked done on the strength of
//! a commit that only exists here.

use luxe_core::engine::Action;
use luxe_core::guided_review::{ReviewTarget, ThreadAnswerResult, ThreadAnswersId, ThreadReply};

use super::{App, ConvoBlock};
use crate::guided_review::ThreadAnswers;

/// Whether the review on screen is your own pull request's, whose threads are answered here.
pub(crate) fn is_own_pull_request(app: &App) -> bool {
    app.guided_review
        .as_ref()
        .is_some_and(|review| matches!(review.target, ReviewTarget::OwnPullRequest { .. }))
}

/// Opens the preview on every thread not yet resolved on GitHub, and asks whether the fix is there.
pub(super) fn open(app: &mut App) {
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    let findings = app.review.session().map_or(&[][..], |session| &session.findings[..]);
    let request_id = ThreadAnswersId::new();
    let answers = ThreadAnswers::new(request_id.clone(), findings);
    if answers.rows.is_empty() {
        app.push_convo(ConvoBlock::System(
            "[review feedback] No threads are left to answer on GitHub.".into(),
        ));
        return;
    }
    if !app.send_action(Action::PrepareThreadAnswers { request_id, review_id }, "prepare answers") {
        return;
    }
    let workspace = &mut app.guided_review_workspace;
    workspace.help_visible = false;
    workspace.close_feedback_window();
    workspace.thread_answers = Some(answers);
}

/// Sends the answers as the preview shows them.
pub(super) fn confirm(app: &mut App) {
    let Some(answers) = app.guided_review_workspace.thread_answers.as_ref() else {
        return;
    };
    if answers.sending || answers.replying.is_some() {
        return;
    }
    // Named by comment: the engine reads which thread each one is from the session
    let to_send: Vec<ThreadReply> = answers
        .rows
        .iter()
        .filter(|row| row.resolve || !row.reply.trim().is_empty())
        .map(|row| ThreadReply {
            finding_id: row.finding_id.clone(),
            reply: Some(row.reply.clone()).filter(|reply| !reply.trim().is_empty()),
            resolve: row.resolve,
        })
        .collect();
    if to_send.is_empty() {
        app.status.flash_toast("Nothing to send · space resolve · c reply");
        return;
    }
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    let request_id = ThreadAnswersId::new();
    if !app.send_action(
        Action::AnswerThreads { request_id: request_id.clone(), review_id, answers: to_send },
        "answer threads",
    ) {
        return;
    }
    if let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() {
        answers.request_id = request_id;
        answers.sending = true;
        answers.error = None;
        for row in &mut answers.rows {
            row.error = None;
        }
    }
}

/// Opens the reply editor on the selected row, holding what was written before.
pub(super) fn begin_reply(app: &mut App) {
    let can_reply = selected_can_reply(app);
    let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() else {
        return;
    };
    if answers.sending {
        return;
    }
    if !can_reply {
        app.status.flash_toast("GitHub does not let this account reply to that thread");
        return;
    }
    let Some(row) = answers.rows.get(answers.selected) else {
        return;
    };
    let mut editor = crate::editor::Editor::new();
    editor.insert_text(&row.reply);
    answers.replying = Some(editor);
}

/// Keeps the reply being written and returns to the list.
pub(super) fn save_reply(app: &mut App) {
    let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() else {
        return;
    };
    let Some(editor) = answers.replying.take() else {
        return;
    };
    if let Some(row) = answers.rows.get_mut(answers.selected) {
        row.reply = editor.current_input().trim().to_string();
    }
}

/// Resolving is offered only where GitHub allows it.
pub(super) fn toggle_resolve(app: &mut App) {
    let can_resolve = selected_can_resolve(app);
    let conversation = selected_thread(app).is_some_and(|thread| thread.conversation);
    let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() else {
        return;
    };
    if answers.sending {
        return;
    }
    if conversation {
        app.status.flash_toast("A comment in the conversation has nothing to resolve: c replies");
        return;
    }
    if !can_resolve {
        app.status.flash_toast("GitHub does not let this account resolve that thread");
        return;
    }
    if let Some(row) = answers.rows.get_mut(answers.selected) {
        row.resolve = !row.resolve;
    }
}

/// GitHub said whether the fix is there.
pub(crate) fn prepared(
    app: &mut App,
    request_id: &ThreadAnswersId,
    pushed: bool,
    uncommitted: bool,
) {
    if let Some(answers) = current(app, request_id) {
        answers.prepared = Some((pushed, uncommitted));
    }
}

/// The answers were sent: keep the threads that failed on screen with why, and close once none are
/// left.
pub(crate) fn answered(
    app: &mut App,
    request_id: &ThreadAnswersId,
    results: Vec<ThreadAnswerResult>,
) {
    let Some(answers) = current(app, request_id) else {
        return;
    };
    answers.sending = false;
    let landed = results.iter().any(|result| result.reply_url.is_some() || result.resolved);
    for result in &results {
        let settled = result.error.is_none();
        if let Some(position) =
            answers.rows.iter().position(|row| row.finding_id == result.finding_id)
        {
            if settled {
                answers.rows.remove(position);
            } else {
                answers.rows[position].error = result.error.clone();
                // What did land is not sent twice
                if result.reply_url.is_some() {
                    answers.rows[position].reply.clear();
                }
            }
        }
    }
    answers.selected = answers.selected.min(answers.rows.len().saturating_sub(1));
    let failed = answers.rows.iter().filter(|row| row.error.is_some()).count();
    let finished = answers.rows.iter().all(|row| row.error.is_none());
    if finished {
        app.guided_review_workspace.thread_answers = None;
    }
    // The engine recorded what landed on the session; the thread count on the bar changed on
    // GitHub just now, and there is no reason to wait for the timer to show it
    if landed {
        app.send_quietly(Action::RefreshWorkspace, "refresh the workspace");
    }
    let answered = results.len() - failed;
    let summary = match (answered, failed) {
        (answered, 0) => format!("[review] Answered {} on GitHub.", threads(answered)),
        (answered, failed) => format!(
            "[review] Answered {} on GitHub; {} did not go, and say why.",
            threads(answered),
            threads(failed)
        ),
    };
    app.push_convo(ConvoBlock::System(summary));
}

/// Answering could not start.
pub(crate) fn failed(app: &mut App, request_id: &ThreadAnswersId, message: String) {
    if let Some(answers) = current(app, request_id) {
        answers.sending = false;
        answers.error = Some(message);
    }
}

fn current<'a>(app: &'a mut App, request_id: &ThreadAnswersId) -> Option<&'a mut ThreadAnswers> {
    let answers = app
        .guided_review_workspace
        .thread_answers
        .as_mut()
        .filter(|answers| &answers.request_id == request_id);
    if answers.is_none() {
        tracing::debug!(?request_id, "ignoring thread answers no longer waited on");
    }
    answers
}

fn selected_thread(app: &App) -> Option<&luxe_core::guided_review::GitHubThread> {
    let answers = app.guided_review_workspace.thread_answers.as_ref()?;
    let row = answers.rows.get(answers.selected)?;
    app.review
        .session()?
        .findings
        .iter()
        .find(|finding| finding.id == row.finding_id)?
        .github_thread
        .as_ref()
}

fn selected_can_reply(app: &App) -> bool {
    selected_thread(app).is_some_and(|thread| thread.can_reply)
}

fn selected_can_resolve(app: &App) -> bool {
    selected_thread(app).is_some_and(|thread| thread.can_resolve)
}

fn threads(count: usize) -> String {
    match count {
        1 => "1 thread".into(),
        count => format!("{count} threads"),
    }
}
