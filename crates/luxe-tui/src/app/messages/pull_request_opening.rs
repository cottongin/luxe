//! Opening a branch's pull request — the draft, the reader's edits, and the answer.
//!
//! The engine drafts the pull request from the review and says what opening it involves: commits
//! to write first, a branch to push. The reader edits the title and the description here, and what
//! opens is exactly what the preview shows.

use luxe_core::engine::Action;
use luxe_core::github::PullRequestState;
use luxe_core::guided_review::{PullRequestOpenId, ReviewOutcome};
use luxe_core::workspace::PullRequestPresence;

use super::{App, ConvoBlock, Msg};
use crate::guided_review::{OpeningField, PreparedOpening, PullRequestOpening};

/// Whether the finish chooser offers opening a pull request: a review of local work on a branch
/// GitHub has no open pull request for, as far as the status row knows.
pub(crate) fn offered(app: &App) -> bool {
    if super::thread_answers::is_own_pull_request(app) || super::github_post::is_pull_request(app) {
        return false;
    }
    match app.workspace.as_ref().map(|snapshot| &snapshot.pull_request) {
        Some(PullRequestPresence::None) => true,
        Some(PullRequestPresence::Found(found)) => found.state != PullRequestState::Open,
        _ => false,
    }
}

/// Opens the preview and asks the engine for the draft.
pub(super) fn open(app: &mut App) {
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    let request_id = PullRequestOpenId::new();
    if !app.send_action(
        Action::PreparePullRequest { request_id: request_id.clone(), review_id },
        "prepare a pull request",
    ) {
        return;
    }
    let workspace = &mut app.guided_review_workspace;
    workspace.help_visible = false;
    workspace.close_feedback_window();
    workspace.pull_request_opening = Some(PullRequestOpening {
        request_id,
        prepared: None,
        editing: None,
        body_scroll: 0,
        body_room: std::cell::Cell::new(0),
        opening: false,
        error: None,
    });
}

/// Opens an editor on the title or the description, holding what is there.
pub(super) fn begin_edit(app: &mut App, field: OpeningField) {
    let Some(opening) = idle(app) else {
        return;
    };
    let Some(prepared) = opening.prepared.as_ref() else {
        return;
    };
    let mut editor = crate::editor::Editor::new();
    editor.insert_text(match field {
        OpeningField::Title => &prepared.pull_request.title,
        OpeningField::Body => &prepared.pull_request.body,
    });
    opening.editing = Some((field, editor));
}

