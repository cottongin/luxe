//! Preview guard — a review dialog the reader cannot see takes only esc.
//!
//! A confirm the reader cannot see is not one, so what the last frame drew is recorded by the view
//! and read by the keys. Every frame that draws the review settles it: below the surface's minimum
//! nothing is drawn and every open dialog waits, and above it a preview drawn or a notice standing
//! in for one says which. It guards open dialogs only, and the review's keys are read only while
//! it is the view, so nothing is held back outside it.

use super::GuidedReviewWorkspace;
use crossterm::event::{KeyCode, KeyEvent};

/// What the last frame of the review drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Drawn {
    /// The open preview, or there was none to draw
    pub(crate) preview: bool,
    /// The surface at all, rather than its minimum-size notice
    pub(crate) surface: bool,
}

impl Drawn {
    /// Everything, as before a first frame.
    pub(crate) const ALL: Self = Self { preview: true, surface: true };
}

impl GuidedReviewWorkspace {
    /// Whether a dialog that fits itself to the screen is open: the finish dialog, a finding's
    /// details, or a confirming preview — stack, worktree fix, thread answers or the post.
    fn previewing(&self) -> bool {
        self.finish_review_choice.is_some()
            || (self.findings_visible && self.finding_details_visible)
            || self.stack_opening.is_some()
            || self.pull_request_fix.is_some()
            || self.thread_answers.is_some()
            || self.github_post.is_some()
    }

    /// Whether anything that acts on a key is open over the review: a dialog, a composer, the
    /// feedback window (whose `x` deletes), or the source picker and its reference prompt.
    fn dialog_open(&self) -> bool {
        self.previewing()
            || self.send_preview.is_some()
            || self.commit_preview.is_some()
            || self.pull_request_opening.is_some()
            || self.finding_draft.is_some()
            || self.findings_visible
            || self.selected_source_picker().is_some()
            || self.source_reference_entry.is_some()
    }

    /// Starts a frame: whether the surface fits at all, and no preview drawn yet. Returns `fits`.
    pub(crate) fn settle_surface(&self, fits: bool) -> bool {
        self.drawn.set(Drawn { preview: !self.previewing(), surface: fits });
        fits
    }

    /// Records whether the open preview drew this frame.
    pub(crate) fn settle_preview(&self, drawn: bool) {
        let surface = self.drawn.get().surface;
        self.drawn.set(Drawn { preview: drawn || !self.previewing(), surface });
    }

    /// Whether `key` is swallowed because the dialog it would reach was not drawn: every open
    /// dialog below the surface's minimum, or a preview behind its notice. A summary composer
    /// opened over the post is drawn on its own, so it keeps its keys while the surface is drawn.
    pub(crate) fn hidden_preview_takes(&self, key: KeyEvent) -> bool {
        let drawn = self.drawn.get();
        let composing = self.github_post.is_some() && self.finding_draft.is_some();
        let undrawn = !drawn.surface && self.dialog_open();
        let hidden = undrawn || (!drawn.preview && self.previewing() && !composing);
        hidden && key.code != KeyCode::Esc
    }
}
