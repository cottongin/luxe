//! Review commands — opening, refreshing and steering the guided review from the command layer.

use super::*;

const REVIEW_TARGET_ARGUMENT_LIMIT: usize = 256;
pub(super) const REVIEW_TARGET_USAGE: &str =
    "Usage: /diff [refresh|turn|base <ref>|commit <oid>|pr <number or URL>]";
pub(super) const LAST_TURN_UNAVAILABLE: &str =
    "No complete baseline is available for the latest turn.";

pub(super) fn dispatch_review_navigation_action(app: &mut App, action: UiAction) -> bool {
    if app.current_view != crate::app::ViewMode::GuidedReview {
        return false;
    }

    let message = match action {
        UiAction::ReviewChapterNext => crate::app::Msg::GuidedReviewChapterNext,
        UiAction::ReviewChapterPrev => crate::app::Msg::GuidedReviewChapterPrevious,
        UiAction::ReviewFileNext => crate::app::Msg::GuidedReviewFileNext,
        UiAction::ReviewFilePrev => crate::app::Msg::GuidedReviewFilePrevious,
        UiAction::ReviewHunkNext => crate::app::Msg::GuidedReviewHunkNext,
        UiAction::ReviewHunkPrev => crate::app::Msg::GuidedReviewHunkPrevious,
        UiAction::ReviewOpenSelection => crate::app::Msg::GuidedReviewOpenSelection,
        UiAction::ReviewLineNext => crate::app::Msg::GuidedReviewLineNext,
        UiAction::ReviewLinePrev => crate::app::Msg::GuidedReviewLinePrevious,
        UiAction::ReviewPageNext => crate::app::Msg::GuidedReviewPageNext,
        UiAction::ReviewPagePrev => crate::app::Msg::GuidedReviewPagePrevious,
        UiAction::ReviewScrollLeft => crate::app::Msg::GuidedReviewColumnPrevious,
        UiAction::ReviewScrollRight => crate::app::Msg::GuidedReviewColumnNext,
        UiAction::ReviewLineStart => crate::app::Msg::GuidedReviewLineStart,
        UiAction::ReviewToggleGuide => crate::app::Msg::GuidedReviewToggleGuide,
        UiAction::ReviewAddFinding => crate::app::Msg::GuidedReviewBeginFinding,
        UiAction::ReviewAskQuestion => crate::app::Msg::GuidedReviewBeginQuestion,
        UiAction::ReviewOpenInBrowser => crate::app::Msg::GuidedReviewOpenInBrowser,
        UiAction::ReviewSend => crate::app::Msg::GuidedReviewOpenSendPreview,
        UiAction::ReviewEditFinding => crate::app::Msg::GuidedReviewEditSelectedFinding,
        UiAction::ReviewDeleteFinding => crate::app::Msg::GuidedReviewDeleteSelectedFindingDraft,
        UiAction::ReviewReanchorFinding => crate::app::Msg::GuidedReviewReanchorSelectedFinding,
        UiAction::ReviewFindings => crate::app::Msg::GuidedReviewToggleFindings,
        UiAction::ReviewResolveFinding => {
            crate::app::Msg::GuidedReviewToggleSelectedFindingResolved
        }
        UiAction::ReviewToggleNoise => crate::app::Msg::GuidedReviewToggleNoise,
        UiAction::ReviewToggleSplit => crate::app::Msg::GuidedReviewToggleSplit,
        UiAction::ReviewToggleWordWrap => crate::app::Msg::GuidedReviewToggleWordWrap,
        UiAction::ReviewSource => crate::app::Msg::GuidedReviewToggleSourcePicker,
        UiAction::ReviewHelp => crate::app::Msg::GuidedReviewToggleHelp,
        _ => return false,
    };
    app.update(message);
    true
}

/// Opens the Changes pane on a sensible file, or puts it away.
///
/// "Sensible" is the first row the dock draws, which is the file with the most change in the
/// directory with the most change — the same thing the eye lands on in the rail, so the keyboard
/// and the mouse open the same file. A project with nothing changed says so and opens nothing:
/// an empty pane over the transcript would be a surface with no content and no way to guess why.
pub(in crate::app) fn toggle_changes(app: &mut App) {
    if app.changes.is_some() {
        app.update(crate::app::Msg::CloseChanges);
        return;
    }
    match crate::app::messages::changes::changed_files(app).first() {
        Some(file) => app.update(crate::app::Msg::OpenChanges(file.path.clone())),
        None => app.status.flash_toast("Nothing has changed in this project yet".to_string()),
    }
}

