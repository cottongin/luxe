//! Review scrolling — the diff's rows and columns, and the hunk the cursor lands in.

use super::*;

impl GuidedReviewWorkspace {
    /// Returns horizontally clipped diff source lines to their first terminal cell.
    pub(crate) fn reset_column_offset(&mut self) {
        self.diff_column_offset = 0;
    }

    /// Moves the selected file's source right by one bounded terminal-cell step, in whichever
    /// layout it is drawn.
    pub(crate) fn next_column(&mut self, files: &[ReviewFile]) {
        let maximum = self.selected_file(files).map_or(0, |file| self.column_maximum(file));
        self.diff_column_offset =
            self.diff_column_offset.saturating_add(COLUMN_SCROLL_STEP).min(maximum);
    }

    /// Clamps horizontal position after a resize or presentation change.
    pub(crate) fn clamp_column_offset(&mut self, files: &[ReviewFile]) {
        let maximum = self.selected_file(files).map_or(0, |file| self.column_maximum(file));
        self.diff_column_offset = self.diff_column_offset.min(maximum);
    }

    /// Moves the selected diff source left by one terminal-cell step without underflow.
    pub(crate) fn previous_column(&mut self) {
        self.diff_column_offset = self.diff_column_offset.saturating_sub(COLUMN_SCROLL_STEP);
    }

    /// Puts the cursor on the row `pick` chooses out of the furthest it can go, the way a drag on
    /// the diff's scrollbar does.
    pub(crate) fn scroll_to_row(
        &mut self,
        review: &GuidedReview,
        pick: impl FnOnce(usize) -> usize,
    ) {
        let files = &review.files;
        let max_offset =
            selected_file_row_count(&self.position, files, &self.fold).saturating_sub(1);
        self.position.line_offset = pick(max_offset).min(max_offset);
        self.sync_hunk_to_offset(files);
    }

    /// Scrolls the diff within the selected file, stopping at either end.
    ///
    /// Never rolls into the neighboring file: the pane draws one file at a time, so one row too
    /// many would silently move you to a diff you did not mean to be in. Moving between files is
    /// what the explicit keys are for (`Space`/`n`, `[`/`]`, `Ctrl+↑/↓`), which keep the narration
    /// and the reviewed marks in step; scrolling is only ever a look around.
    pub(in crate::guided_review) fn scroll_lines(
        &mut self,
        review: &GuidedReview,
        amount: usize,
        forward: bool,
    ) {
        let files = &review.files;
        let max_offset =
            selected_file_row_count(&self.position, files, &self.fold).saturating_sub(1);
        self.position.line_offset = if forward {
            self.position.line_offset.saturating_add(amount).min(max_offset)
        } else {
            self.position.line_offset.saturating_sub(amount)
        };
        self.sync_hunk_to_offset(files);
    }

    /// Re-points the selection at whatever the cursor row now sits inside, after the row space
    /// changed underneath it (a fold opening or closing).
    pub(crate) fn sync_hunk_to_cursor(&mut self, files: &[ReviewFile]) {
        let rows = selected_file_row_count(&self.position, files, &self.fold);
        self.position.line_offset = self.position.line_offset.min(rows.saturating_sub(1));
        self.sync_hunk_to_offset(files);
    }

    /// Keeps the selected hunk pointing at whatever the cursor row now sits inside, so the
    /// breadcrumb and reviewed marker follow a continuous scroll.
    pub(super) fn sync_hunk_to_offset(&mut self, files: &[ReviewFile]) {
        let Some(file) =
            self.position.file_id.as_ref().and_then(|id| files.iter().find(|file| &file.id == id))
        else {
            return;
        };
        let mut row = 0usize;
        for group in visible_groups(file, &self.fold) {
            let span = group.rows();
            if self.position.line_offset < row + span {
                // A collapsed run belongs to no hunk, so the breadcrumb drops to the file.
                let landed = match group {
                    VisibleGroup::Hunk(hunk) => Some(hunk.id.clone()),
                    VisibleGroup::Folded(_) => None,
                };
                if self.position.hunk_id != landed {
                    self.position.hunk_id = landed;
                }
                return;
            }
            row += span;
        }
    }
}
