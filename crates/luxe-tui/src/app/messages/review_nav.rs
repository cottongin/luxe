//! Guided Review navigation — moving through a review and changing how it is shown.
//!
//! The cursor, search, view toggles, the source picker and the findings window. None of it edits
//! the feedback session; that is `review`'s. [`reduce`] takes the cursor's messages and hands the
//! rest down one table at a time, so each stays short.

use crate::app::{App, DescriptionStep, Msg};
use crate::guided_review::GuidedReviewWorkspace;
use luxe_core::guided_review::{GuidedReview, ReviewFinding};

/// Document rows one wheel notch moves the guide pane.
///
/// Three rather than one: the guide is prose, and a notch that moves a single line makes reading a
/// long chapter a scroll marathon.
const GUIDE_WHEEL_ROWS: usize = 3;

impl App {
    /// Runs `f` with the review workspace and the open review, or does nothing without a review.
    ///
    /// They are disjoint fields, so the workspace can be changed against the review it shows
    /// without cloning the review (every file and hunk of it) to get past the borrow. Whatever `f`
    /// returns is dropped; a caller that needs an answer captures it.
    pub(in crate::app) fn with_review<R>(
        &mut self,
        f: impl FnOnce(&mut GuidedReviewWorkspace, &GuidedReview) -> R,
    ) {
        if let Some(review) = self.guided_review.as_deref() {
            f(&mut self.guided_review_workspace, review);
        }
    }

    /// The finding the feedback window's cursor is on, while the session still has it.
    pub(in crate::app) fn selected_finding(&self) -> Option<&ReviewFinding> {
        let selected = self.guided_review_workspace.selected_finding_id.as_ref()?;
        self.review.session()?.findings.iter().find(|finding| &finding.id == selected)
    }
}

pub(super) fn reduce(app: &mut App, msg: Msg) {
    match msg {
        Msg::GuidedReviewSelectDiffRow(row) => {
            app.with_review(|workspace, review| workspace.select_diff_row(review, row));
        }
        Msg::GuidedReviewSelectFile(file_id) => {
            app.with_review(|workspace, review| workspace.select_file_by_id(review, &file_id));
        }
        Msg::GuidedReviewToggleFileFold(file_id) => app.with_review(|workspace, review| {
            workspace.toggle_file_fold(&review.files, &file_id);
            workspace.sync_hunk_to_cursor(&review.files);
        }),
        // One scroll model: side-by-side is a projection of the unified rows, so line and page
        // movement need no special case.
        Msg::GuidedReviewLineNext => {
            app.with_review(|workspace, review| workspace.next_line(review))
        }
        Msg::GuidedReviewLinePrevious => {
            app.with_review(|workspace, review| workspace.previous_line(review));
        }
        Msg::GuidedReviewPageNext => {
            app.with_review(|workspace, review| workspace.next_page(review))
        }
        Msg::GuidedReviewHalfPage(forward) => {
            app.with_review(|workspace, review| workspace.half_page(review, forward));
        }
        Msg::GuidedReviewPagePrevious => {
            app.with_review(|workspace, review| workspace.previous_page(review));
        }
        Msg::GuidedReviewHunkNext => next_hunk(app),
        Msg::GuidedReviewHunkPrevious => {
            app.with_review(|workspace, review| workspace.previous_hunk(review));
            persist_review_progress(app);
        }
        Msg::GuidedReviewChapterNext => {
            app.with_review(|workspace, review| workspace.next_chapter(review));
        }
        Msg::GuidedReviewChapterPrevious => {
            app.with_review(|workspace, review| workspace.previous_chapter(review));
        }
        Msg::GuidedReviewFileNext => {
            app.with_review(|workspace, review| workspace.next_chapter_file(review));
        }
        Msg::GuidedReviewFilePrevious => {
            app.with_review(|workspace, review| workspace.previous_chapter_file(review));
        }
        Msg::GuidedReviewLineStart => app.guided_review_workspace.reset_column_offset(),
        Msg::GuidedReviewColumnNext => {
            app.with_review(|workspace, review| workspace.next_column(&review.files));
        }
        Msg::GuidedReviewColumnPrevious => app.guided_review_workspace.previous_column(),
        Msg::GuidedReviewOpenSelection => open_review_selection(app),
        Msg::GuidedReviewOpenChapter(chapter_id) => app.with_review(|workspace, review| {
            workspace.open_guide_target(review, None, None, Some(&chapter_id));
        }),
        Msg::GuidedReviewScrollGuide { up } => scroll_guide(app, up),
        other => display(app, other),
    }
}

