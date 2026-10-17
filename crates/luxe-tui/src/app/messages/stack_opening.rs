//! Opening a branch's review as a stack — the layers drafted from its chapters, and the answer.
//!
//! The engine drafts one pull request per chapter and says whether it can be made; the reader
//! sees every layer, may make them drafts, and Enter opens exactly those.

use luxe_core::engine::Action;
use luxe_core::guided_review::{ChapterKind, PullRequestOpenId, ReviewOutcome};

use super::{App, ConvoBlock, Msg};
use crate::guided_review::StackOpening;

/// Whether the finish chooser offers a stack: where it offers a pull request, and the review has
/// two chapters worth a pull request each.
pub(crate) fn offered(app: &App) -> bool {
    super::pull_request_opening::offered(app)
        && app.guided_review.as_ref().is_some_and(|review| {
            review.chapters.iter().filter(|chapter| chapter.kind != ChapterKind::Generated).count()
                >= 2
        })
}

/// Opens the preview and asks the engine for the layers.
pub(super) fn open(app: &mut App) {
    let Some(review_id) = app.guided_review.as_ref().map(|review| review.id.clone()) else {
        return;
    };
    let request_id = PullRequestOpenId::new();
    if !app.send_action(
        Action::PrepareStack { request_id: request_id.clone(), review_id },
        "prepare a stack",
    ) {
        return;
    }
    let workspace = &mut app.guided_review_workspace;
    workspace.help_visible = false;
    workspace.close_feedback_window();
    workspace.stack_opening =
        Some(StackOpening { request_id, layers: None, draft: false, opening: false, error: None });
}

pub(super) fn toggle_draft(app: &mut App) {
    if let Some(stack) = app.guided_review_workspace.stack_opening.as_mut().filter(|s| !s.opening) {
        stack.draft = !stack.draft;
    }
}

/// Opens the layers as the preview shows them.
pub(super) fn confirm(app: &mut App) {
    let Some(stack) = app.guided_review_workspace.stack_opening.as_ref() else {
        return;
    };
    if stack.layers.is_none() || stack.opening {
        return;
    }
    // The engine kept the layers; the reader chose only whether they are drafts
    let action = Action::OpenStack { request_id: stack.request_id.clone(), draft: stack.draft };
    if !app.send_action(action, "open a stack") {
        return;
    }
    if let Some(stack) = app.guided_review_workspace.stack_opening.as_mut() {
        stack.opening = true;
        stack.error = None;
    }
}

pub(super) fn cancel(app: &mut App) {
    let workspace = &mut app.guided_review_workspace;
    if workspace.stack_opening.as_ref().is_some_and(|stack| !stack.opening) {
        workspace.stack_opening = None;
    }
}

/// The engine drafted the layers, or said why it cannot.
pub(crate) fn prepared(
    app: &mut App,
    request_id: &PullRequestOpenId,
    layers: Option<Vec<luxe_core::github::NewPullRequest>>,
    refusal: Option<String>,
) {
    if let Some(stack) = current(app, request_id) {
        stack.layers = layers;
        stack.error = refusal;
    }
}

/// The stack opened, all of it or as far as it got: say which, and close when it all did.
pub(crate) fn opened(
    app: &mut App,
    request_id: &PullRequestOpenId,
    numbers: Vec<u64>,
    urls: Vec<String>,
    failed: Option<String>,
) {
    if current(app, request_id).is_none() {
        return;
    }
    let list = numbers.iter().map(|number| format!("#{number}")).collect::<Vec<_>>().join(", ");
    if let Some(message) = failed {
        let opened =
            if numbers.is_empty() { String::new() } else { format!(" Opened so far: {list}.") };
        if let Some(stack) = app.guided_review_workspace.stack_opening.as_mut() {
            stack.opening = false;
            stack.error = Some(format!("{message}{opened}"));
            // What opened is on GitHub; opening again would try it a second time
            stack.layers = None;
        }
        return;
    }
    app.guided_review_workspace.stack_opening = None;
    app.update(Msg::GuidedReviewSetOutcome(ReviewOutcome::Accepted));
    app.push_convo(ConvoBlock::System(format!(
        "[review] Committed the reviewed work chapter by chapter and opened it as a stack of {}: \
         {list}",
        numbers.len()
    )));
    app.status.flash_toast(format!("Stack of {} opened · o opens the top", numbers.len()));
    app.guided_review_workspace.posted_review_url = urls.last().cloned();
}

fn current<'a>(app: &'a mut App, request_id: &PullRequestOpenId) -> Option<&'a mut StackOpening> {
    app.guided_review_workspace
        .stack_opening
        .as_mut()
        .filter(|stack| &stack.request_id == request_id)
}
