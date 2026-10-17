//! Guided Review progress — which chapters the reviewer has walked, and the pending send preview.
//!
//! Progress is deliberately chapter-shaped. Per-hunk reviewed marks made the review an inspection
//! with a completion target ("Reviewed 0/92"), which is a task nobody can finish on an agent-scale
//! change and which makes the correct behavior — read the core sections closely, judge the rest —
//! look like failure. What a resumed review needs is where the reading stopped.

use super::*;

impl GuidedReviewWorkspace {
    /// Marks the current chapter walked once the cursor has nothing further to offer in it.
    pub(super) fn note_chapter_walked(&mut self, review: &GuidedReview) {
        if let Some(chapter_id) = self.position.chapter_id.clone() {
            if self.at_scope_end(review) {
                self.walked_chapters.insert(chapter_id);
            }
        }
    }

    pub(crate) fn is_chapter_walked(&self, chapter: &ChapterId) -> bool {
        self.walked_chapters.contains(chapter)
    }

    /// `(walked, total)` chapters, the review's only progress readout.
    pub(crate) fn chapters_walked(&self, review: &GuidedReview) -> (usize, usize) {
        let walked = review
            .chapters
            .iter()
            .filter(|chapter| self.walked_chapters.contains(&chapter.id))
            .count();
        (walked, review.chapters.len())
    }

    /// Snapshots progress for persistence with the feedback session.
    pub(crate) fn progress_state(
        &self,
        review: &GuidedReview,
    ) -> luxe_core::guided_review::ReviewProgress {
        let mut walked_chapter_ids: Vec<ChapterId> = review
            .chapters
            .iter()
            .filter(|chapter| self.walked_chapters.contains(&chapter.id))
            .map(|chapter| chapter.id.clone())
            .collect();
        walked_chapter_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        luxe_core::guided_review::ReviewProgress {
            walked_chapter_ids,
            // Written only when a review is finished; a session that is still open has not "seen"
            // anything it can subtract from the next one.
            seen_item_ids: Vec::new(),
            seen_revision: None,
        }
    }

    /// Restores which chapters were walked, keeping only chapters the pinned source still has.
    pub(crate) fn restore_progress(
        &mut self,
        progress: &luxe_core::guided_review::ReviewProgress,
        chapters: &[ReviewChapter],
    ) {
        self.walked_chapters = chapters
            .iter()
            .filter(|chapter| progress.walked_chapter_ids.contains(&chapter.id))
            .map(|chapter| chapter.id.clone())
            .collect();
    }

    /// Drops walked marks for chapters a refresh has replaced.
    pub(crate) fn reconcile_walked_chapters(&mut self, chapters: &[ReviewChapter]) {
        self.walked_chapters.retain(|walked| chapters.iter().any(|chapter| &chapter.id == walked));
    }
}

impl GuidedReviewWorkspace {
    /// Opens the send preview. `outcome`/`mode` are set only when the Finish dialog opened it.
    ///
    /// Nothing is dispatched unseen: the reviewer confirms exactly which comments travel, which is
    /// how silent drops went unnoticed before.
    pub(crate) fn open_send_preview(
        &mut self,
        outcome: Option<luxe_core::guided_review::ReviewOutcome>,
        mode: Option<luxe_core::guided_review::ReviewFeedbackMode>,
    ) {
        self.feedback_scroll = 0;
        self.send_preview = Some(SendPreview {
            excluded: std::collections::HashSet::new(),
            selected: 0,
            outcome,
            mode,
        });
        self.help_visible = false;
        self.close_feedback_window();
        self.finish_review_choice = None;
    }

    /// Closes the send preview without dispatching.
    pub(crate) fn close_send_preview(&mut self) {
        self.send_preview = None;
    }

    /// Moves the preview cursor through the candidate comments.
    pub(crate) fn move_send_preview(&mut self, rows: usize, forward: bool) {
        self.feedback_scroll = 0;
        let Some(preview) = self.send_preview.as_mut() else {
            return;
        };
        if rows == 0 {
            preview.selected = 0;
            return;
        }
        preview.selected = if forward {
            (preview.selected + 1) % rows
        } else {
            preview.selected.checked_sub(1).unwrap_or(rows - 1)
        };
    }

    /// Includes or excludes the comment under the preview cursor.
    pub(crate) fn toggle_send_preview_row(&mut self, candidates: &[FindingId]) {
        let Some(preview) = self.send_preview.as_mut() else {
            return;
        };
        let Some(finding_id) = candidates.get(preview.selected) else {
            return;
        };
        if !preview.excluded.insert(finding_id.clone()) {
            preview.excluded.remove(finding_id);
        }
    }
}