/// Search and the view toggles; the rest goes on to [`choose`].
fn display(app: &mut App, msg: Msg) {
    let has_description = review_has_description(app);
    let workspace = &mut app.guided_review_workspace;
    match msg {
        Msg::GuidedReviewBeginSearch => workspace.begin_search(),
        Msg::GuidedReviewSearchInput(character) => {
            workspace.search_input(character);
            app.with_review(|workspace, review| workspace.jump_search(review, true));
        }
        Msg::GuidedReviewSearchBackspace => workspace.search_backspace(),
        Msg::GuidedReviewCancelSearch => workspace.cancel_search(),
        Msg::GuidedReviewSearchNext => {
            app.with_review(|workspace, review| workspace.jump_search(review, true));
        }
        Msg::GuidedReviewSearchPrevious => {
            app.with_review(|workspace, review| workspace.jump_search(review, false));
        }
        Msg::GuidedReviewToggleGuide => workspace.toggle_guide(),
        Msg::GuidedReviewToggleNoise => {
            app.with_review(|workspace, review| workspace.toggle_noise(&review.files));
        }
        Msg::GuidedReviewToggleNarration => workspace.toggle_narration(),
        Msg::GuidedReviewToggleNoiseDetails => workspace.toggle_noise_details(),
        Msg::GuidedReviewToggleSplit => app.with_review(|workspace, review| {
            workspace.toggle_split_preference(&review.files);
            workspace.clamp_column_offset(&review.files);
        }),
        Msg::GuidedReviewToggleWordWrap => workspace.toggle_word_wrap(),
        Msg::GuidedReviewToggleHelp => workspace.toggle_help(),
        // Only a pull request has a description to read; anywhere else the key does nothing
        Msg::GuidedReviewDescription(DescriptionStep::Toggle) if has_description => {
            workspace.toggle_description();
        }
        Msg::GuidedReviewDescription(DescriptionStep::Toggle) => {}
        Msg::GuidedReviewDescription(DescriptionStep::Scroll(rows)) => {
            workspace.scroll_description(rows);
        }
        other => choose(app, other),
    }
}

/// Whether the review on screen is a pull request's with a description to open.
fn review_has_description(app: &App) -> bool {
    let pull_request =
        app.guided_review.as_ref().and_then(|review| review.context.pull_request.as_ref());
    pull_request.is_some_and(|pr| !pr.body.trim().is_empty())
}

/// The source picker and the findings window.
fn choose(app: &mut App, msg: Msg) {
    let last_turn_available = app.review.last_turn.is_some();
    let workspace = &mut app.guided_review_workspace;
    match msg {
        Msg::GuidedReviewToggleSourcePicker => workspace.toggle_source_picker(),
        Msg::GuidedReviewSourcePickerNext => {
            workspace.move_source_picker_selection(true, last_turn_available);
        }
        Msg::GuidedReviewSourcePickerPrevious => {
            workspace.move_source_picker_selection(false, last_turn_available);
        }
        Msg::GuidedReviewSelectSourcePicker => super::review::select_source(app),
        Msg::GuidedReviewSourceReferenceInsert(character) => {
            workspace.push_source_reference_char(character);
        }
        Msg::GuidedReviewSourceReferenceBackspace => workspace.pop_source_reference_char(),
        Msg::GuidedReviewSubmitSourceReference => {
            if let Some(target) = workspace.submit_source_reference() {
                crate::app::commands::start_guided_review(app, target);
            }
        }
        Msg::GuidedReviewCancelSourceReference => workspace.cancel_source_reference_entry(),
        Msg::GuidedReviewToggleFindings => toggle_findings(app),
        Msg::GuidedReviewSelectFinding(finding_id) => {
            // Only honor a click that lands on a finding that still exists in the session.
            let exists = app
                .review
                .session()
                .is_some_and(|session| session.findings.iter().any(|f| f.id == finding_id));
            if exists {
                app.guided_review_workspace.selected_finding_id = Some(finding_id);
            }
        }
        // `messages` routes a variant here only by listing it, and this match cannot be exhaustive
        other => tracing::warn!(?other, "a review navigation message with no arm"),
    }
}