/// What the review key means right now.
///
/// The working tree, until it is empty and this session put something into a commit. A commit
/// moves `HEAD` and takes the diff against it to zero, so the key would otherwise open a review of
/// nothing at the exact moment there is most to read — while the turn's own retained baseline,
/// which is content on disk rather than a Git range, still holds the whole change.
fn default_review_target(app: &App) -> ReviewTarget {
    let clean = app.workspace.as_ref().is_some_and(|state| state.changes.total_files == 0);
    match app.review.last_turn.as_ref() {
        Some((session_id, turn_id)) if clean && app.worktree_emptied => {
            ReviewTarget::LastTurn { session_id: session_id.clone(), turn_id: turn_id.clone() }
        }
        _ => ReviewTarget::WorkingTree,
    }
}

/// What the review key means: show me the review, creating one first if there is not one yet.
///
/// Distinct from [`start_guided_review`], which always captures — that is what the source picker
/// and `/diff <target>` want, because they are naming a *different* thing to review. This is the
/// bare verb, and its answer is usually the review already pinned.
pub(in crate::app) fn open_or_start_guided_review(app: &mut App) {
    // Pinned, complete, and labeled if the tree has moved under it. Someone coming back to the
    // review they just left should not wait for a re-capture to get there.
    if app.guided_review.is_some() && app.review.queued.is_none() {
        app.current_view = crate::app::ViewMode::GuidedReview;
        app.dock.scroll = 0;
        return;
    }
    start_guided_review(app, default_review_target(app));
}

/// Ends the session to remove its worktree when nothing is uncommitted, or says what is.
pub(in crate::app) fn worktree_removal_checked(app: &mut App, uncommitted: Vec<String>) {
    let Some(root) = app.worktree_removal_check.take() else {
        return;
    };
    if uncommitted.is_empty() {
        app.outbox.worktree_removal = Some(root);
        return;
    }
    let shown: Vec<&str> = uncommitted.iter().take(5).map(String::as_str).collect();
    let more = uncommitted.len().saturating_sub(shown.len());
    let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
    app.push_convo(ConvoBlock::System(format!(
        "The worktree has work that is not committed: {}{tail}. Commit it, or discard it, and \
         `/worktree remove` again.",
        shown.join(", ")
    )));
}

/// Opens a review, or parks the request until the turn that is running finishes.
///
/// The engine will not capture mid-turn — `turn.rs` re-queues anything it cannot act on — so
/// switching to the review surface here left the reader on a full-screen "Capturing the working
/// tree" that had not started, blind to the agent still producing the very change they wanted to
/// read. A review is a pinned immutable revision, and there is no honest one to pin while the
/// change is still being written.
pub(in crate::app) fn start_guided_review(app: &mut App, target: ReviewTarget) {
    use crate::app::types::{QueuedReview, QueuedReviewState};
    // Already captured while the reader stayed in the conversation: this press is them taking the
    // tray row up on its offer.
    if app.review.queued.as_ref().is_some_and(|queued| queued.state == QueuedReviewState::Ready) {
        app.review.queued = None;
        app.current_view = crate::app::ViewMode::GuidedReview;
        app.dock.scroll = 0;
        return;
    }
    if app.turn_in_flight() {
        app.review.queued = Some(QueuedReview { target, state: QueuedReviewState::Waiting });
        return;
    }
    // Idle, so the capture can start at once — but the surface still does not open until there is
    // a whole review to open. Arriving on a half-built one and watching the prose rewrite itself
    // under you is the same bad experience whether the wait was five seconds or a turn long.
    app.review.queued =
        Some(QueuedReview { target: target.clone(), state: QueuedReviewState::Capturing });
    app.status.animating = true;
    app.guided_review_workspace.begin_loading(target.clone());
    dispatch_guided_review(app, target);
}

/// Starts the capture the tray is waiting on, without taking the screen.
///
/// The reader has been in the conversation since they asked; pulling them into a full-screen review
/// the moment a background job finishes would take the display away from whatever they moved on to.
/// The tray says when it is ready and `Ctrl+G` opens it.
pub(in crate::app) fn flush_queued_review(app: &mut App) {
    use crate::app::QueuedReviewState;
    let Some(queued) = app.review.queued.as_mut() else { return };
    if queued.state != QueuedReviewState::Waiting {
        return;
    }
    queued.state = QueuedReviewState::Capturing;
    app.status.animating = true;
    let target = queued.target.clone();
    app.guided_review_workspace.begin_loading(target.clone());
    dispatch_guided_review(app, target);
}

