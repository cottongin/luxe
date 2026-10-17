//! Guided Review surface state — route-local loading and the reviewer's one position.
//!
//! The pinned review data stays on [`crate::app::App`], while this module owns only the
//! full-screen surface state needed to present it. That keeps an in-flight refresh from
//! replacing the immutable revision still visible to the reviewer.
//!
//! # One selection, no focus
//!
//! The surface is two panes — the guide and the evidence under it — and exactly one of them takes
//! input. `ReviewPosition` *is* the cursor: the keyboard moves it, the detail pane shows whatever
//! it lands on, and the guide pane follows it. There is no focus owner and no second selection,
//! which is why every key means one thing here. A mode switch and a focus switch would be state a
//! key could be routed on wrongly — `Enter` dead in the whole view, `↑`/`↓` never reaching the
//! surface that needed them — and with one cursor neither is expressible.

pub(crate) mod diff;
mod findings;
mod fold;
mod geometry;
mod navigation;
pub(crate) mod preview_guard;
mod progress;
mod source;
mod source_picker;
pub mod state;
mod viewport;

pub use fold::{
    has_mechanical_churn, hunk_row_span, is_mechanical, mechanical_label, visible_groups, Fold,
    FoldedRun, VisibleGroup,
};
pub(crate) use state::*;

use geometry::*;
use luxe_core::config::DiffLayout;
use luxe_core::event::GuidedReviewFailure;
use luxe_core::guided_review::{
    ChapterId, DiffSide, FileId, FindingAnchor, FindingId, FindingSeverity, FindingState,
    GuidedReview, HunkId, ReviewChapter, ReviewFile, ReviewFinding, ReviewHunk, ReviewItemRef,
    ReviewTarget,
};
use std::collections::HashSet;

const COLUMN_SCROLL_STEP: usize = 8;

/// Cells the diff pane spends before either column starts.
///
/// Two of them for the active-hunk bar, which sits outside both columns so the rule between them
/// does not move as the cursor does, and three for the rule. The renderer has its own names for
/// the same two costs and `the_split_gate_measures_what_the_renderer_lays_out` holds them equal —
/// the model may not reach into the view for a constant, and a gate that disagrees with the
/// layout it gates is how a column comes to run off the edge.
pub(crate) const SPLIT_PANE_CHROME: usize = 5;

/// Cells a unified row spends beyond its two line numbers: the active-hunk bar, a space after each
/// number, and the diff sign. Held equal to the renderer's own sum the same way.
pub(crate) const UNIFIED_ROW_CHROME: usize = 5;

/// Cells the diff pane spends on itself: a left pad off the divider, and the scrollbar column.
const DIFF_PANE_CHROME: u16 = 2;

