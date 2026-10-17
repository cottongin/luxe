//! Review viewport — the terminal's size, the guide's width, the diff's layout, wrapping and
//! scrolling by lines and pages.

use super::*;

impl GuidedReviewWorkspace {
    /// Toggles soft word-wrap for long unified-diff source lines. Wrapping supersedes horizontal
    /// scrolling, so the column offset is reset when it turns on.
    pub(crate) fn toggle_word_wrap(&mut self) {
        self.word_wrap = !self.word_wrap;
        if self.word_wrap {
            self.diff_column_offset = 0;
        }
    }

    /// Flips the diff between one column and two for this session.
    ///
    /// Records the opposite of what is *showing* rather than cycling the setting, so the first
    /// press always changes something: moving `auto` to `split` on a pane already wide enough to
    /// pair the sides would be a key that does nothing.
    pub(crate) fn toggle_split_preference(&mut self, files: &[ReviewFile]) {
        self.layout = match self.split_layout_showing(files) {
            true => DiffLayout::Unified,
            // `split` rather than `auto`, because a reader who reached for the key on a pane
            // `auto` had already declined wants the pairing, not the rule that refused it.
            false => DiffLayout::Split,
        };
    }

    /// Cells of code one column would get for a file whose line numbers need `gutter` of them.
    ///
    /// The quantity [`DiffLayout::splits`] is written in, so the review reaches the same verdict a
    /// tool card would on the same content — it just has different chrome to pay for first.
    fn split_column_cells(&self, gutter: usize) -> usize {
        (self.diff_viewport_columns.saturating_sub(SPLIT_PANE_CHROME) / 2)
            .saturating_sub(gutter + 2)
    }

    /// The farthest right the diff may scroll on `file`: until its widest line's last cell shows.
    ///
    /// Measured over the whole file, because the pane draws the whole file at one offset. Bounding
    /// it by the hunk under the cursor left every longer line in the other hunks out of reach.
    pub(crate) fn column_maximum(&self, file: &ReviewFile) -> usize {
        let gutter = diff::gutter_width(&file.hunks);
        let room = if self.split_layout_active_for(file) {
            self.split_column_cells(gutter)
        } else {
            self.diff_viewport_columns.saturating_sub(gutter * 2 + UNIFIED_ROW_CHROME)
        };
        file_source_width(file).saturating_sub(room)
    }

    /// Whether *this file* renders side by side.
    ///
    /// Side-by-side is a *view* of the unified row space rather than a mode with its own scroll
    /// position, so this is the only state the renderer needs.
    pub(crate) fn split_layout_active_for(&self, file: &ReviewFile) -> bool {
        self.layout
            .splits(self.split_column_cells(diff::gutter_width(&file.hunks)), has_two_sides(file))
    }

    /// Whether the file the reviewer is on is drawn side by side.
    ///
    /// For the callers that hold the file list rather than the file: horizontal scroll bounds, and
    /// the toggle deciding what it is flipping away from.
    pub(crate) fn split_layout_showing(&self, files: &[ReviewFile]) -> bool {
        self.selected_file(files).is_some_and(|file| self.split_layout_active_for(file))
    }

    /// The file the reviewer is on, when the position names one the review still has.
    pub(super) fn selected_file<'a>(&self, files: &'a [ReviewFile]) -> Option<&'a ReviewFile> {
        let id = self.position.file_id.as_ref()?;
        files.iter().find(|file| &file.id == id)
    }

    /// Updates the usable full-screen diff size after a terminal resize.
    ///
    /// What the *diff pane* has, not what the terminal has: the layout decisions that hang off this
    /// — whether side by side fits, how far a horizontal scroll may run — are wrong by the width of
    /// the guide pane otherwise, and a side-by-side diff runs its right column off the edge.
    pub(crate) fn set_terminal_size(&mut self, terminal_columns: u16, terminal_rows: u16) {
        self.diff_viewport_rows = usize::from(terminal_rows.saturating_sub(3)).max(1);
        self.diff_viewport_columns = usize::from(self.diff_columns(terminal_columns)).max(1);
        self.terminal_columns = terminal_columns;
    }

    /// Sizes the guide to `width`, the reader's drag, or back to the terminal's share with `None`.
    ///
    /// The diff's column count follows at once, because everything that scrolls it horizontally
    /// reads that rather than the frame.
    pub(crate) fn set_guide_width(&mut self, width: Option<u16>) {
        self.guide_width = width;
        self.diff_viewport_columns = usize::from(self.diff_columns(self.terminal_columns)).max(1);
    }

    /// Cells the guide pane takes in a `terminal_columns`-wide review.
    pub(crate) fn guide_width(&self, terminal_columns: u16) -> u16 {
        crate::workspace_layout::guide_width(terminal_columns, self.guide_width)
    }

    /// Cells the diff pane gets at `terminal_columns`, the guide's share already taken out.
    fn diff_columns(&self, terminal_columns: u16) -> u16 {
        let guide = if terminal_columns >= crate::workspace_layout::WIDE_LAYOUT_MIN_WIDTH
            && self.guide_visible
        {
            self.guide_width(terminal_columns)
                + crate::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH
        } else {
            0
        };
        terminal_columns.saturating_sub(guide).saturating_sub(DIFF_PANE_CHROME)
    }

    /// Advances the visible source-row window by one row, stopping at the file's last row.
    pub(crate) fn next_line(&mut self, review: &GuidedReview) {
        self.scroll_lines(review, 1, true);
    }

    /// Moves the visible source-row window up by one row, stopping at the file's first row.
    pub(crate) fn previous_line(&mut self, review: &GuidedReview) {
        self.scroll_lines(review, 1, false);
    }

    /// Advances by the rendered diff viewport, stopping at the file's last row.
    pub(crate) fn next_page(&mut self, review: &GuidedReview) {
        self.scroll_lines(review, self.diff_viewport_rows, true);
    }

    /// Moves back by the rendered diff viewport, stopping at the file's first row.
    pub(crate) fn previous_page(&mut self, review: &GuidedReview) {
        self.scroll_lines(review, self.diff_viewport_rows, false);
    }

    /// Moves half the rendered diff viewport, as `Ctrl+D` and `Ctrl+U` do in vim.
    pub(crate) fn half_page(&mut self, review: &GuidedReview, forward: bool) {
        self.scroll_lines(review, (self.diff_viewport_rows / 2).max(1), forward);
    }

    /// Hides the guide pane, or brings it back.
    ///
    /// A reviewer who wants every cell for the code, on a terminal that has room for both. Below
    /// the wide tier the guide is not shown at all and this changes nothing visible — which is why
    /// the footer only offers the key where it can act.
    pub(crate) fn toggle_guide(&mut self) {
        self.guide_visible = !self.guide_visible;
        // The diff pane just grew or shrank by the guide's width, and everything that hangs off
        // that — the side-by-side threshold, the horizontal scroll bound — is stale until it is
        // told so.
        self.diff_viewport_columns = usize::from(self.diff_columns(self.terminal_columns)).max(1);
    }
}