/// Keeps what was written in the field being edited.
pub(super) fn save_edit(app: &mut App) {
    let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() else {
        return;
    };
    let (Some((field, editor)), Some(prepared)) =
        (opening.editing.take(), opening.prepared.as_mut())
    else {
        return;
    };
    let written = editor.current_input();
    match field {
        // One line: a title that wrapped onto a second would be cut by GitHub
        OpeningField::Title => {
            prepared.pull_request.title = written.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        OpeningField::Body => prepared.pull_request.body = written.trim_end().to_string(),
    }
}

/// Hands the description to the reader's editor.
pub(super) fn edit_externally(app: &mut App) {
    let Some(prepared) = idle(app).and_then(|opening| opening.prepared.as_ref()) else {
        return;
    };
    let text = prepared.pull_request.body.clone();
    app.outbox.text_edit = Some(crate::launcher::TextEdit {
        text,
        target: crate::launcher::EditTarget::PullRequestBody,
    });
}

/// Takes the description back from the reader's editor. Nothing is kept for a preview that
/// closed or started opening while the editor was up.
pub(super) fn edited(app: &mut App, text: String) {
    let Some(opening) = idle(app) else {
        return;
    };
    let Some(prepared) = opening.prepared.as_mut() else {
        return;
    };
    prepared.pull_request.body = text;
    opening.body_scroll = 0;
}

/// Scrolls the description by `rows`, down when positive, within what it was last drawn with.
pub(super) fn scroll(app: &mut App, rows: i32) {
    if let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() {
        let room = i32::from(opening.body_room.get());
        let at = i32::from(opening.body_scroll).saturating_add(rows).clamp(0, room);
        opening.body_scroll = u16::try_from(at).unwrap_or(0);
    }
}

/// Switches between opening ready for review and opening as a draft.
pub(super) fn toggle_draft(app: &mut App) {
    if let Some(prepared) = idle(app).and_then(|opening| opening.prepared.as_mut()) {
        prepared.pull_request.draft = !prepared.pull_request.draft;
    }
}

/// Opens what the preview shows, or asks for the draft again when reading it failed.
pub(super) fn confirm(app: &mut App) {
    let Some(opening) = idle(app) else {
        return;
    };
    let Some(prepared) = opening.prepared.as_ref() else {
        if opening.error.is_some() {
            open(app);
        }
        return;
    };
    if prepared.pull_request.title.trim().is_empty() {
        app.status.flash_toast("A pull request needs a title · t writes one");
        return;
    }
    // The engine kept the draft; what goes back is only what the reader could change
    let action = Action::OpenPullRequest {
        request_id: opening.request_id.clone(),
        title: prepared.pull_request.title.clone(),
        body: prepared.pull_request.body.clone(),
        draft: prepared.pull_request.draft,
    };
    if !app.send_action(action, "open a pull request") {
        return;
    }
    if let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() {
        opening.opening = true;
        opening.error = None;
    }
}

/// Closes the preview, unless it is opening.
pub(super) fn cancel(app: &mut App) {
    let workspace = &mut app.guided_review_workspace;
    if workspace.pull_request_opening.as_ref().is_some_and(|opening| !opening.opening) {
        workspace.pull_request_opening = None;
    }
}

/// The engine drafted the pull request.
pub(crate) fn prepared(app: &mut App, request_id: &PullRequestOpenId, prepared: PreparedOpening) {
    if let Some(opening) = current(app, request_id) {
        opening.prepared = Some(prepared);
        opening.error = None;
    }
}

/// It opened: record the review as accepted, say where it is, and close the preview.
pub(crate) fn opened(
    app: &mut App,
    request_id: &PullRequestOpenId,
    number: u64,
    url: String,
    committed: usize,
) {
    if current(app, request_id).is_none() {
        return;
    }
    app.guided_review_workspace.pull_request_opening = None;
    app.update(Msg::GuidedReviewSetOutcome(ReviewOutcome::Accepted));
    let first = match committed {
        0 => String::new(),
        1 => "Committed the reviewed work and ".to_string(),
        count => format!("Committed the reviewed work as {count} commits and "),
    };
    let opened = if first.is_empty() { "Opened" } else { "opened" };
    app.push_convo(ConvoBlock::System(format!("[review] {first}{opened} #{number}: {url}")));
    app.status.flash_toast(format!("Pull request #{number} opened · o opens it"));
    app.guided_review_workspace.posted_review_url = Some(url);
}

/// Drafting or opening failed; the preview stays open to say why.
pub(crate) fn failed(app: &mut App, request_id: &PullRequestOpenId, message: String) {
    if let Some(opening) = current(app, request_id) {
        opening.opening = false;
        opening.error = Some(message);
    }
}

/// The open preview when nothing is in flight, for a change the reader makes.
fn idle(app: &mut App) -> Option<&mut PullRequestOpening> {
    app.guided_review_workspace
        .pull_request_opening
        .as_mut()
        .filter(|opening| !opening.opening && opening.editing.is_none())
}

/// The open preview, when the answer is for the attempt it is waiting on.
fn current<'a>(
    app: &'a mut App,
    request_id: &PullRequestOpenId,
) -> Option<&'a mut PullRequestOpening> {
    let opening = app
        .guided_review_workspace
        .pull_request_opening
        .as_mut()
        .filter(|opening| &opening.request_id == request_id);
    if opening.is_none() {
        tracing::debug!(?request_id, "ignoring an answer for an opening no longer waiting");
    }
    opening
}