/// Route-local state for one Guided Review presentation.
#[derive(Debug, Clone)]
pub(crate) struct GuidedReviewWorkspace {
    /// Current immutable-source load state.
    pub(crate) load_state: GuidedReviewLoadState,
    /// Whether the guide pane is beside the diff, where the terminal is wide enough for both.
    pub(crate) guide_visible: bool,
    /// Current source-anchored location, retained where IDs survive refresh: the surface's only
    /// cursor, which both panes render and every navigation key moves.
    pub(crate) position: ReviewPosition,
    /// Chapters the reviewer has walked through — the review's only notion of progress.
    pub(crate) walked_chapters: HashSet<ChapterId>,
    /// Width the open modal's field was last drawn at, so `↑/↓` moves by the rows the reader sees
    pub(crate) field_width: std::cell::Cell<u16>,
    /// What the last frame drew; a dialog it did not draw takes only esc
    pub(crate) drawn: std::cell::Cell<preview_guard::Drawn>,
    /// Rows the feedback window's reading pane is scrolled down
    pub(crate) feedback_scroll: u16,
    /// Rows the reading pane can scroll, as last drawn
    pub(crate) feedback_room: std::cell::Cell<u16>,
    /// Whether the narration band (section prose + this change's sentence) is shown above the diff;
    /// on by default, since the prose is the point, and `z` folds it for one who wants every row.
    pub(crate) narration_visible: bool,
    /// What mechanical churn is collapsed right now — the review's only fold.
    pub(crate) fold: Fold,
    /// Whether the local explanation for folded classifications is visible.
    pub(crate) noise_details_visible: bool,
    /// Local source picker state, independent from the pinned immutable review.
    pub(crate) source_picker: ReviewSourcePickerState,
    /// Pending typed reference input for Base or Commit sources.
    pub(crate) source_reference_entry: Option<ReviewSourceReferenceEntry>,
    /// Active in-diff search query (find-in-diff). `None` = not searching; `Some("")` = the search
    /// prompt is open with an empty query.
    pub(crate) search: Option<String>,
    /// Whether the local review shortcut reference is visible.
    pub(crate) help_visible: bool,
    /// Unsaved feedback composer state, captured from the selected diff anchor.
    pub(crate) finding_draft: Option<FindingDraft>,
    /// Existing draft finding being edited by the local composer, if any.
    pub(crate) editing_finding_id: Option<FindingId>,
    /// Whether the local feedback inspector is open over the immutable review.
    pub(crate) findings_visible: bool,
    /// Persisted finding selected within the local inspector.
    pub(crate) selected_finding_id: Option<FindingId>,
    /// Whether the selected finding's read-only evidence is visible.
    pub(crate) finding_details_visible: bool,
    /// Whether the before/after view of the agent's change at this anchor is visible.
    pub(crate) finding_fix_visible: bool,
    /// Selected local finish-review choice, if the decision overlay is open.
    pub(crate) finish_review_choice: Option<FinishReviewChoice>,
    /// Whether accepting despite a blocking finding has received a second confirmation.
    pub(crate) finish_review_accept_confirmed: bool,
    /// Whether the finish chooser offers answering on GitHub: your own pull request's review
    pub(crate) finish_offers_answers: bool,
    /// Whether the finish chooser offers opening the branch's pull request
    pub(crate) finish_offers_opening: bool,
    /// Whether the finish chooser offers opening the review as a stack
    pub(crate) finish_offers_stack: bool,
    /// How this review draws a diff: the configured preference until `Ctrl+B` overrules it.
    pub(crate) layout: DiffLayout,
    /// Number of rows available to the rendered full-screen diff body.
    pub(crate) diff_viewport_rows: usize,
    /// Terminal-cell offset for horizontally clipped source lines.
    pub(crate) diff_column_offset: usize,
    /// Number of terminal cells available to a full-screen diff line.
    pub(crate) diff_viewport_columns: usize,
    /// Terminal width the diff viewport was last measured against, so hiding the guide can
    /// re-measure without waiting for a resize that may never come.
    terminal_columns: u16,
    /// The guide's width when the reader dragged its divider; `None` sizes it to the terminal
    guide_width: Option<u16>,
    /// The surface's own animation frame.
    ///
    /// The status model only advances its spinner while a turn is streaming, and the two waits
    /// that happen *here* — the guide still being written, the agent still addressing comments —
    /// both happen with the app otherwise idle. A review that says it is working has to look like
    /// it, or the reviewer reads a static label as a stuck one.
    pub(crate) frame: usize,
    /// Where the guide pane is scrolled to, or `None` while it follows the cursor.
    ///
    /// The guide normally tracks the review position, which is right while you are walking and
    /// useless when you want to read the section you are *not* standing in. Taking the wheel to it
    /// pins an offset; moving the cursor hands it back, so it never gets stuck somewhere the
    /// reader has left. Interior mutability because the release is decided at draw time, which is
    /// the only place the selected row and the wrapped document exist together.
    pub(crate) guide_scroll: std::cell::Cell<Option<usize>>,
    /// The review position the pinned scroll was taken at, so a moved cursor can release it.
    ///
    /// A fingerprint rather than the position itself, because this is compared at draw time from
    /// an `&App` and a `Cell` needs `Copy`. Keyed on the *position* rather than on the guide row
    /// it selects: two hunks of one file share a guide row, so a row-keyed release would leave the
    /// pane pinned while the reader walked the file it is describing.
    pub(crate) guide_scroll_anchor: std::cell::Cell<u64>,
    /// The offset the guide pane was last drawn at, so the wheel moves on from what is on screen
    /// rather than from the top.
    pub(crate) guide_drawn_offset: std::cell::Cell<usize>,
    /// Furthest the guide could scroll, as last drawn, for a drag on its scrollbar
    pub(crate) guide_room: std::cell::Cell<usize>,
    /// Whether long unified-diff source lines soft-wrap (with a continuation glyph) instead of
    /// being horizontally scrolled. Toggled with `w`; default seeded from config.
    pub(crate) word_wrap: bool,
    /// Pending send, showing exactly what will reach the agent before anything is dispatched.
    pub(crate) send_preview: Option<SendPreview>,
    /// Pending commit, showing the messages before any history is written.
    pub(crate) commit_preview: Option<CommitPreview>,
    /// A pull request's review on its way to GitHub
    pub(crate) github_post: Option<GithubPost>,
    /// Answers on their way to your own pull request's review threads
    pub(crate) thread_answers: Option<ThreadAnswers>,
    /// A branch's pull request, drafted from its review, before it opens
    pub(crate) pull_request_opening: Option<PullRequestOpening>,
    /// Someone else's pull request, about to be checked out to fix
    pub(crate) pull_request_fix: Option<PullRequestFix>,
    /// A branch's review, about to open as a stack
    pub(crate) stack_opening: Option<StackOpening>,
    /// The review last posted to GitHub from here, which `o` opens instead of the pull request
    pub(crate) posted_review_url: Option<String>,
}

