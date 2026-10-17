//! Review search — the query, the rows it matches, and jumping between them.

use super::*;

impl GuidedReviewWorkspace {
    /// Opens the in-diff search prompt with an empty query.
    pub(crate) fn begin_search(&mut self) {
        self.search = Some(String::new());
    }

    pub(crate) fn search_input(&mut self, character: char) {
        if let Some(query) = self.search.as_mut() {
            query.push(character);
        }
    }

    pub(crate) fn search_backspace(&mut self) {
        if let Some(query) = self.search.as_mut() {
            query.pop();
        }
    }

    pub(crate) fn cancel_search(&mut self) {
        self.search = None;
    }

    /// Whether a source line matches the active (non-empty) search query, case-insensitively.
    pub(crate) fn line_matches_search(&self, text: &str) -> bool {
        self.search
            .as_ref()
            .filter(|query| !query.is_empty())
            .is_some_and(|query| text.to_lowercase().contains(&query.to_lowercase()))
    }

    /// Visible-row indices in `file` whose source text matches `needle` (already lowercased),
    /// counted like `line_offset` (hunk header + lines, respecting noise folding) so a match row
    /// is jumpable.
    fn file_match_rows(&self, file: &ReviewFile, needle: &str) -> Vec<usize> {
        let mut rows = Vec::new();
        let mut row = 0usize;
        for group in visible_groups(file, &self.fold) {
            let VisibleGroup::Hunk(hunk) = group else {
                row += 1;
                continue;
            };
            row += 1; // hunk header
            for line in &hunk.lines {
                if crate::tabstop::display_text(&line.text).to_lowercase().contains(needle) {
                    rows.push(row);
                }
                row += 1;
            }
        }
        rows
    }

    /// Visible-row indices in the selected file whose source text matches the query.
    fn current_file_match_rows(&self, files: &[ReviewFile], query: &str) -> Vec<usize> {
        let Some(file) =
            self.position.file_id.as_ref().and_then(|id| files.iter().find(|f| &f.id == id))
        else {
            return Vec::new();
        };
        self.file_match_rows(file, &query.to_lowercase())
    }

    /// Jumps the diff cursor to the next/previous search match. Prefers a match past the cursor in
    /// the selected file, then walks to the next/previous file (cyclically) that has a match,
    /// landing on its first (forward) / last (backward) hit; the final step wraps within the
    /// current file. Returns whether a match was found anywhere.
    ///
    /// Find-in-diff is deliberately whole-review, so a hit outside the current chapter re-points
    /// the narrative rather than showing a chapter that does not own the match.
    pub(crate) fn jump_search(&mut self, review: &GuidedReview, forward: bool) -> bool {
        let files = &review.files;
        let Some(query) = self.search.as_ref().filter(|q| !q.is_empty()).cloned() else {
            return false;
        };
        let needle = query.to_lowercase();

        // A match strictly past the cursor in the current file wins first.
        let current_rows = self.current_file_match_rows(files, &query);
        let current = self.position.line_offset;
        let within = if forward {
            current_rows.iter().copied().find(|&row| row > current)
        } else {
            current_rows.iter().copied().rev().find(|&row| row < current)
        };
        if let Some(target) = within {
            self.select_diff_row(review, target);
            return true;
        }

        // Otherwise scan the other files in order (wrapping); the last step revisits the current
        // file, so a single-file query still wraps to its first/last match.
        if files.is_empty() {
            return false;
        }
        let count = files.len();
        let current_index = self
            .position
            .file_id
            .as_ref()
            .and_then(|id| files.iter().position(|file| &file.id == id))
            .unwrap_or(0);
        for step in 1..=count {
            let index = if forward {
                (current_index + step) % count
            } else {
                (current_index + count - step) % count
            };
            let rows = self.file_match_rows(&files[index], &needle);
            let target = if forward { rows.first().copied() } else { rows.last().copied() };
            let Some(target) = target else {
                continue;
            };
            if index != current_index {
                let file_id = files[index].id.clone();
                self.select_file_by_id(review, &file_id);
            }
            self.select_diff_row(review, target);
            return true;
        }
        false
    }
}
