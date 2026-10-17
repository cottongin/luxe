//! Scroll state management for conversation view.
//!
//! Tracks scroll offset, follow mode (auto-stick-to-bottom while streaming), and viewport
//! height for virtualized rendering.

/// Scroll state for a virtualized list of lines.
#[derive(Debug, Clone)]
pub struct ScrollState {
    /// Top-line offset into the flattened line list.
    pub offset: usize,
    /// Whether to auto-stick to bottom (enabled while streaming, disabled on manual scroll).
    pub follow: bool,
    /// Current viewport height in lines.
    pub viewport: usize,
    /// Total number of lines.
    pub total: usize,
}

impl ScrollState {
    /// Create a new scroll state with follow mode enabled and offset at bottom.
    pub fn new(viewport: usize, total: usize) -> Self {
        let offset = total.saturating_sub(viewport);
        Self { offset, follow: true, viewport, total }
    }

    /// The largest offset that still fills the viewport — the bottom of the scroll range.
    pub fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.viewport)
    }

    /// Move up by one line. Disables follow mode.
    pub fn line_up(&mut self) {
        self.up_by(1);
    }

    /// Move down by one line. Re-enables follow mode if at bottom.
    pub fn line_down(&mut self) {
        self.down_by(1);
    }

    /// Move up by `lines`. Disables follow mode when it actually moves.
    pub fn up_by(&mut self, lines: usize) {
        if self.offset == 0 {
            return;
        }
        self.offset = self.offset.saturating_sub(lines);
        self.follow = false;
    }

    /// Move down by `lines`, stopping at the bottom (where follow mode resumes).
    pub fn down_by(&mut self, lines: usize) {
        let max_offset = self.max_offset();
        if self.offset >= max_offset {
            return;
        }
        self.offset = (self.offset + lines).min(max_offset);
        if self.offset == max_offset {
            self.follow = true;
        }
    }

    /// Jumps so `offset` lands on the position the scrollbar thumb was dragged to.
    ///
    /// `row` is the pointer's row within a `height`-tall track. Dragged to the end, following
    /// resumes: new output keeps arriving in view, which is what the gesture asked for.
    pub fn drag_to(&mut self, row: u16, height: u16) {
        let max_offset = self.max_offset();
        if max_offset == 0 || height == 0 {
            return;
        }
        self.offset = crate::widgets::drag_offset(row, height, max_offset);
        self.follow = self.offset >= max_offset;
    }

    /// Move up by one page (viewport height). Disables follow mode.
    pub fn page_up(&mut self) {
        let pages = self.viewport;
        if self.offset > pages {
            self.offset -= pages;
        } else {
            self.offset = 0;
        }
        self.follow = false;
    }

    /// Move down by one page (viewport height). Re-enables follow mode if at bottom.
    pub fn page_down(&mut self) {
        let pages = self.viewport;
        let max_offset = self.max_offset();
        if self.offset + pages < max_offset {
            self.offset += pages;
        } else {
            self.offset = max_offset;
        }
        if self.offset == max_offset {
            self.follow = true;
        }
    }

    /// Jump to the bottom. Re-enables follow mode.
    pub fn to_bottom(&mut self) {
        self.offset = self.max_offset();
        self.follow = true;
    }

    /// Handle new content being added. If following, stay pinned to bottom.
    pub fn on_new_content(&mut self, new_total: usize) {
        self.total = new_total;
        if self.follow {
            self.to_bottom();
        } else {
            // Stay at current offset, but clamp if new content pushed us past the end.
            self.offset = self.offset.min(self.max_offset());
        }
    }

    /// Get the range of visible line indices.
    pub fn window(&self) -> std::ops::Range<usize> {
        let start = self.offset;
        let end = (start + self.viewport).min(self.total);
        start..end
    }

    /// Recompute viewport and clamp offset after a resize.
    ///
    /// A shrinking viewport (the composer growing a row as the draft wraps, the pending tray
    /// opening) raises the bottom of the scroll range. Following means "pinned to the bottom", so
    /// re-derive the offset rather than leaving it stranded a few lines above the end until the
    /// next token happens to arrive.
    pub fn on_resize(&mut self, new_viewport: usize) {
        self.viewport = new_viewport;
        if self.follow {
            self.offset = self.max_offset();
        } else {
            self.offset = self.offset.min(self.max_offset());
        }
    }

    /// Get the percentage position (0..=100).
    pub fn position_percent(&self) -> usize {
        if self.total <= self.viewport {
            100
        } else {
            ((self.offset as f64 / self.max_offset() as f64) * 100.0) as usize
        }
    }
}

#[cfg(test)]
mod tests;