impl Default for GuidedReviewWorkspace {
    fn default() -> Self {
        Self {
            load_state: GuidedReviewLoadState::Idle,
            guide_visible: true,
            position: ReviewPosition::default(),
            walked_chapters: HashSet::new(),
            field_width: std::cell::Cell::new(80),
            drawn: std::cell::Cell::new(preview_guard::Drawn::ALL),
            feedback_scroll: 0,
            feedback_room: std::cell::Cell::new(0),
            narration_visible: true,
            fold: Fold::default(),
            noise_details_visible: false,
            source_picker: ReviewSourcePickerState::Closed,
            source_reference_entry: None,
            search: None,
            help_visible: false,
            finding_draft: None,
            editing_finding_id: None,
            findings_visible: false,
            selected_finding_id: None,
            finding_details_visible: false,
            finding_fix_visible: false,
            finish_review_choice: None,
            finish_review_accept_confirmed: false,
            finish_offers_answers: false,
            finish_offers_opening: false,
            finish_offers_stack: false,
            layout: DiffLayout::Auto,
            diff_viewport_rows: 1,
            diff_column_offset: 0,
            diff_viewport_columns: 1,
            terminal_columns: 0,
            guide_width: None,
            frame: 0,
            guide_scroll: std::cell::Cell::new(None),
            guide_scroll_anchor: std::cell::Cell::new(0),
            guide_drawn_offset: std::cell::Cell::new(0),
            guide_room: std::cell::Cell::new(0),
            word_wrap: false,
            send_preview: None,
            commit_preview: None,
            github_post: None,
            thread_answers: None,
            pull_request_opening: None,
            pull_request_fix: None,
            stack_opening: None,
            posted_review_url: None,
        }
    }
}

impl GuidedReviewWorkspace {
    /// Opens the local finish-review chooser without changing durable review state.
    ///
    /// It opens on what the review most likely ends in: fixes when comments are waiting to go,
    /// and accepting when there is nothing to send, where asking for fixes would send nothing.
    pub(crate) fn open_finish_review(&mut self, unsent: bool) {
        self.finish_review_choice = Some(match unsent {
            true => FinishReviewChoice::RequestFixes,
            false => FinishReviewChoice::Accept,
        });
        self.finish_review_accept_confirmed = false;
        self.finish_offers_answers = false;
        self.finish_offers_opening = false;
        self.finish_offers_stack = false;
        self.help_visible = false;
        self.close_feedback_window();
    }

    /// Moves the local finish-review selection without changing durable review state.
    pub(crate) fn move_finish_review_choice(&mut self, forward: bool) {
        if let Some(choice) = self.finish_review_choice {
            self.finish_review_choice = Some(choice.next(
                forward,
                self.finish_offers_answers,
                self.finish_offers_opening,
                self.finish_offers_stack,
            ));
            self.finish_review_accept_confirmed = false;
        }
    }

    /// Closes the local finish-review chooser without changing durable review state.
    pub(crate) fn close_finish_review(&mut self) {
        self.finish_review_choice = None;
        self.finish_review_accept_confirmed = false;
    }

    /// Arms the required second confirmation for accepting with blocking findings.
    pub(crate) fn confirm_finish_review_accept(&mut self) {
        self.finish_review_accept_confirmed = true;
    }

    /// Toggles the local review shortcut reference.
    pub(crate) fn toggle_help(&mut self) {
        self.help_visible = !self.help_visible;
    }

    /// Opens the commit preview so the messages are read before history is written.
    pub(crate) fn open_commit_preview(&mut self) {
        self.commit_preview = Some(CommitPreview { squash: false, running: false });
        self.finish_review_choice = None;
        self.help_visible = false;
    }

    /// Closes the commit preview without writing anything.
    pub(crate) fn close_commit_preview(&mut self) {
        self.commit_preview = None;
    }

    /// Switches between one commit per chapter and a single squashed commit.
    pub(crate) fn toggle_commit_squash(&mut self) {
        if let Some(preview) = self.commit_preview.as_mut() {
            preview.squash = !preview.squash;
        }
    }

