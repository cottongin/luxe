//! Prompts — the permission and question modals a turn waits on, opened and taken down.
//!
//! A prompt can be answered somewhere else: a phone joined to the session, or any other front-end
//! on the same engine. The engine says when one stops waiting, and the modal goes with it.

use super::super::App;
use luxe_core::config::NotificationEvent;
use luxe_core::permission::PermissionRequestId;
use luxe_core::question::{QuestionOption, QuestionRequestId};

/// Opens the permission prompt the turn is waiting on, and tells the reader.
pub(super) fn permission_requested(
    app: &mut App,
    id: PermissionRequestId,
    tool: String,
    summary: String,
    always_patterns: Vec<String>,
) {
    // The prompt is a question to the user; nothing above it should still be arriving.
    app.reveal_catch_up();
    // The turn stops until someone answers, which is the moment most worth hearing about
    let body = super::turn::permission_body(&tool, &summary);
    super::turn::alert(app, NotificationEvent::Permission, &body);
    let prompt =
        crate::permission_prompt::PermissionPromptState::new(id, tool, summary, always_patterns);
    app.overlays.permission = Some(prompt.opened(app.frame_now()));
}

/// Opens the question the turn is waiting on, and tells the reader.
pub(super) fn question_asked(
    app: &mut App,
    id: QuestionRequestId,
    prompt: String,
    options: Vec<QuestionOption>,
    multi: bool,
) {
    app.reveal_catch_up();
    super::turn::alert(app, NotificationEvent::Question, &format!("Question: {prompt}"));
    app.overlays.question =
        Some(crate::question_prompt::QuestionPromptState::new(id, prompt, options, multi));
}

/// Takes down the prompt `id` names, once nothing is waiting on it.
///
/// Only that one: the reader may already be looking at the next prompt, and a question the TUI
/// asks itself (the plan handoff) has an id the engine never closes.
pub(super) fn prompt_closed(app: &mut App, id: &str) {
    if app.overlays.permission.as_ref().is_some_and(|prompt| prompt.id.as_str() == id) {
        app.overlays.permission = None;
    }
    if app.overlays.question.as_ref().is_some_and(|prompt| prompt.id.as_str() == id) {
        app.overlays.question = None;
    }
}