fn next_hunk(app: &mut App) {
    let mut finished = false;
    app.with_review(|workspace, review| {
        let before = workspace.position.clone();
        workspace.next_hunk(review);
        // "Next" past the review's last stop is "what now": the finish dialog
        let last = review.chapters.last().map(|chapter| &chapter.id);
        finished = workspace.position == before
            && last.is_some()
            && workspace.position.chapter_id.as_ref() == last
            && workspace.at_scope_end(review);
    });
    // Reaching the end of a section is the progress worth remembering.
    persist_review_progress(app);
    if finished {
        app.dispatch_action(luxe_core::keymap::UiAction::ReviewFinish);
    }
}

fn scroll_guide(app: &App, up: bool) {
    let workspace = &app.guided_review_workspace;
    // The first notch moves on from what is on screen, wherever following had put it;
    // the draw clamps whatever lands here
    let current =
        workspace.guide_scroll.get().unwrap_or_else(|| workspace.guide_drawn_offset.get());
    let next = match up {
        true => current.saturating_sub(GUIDE_WHEEL_ROWS),
        false => current.saturating_add(GUIDE_WHEEL_ROWS),
    };
    workspace.guide_scroll.set(Some(next));
}

fn toggle_findings(app: &mut App) {
    app.guided_review_workspace.toggle_findings();
    // The window draws a cursor, so there has to be a selection for its keys to act on
    let findings = app.review.session().map(|session| &session.findings);
    let workspace = &mut app.guided_review_workspace;
    if let (true, Some(findings)) = (workspace.findings_visible, findings) {
        let ordered = crate::guided_review::ordered_findings(findings);
        let selected = workspace.selected_finding_id.as_ref();
        if !ordered.iter().any(|finding| Some(&finding.id) == selected) {
            workspace.selected_finding_id = ordered.first().map(|finding| finding.id.clone());
            workspace.feedback_scroll = 0;
        }
    }
}

/// Records which chapters have been walked with the feedback session.
///
/// Progress is stored with the feedback session, so quitting mid-review keeps it with the comments.
/// Sent only when it differs from what the desk stored last, so stepping through a chapter does not
/// thrash the disk; the desk checks again, since a step can race the save of the one before it.
pub(in crate::app) fn persist_review_progress(app: &mut App) {
    let Some(review) = app.guided_review.as_ref() else {
        return;
    };
    let progress = app.guided_review_workspace.progress_state(review);
    let stored = app.review.session().filter(|session| session.id == review.id);
    if stored.is_none_or(|session| session.progress == progress) {
        return;
    }
    let action =
        luxe_core::engine::Action::RecordReviewProgress { review_id: review.id.clone(), progress };
    // the next step sends everything walked by then
    app.send_quietly(action, "record review progress");
}

/// Opens whatever the cursor is on that has more to show.
///
/// There is no separate selection to "open" any more — the position *is* the cursor and the detail
/// pane already renders it — so this is only for the two things that are genuinely collapsed: a
/// run of folded churn, and a comment shown as a gutter label.
fn open_review_selection(app: &mut App) {
    if cursor_is_on_a_collapsed_run(app) {
        open_selected_file_fold(app);
    } else if app.guided_review_workspace.selected_finding_id.is_some() {
        app.guided_review_workspace.toggle_finding_details();
    }
}

/// Whether the diff cursor sits on a collapsed run rather than a source row.
fn cursor_is_on_a_collapsed_run(app: &App) -> bool {
    app.guided_review_workspace.position.hunk_id.is_none()
        && app
            .guided_review
            .as_ref()
            .and_then(|review| {
                let file_id = app.guided_review_workspace.position.file_id.as_ref()?;
                review.files.iter().find(|file| &file.id == file_id)
            })
            .is_some_and(|file| {
                file.is_text()
                    && file
                        .hunks
                        .iter()
                        .any(|hunk| app.guided_review_workspace.is_hunk_hidden(file, hunk))
            })
}

/// Opens (or re-collapses) the mechanical changes in the file the cursor is reading.
fn open_selected_file_fold(app: &mut App) {
    let Some(file_id) = app.guided_review_workspace.position.file_id.clone() else {
        return;
    };
    app.with_review(|workspace, review| {
        workspace.toggle_file_fold(&review.files, &file_id);
        workspace.sync_hunk_to_cursor(&review.files);
    });
}