    /// Whether a modal is waiting on the engine or GitHub, drawing a spinner as it does.
    ///
    /// These waits happen while the app is otherwise idle, so nothing else earns the frames that
    /// turn the spinner: left out, it froze on one character through a whole pull request opening.
    pub(crate) fn waiting(&self) -> bool {
        let post = self.github_post.as_ref().is_some_and(|post| {
            post.posting
                || post.drafting.is_some()
                || (post.prepared.is_none() && post.error.is_none())
        });
        let answers = self.thread_answers.as_ref().is_some_and(|answers| {
            answers.sending || (answers.prepared.is_none() && answers.error.is_none())
        });
        let opening = self.pull_request_opening.as_ref().is_some_and(|opening| {
            opening.opening || (opening.prepared.is_none() && opening.error.is_none())
        });
        let stack = self.stack_opening.as_ref().is_some_and(|stack| {
            stack.opening || (stack.layers.is_none() && stack.error.is_none())
        });
        let fix = self
            .pull_request_fix
            .as_ref()
            .is_some_and(|fix| fix.branch.is_none() && fix.refusal.is_none());
        post || answers || opening || stack || fix
    }

    /// Advances the surface's own animation frame.
    pub(crate) fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Points the reviewer at the start of the change, on a review they have not begun.
    ///
    /// Called once when a review opens. Someone resuming keeps their place; someone arriving gets
    /// chapter 01 rather than whatever file happened to sort first, which is the difference between
    /// a walkthrough and a file listing.
    pub(crate) fn sync_narrative(&mut self, review: &GuidedReview) {
        if self.walked_chapters.is_empty() {
            if let Some(first) = review.chapters.first() {
                self.position.chapter_id = Some(first.id.clone());
                self.enter_selected_chapter(review);
                return;
            }
        }
        self.sync_chapter_to_selection(review);
    }

    /// Opens or re-collapses every mechanical change, keeping a visible selection when possible.
    pub(crate) fn toggle_noise(&mut self, files: &[ReviewFile]) {
        self.fold.toggle_all();
        self.reconcile_folded_selection(files);
    }

    /// Opens or re-collapses one file's mechanical changes — the collapsed row's own action.
    pub(crate) fn toggle_file_fold(&mut self, files: &[ReviewFile], file_id: &FileId) {
        self.fold.toggle_file(file_id);
        self.reconcile_folded_selection(files);
    }

    pub(crate) fn toggle_narration(&mut self) {
        self.narration_visible = !self.narration_visible;
    }

    /// Toggles the local explanation for why a lower-signal group was folded.
    pub(crate) fn toggle_noise_details(&mut self) {
        self.noise_details_visible = !self.noise_details_visible;
    }

    /// Whether a hunk is collapsed into its file's placeholder row.
    pub(crate) fn is_hunk_hidden(&self, file: &ReviewFile, hunk: &ReviewHunk) -> bool {
        self.fold.hides(file, hunk)
    }

    /// Returns exact collapsed hunk and source-line counts across an immutable review source.
    pub(crate) fn hidden_counts(&self, files: &[ReviewFile]) -> (usize, usize) {
        files
            .iter()
            .flat_map(|file| file.hunks.iter().map(move |hunk| (file, hunk)))
            .filter(|(file, hunk)| self.fold.hides(file, hunk))
            .fold((0, 0), |(hunks, lines), (_, hunk)| (hunks + 1, lines + hunk.lines.len()))
    }
}

/// Whether a file's diff has both an old and a new side worth showing.
///
/// `A` and `D` say it outright; a modification with no deletion (or no addition) at all is the same
/// shape in practice, e.g. a file that only gained lines. Two columns for a one-sided file means
/// half the pane is an empty tinted block, which is worse than unified in every way.
pub(crate) fn has_two_sides(file: &ReviewFile) -> bool {
    use luxe_core::guided_review::{ReviewFileStatus, ReviewLineKind};
    if matches!(
        file.status,
        ReviewFileStatus::Added | ReviewFileStatus::Untracked | ReviewFileStatus::Deleted
    ) {
        return false;
    }
    let mut additions = false;
    let mut deletions = false;
    for line in file.hunks.iter().flat_map(|hunk| &hunk.lines) {
        match line.kind {
            ReviewLineKind::Addition => additions = true,
            ReviewLineKind::Deletion => deletions = true,
            _ => {}
        }
    }
    additions && deletions
}

/// The new file's line a diff row stands for: its own, or for a removal the next line that survived
/// it — where the removed text used to be — and failing that the one before.
pub(crate) fn new_side_line(
    hunk: &luxe_core::guided_review::ReviewHunk,
    index: usize,
) -> Option<u32> {
    let lines = &hunk.lines;
    lines.get(index)?.new_line.or_else(|| {
        lines[index..]
            .iter()
            .find_map(|line| line.new_line)
            .or_else(|| lines[..index].iter().rev().find_map(|line| line.new_line))
            .or((hunk.new_range.start > 0).then_some(hunk.new_range.start))
    })
}

#[cfg(test)]
mod tests;