/// Starts a typed review source load without deriving source state in the UI.
fn dispatch_guided_review(app: &mut App, target: ReviewTarget) {
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    if !app.send_action(
        Action::LoadGuidedReview { request_id, target: target.clone() },
        "load a review",
    ) {
        app.active_guided_review_request = None;
        app.guided_review_workspace.mark_failed(
            target,
            GuidedReviewFailure {
                kind: GuidedReviewFailureKind::Capture,
                message: "Could not start the review.".into(),
            },
        );
        app.review.queued = None;
        app.push_convo(ConvoBlock::System("Could not start the review.".to_string()));
    }
}

/// Refreshes the currently pinned immutable review with an optimistic revision guard.
pub(in crate::app) fn refresh_guided_review(app: &mut App) {
    let Some(review) = app.guided_review.as_ref() else {
        app.push_convo(ConvoBlock::System("No review is available to refresh.".to_string()));
        return;
    };
    let (review_id, expected_revision, target) =
        (review.id.clone(), review.revision_id.clone(), review.target.clone());
    crate::app::navigation::enter_guided_review(app, target.clone());
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    if !app.send_action(
        Action::RefreshGuidedReview { request_id, review_id, expected_revision },
        "refresh the review",
    ) {
        app.active_guided_review_request = None;
        app.guided_review_workspace.mark_failed(
            target,
            GuidedReviewFailure {
                kind: GuidedReviewFailureKind::Capture,
                message: "Could not refresh the review.".into(),
            },
        );
        app.push_convo(ConvoBlock::System("Could not refresh the review.".to_string()));
    }
}

pub(in crate::app) fn parse_review_target(
    arguments: Option<&str>,
    last_turn_target: Option<&(String, String)>,
) -> Result<ReviewTarget, &'static str> {
    let Some(arguments) = arguments.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ReviewTarget::WorkingTree);
    };
    if arguments.len() > REVIEW_TARGET_ARGUMENT_LIMIT || arguments.chars().any(char::is_control) {
        return Err(REVIEW_TARGET_USAGE);
    }
    if arguments == "turn" {
        return last_turn_target
            .map(|(session_id, turn_id)| ReviewTarget::LastTurn {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
            })
            .ok_or(LAST_TURN_UNAVAILABLE);
    }
    if arguments.split_whitespace().count() != 2 {
        return Err(REVIEW_TARGET_USAGE);
    }

    let mut parts = arguments.split_whitespace();
    let kind = parts.next().ok_or(REVIEW_TARGET_USAGE)?;
    let value = parts.next().ok_or(REVIEW_TARGET_USAGE)?;
    match kind {
        "base" => Ok(ReviewTarget::Base { reference: value.to_string() }),
        "commit" => Ok(ReviewTarget::Commit { oid: value.to_string() }),
        "pr" => parse_pull_request(value).ok_or(REVIEW_TARGET_USAGE),
        _ => Err(REVIEW_TARGET_USAGE),
    }
}

/// A pull request as a reader would name one: `123`, `#123`, or its web address.
///
/// A URL keeps its `owner/name`, so the engine can tell the checkout's own pull request from an
/// upstream's; any host is accepted, since GitHub Enterprise lives on its own. Trailing pieces of a
/// URL copied from a browser (`/files`, a query, a fragment) are ignored.
fn parse_pull_request(value: &str) -> Option<ReviewTarget> {
    let number = |text: &str| text.parse::<u64>().ok().filter(|number| *number > 0);
    if let Some(number) = number(value.strip_prefix('#').unwrap_or(value)) {
        return Some(ReviewTarget::PullRequest { number, repository: None });
    }
    let address = value.split_once("://").map_or(value, |(_, rest)| rest);
    let address = address.split(['?', '#']).next()?;
    let segments: Vec<&str> = address.split('/').filter(|segment| !segment.is_empty()).collect();
    match segments.as_slice() {
        [_host, owner, name, "pull", pr, ..] => Some(ReviewTarget::PullRequest {
            number: number(pr)?,
            repository: Some(format!("{owner}/{name}")),
        }),
        _ => None,
    }
}
