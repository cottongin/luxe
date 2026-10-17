//! Review selection — what a click or a jump selects, and where the position lands after a
//! refresh.

use super::*;

impl GuidedReviewWorkspace {
    /// Moves the cursor to `offset` as an explicit jump does (a click, a search hit): within the
    /// file's rows, the hunk following the row and the chapter following the hunk.
    pub(crate) fn select_diff_row(&mut self, review: &GuidedReview, offset: usize) {
        let rows = selected_file_row_count(&self.position, &review.files, &self.fold);
        self.position.line_offset = offset.min(rows.saturating_sub(1));
        self.sync_hunk_to_offset(&review.files);
        self.sync_chapter_to_selection(review);
    }

    /// Points the review at one place in the guide: a chapter, or a file inside one.
    ///
    /// The only way in from outside the stepping keys — a click on a guide row, a jump from a
    /// finding. There is nothing to "open" afterwards, because the detail pane already shows
    /// whatever the position lands on.
    pub(crate) fn open_guide_target(
        &mut self,
        review: &GuidedReview,
        file_id: Option<&FileId>,
        hunk_id: Option<&HunkId>,
        chapter_id: Option<&ChapterId>,
    ) {
        if let Some(chapter_id) = chapter_id {
            self.position.chapter_id = Some(chapter_id.clone());
            let stops = self.stops(review);
            if let Some(first) = step_stop(&stops, None, true) {
                self.select_stop(review, stops[first]);
            }
            return;
        }
        let Some(file_id) = file_id else {
            return;
        };
        let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
            return;
        };
        match hunk_id.and_then(|id| file.hunks.iter().position(|hunk| &hunk.id == id)) {
            Some(index) => self.select_finding_hunk(file, index, None),
            None => self.select_file_by_id(review, file_id),
        }
        self.sync_chapter_to_selection(review);
    }

    /// Selects a file by id (a click on its rail row): makes it current, focuses
    /// its first visible hunk, and re-points the narrative at the owning chapter. Reveals folded
    /// content when that is the only way to show the file. No-op for an unknown or empty file.
    pub(crate) fn select_file_by_id(&mut self, review: &GuidedReview, file_id: &FileId) {
        let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
            return;
        };
        if file.non_text.is_none() && file.hunks.is_empty() {
            return;
        }
        self.position.file_id = Some(file.id.clone());
        // Land on the first hunk the fold leaves open rather than opening the file's churn just
        // because it happens to sort first.
        match file.hunks.iter().position(|hunk| !self.fold.hides(file, hunk)) {
            Some(index) => self.select_hunk(file, index),
            None => self.clear_hunk_selection(),
        }
        self.sync_chapter_to_selection(review);
    }

    pub(in crate::guided_review) fn select_finding_hunk(
        &mut self,
        file: &ReviewFile,
        index: usize,
        line_index: Option<usize>,
    ) {
        self.reveal_if_folded(file, &file.hunks[index]);
        self.position.file_id = Some(file.id.clone());
        self.position.hunk_id = Some(file.hunks[index].id.clone());
        self.position.line_offset = visible_row_offset(file, index, &self.fold)
            .saturating_add(usize::from(line_index.is_some()))
            .saturating_add(line_index.unwrap_or_default());
    }

    pub(super) fn select_hunk(&mut self, file: &ReviewFile, index: usize) {
        let hunk_id = file.hunks[index].id.clone();
        self.position.hunk_id = Some(hunk_id);
        self.position.line_offset = visible_row_offset(file, index, &self.fold);
        // A no-op within one file; arriving at a narrower one, it keeps the code on screen
        self.diff_column_offset = self.diff_column_offset.min(self.column_maximum(file));
    }

    pub(super) fn clear_hunk_selection(&mut self) {
        self.position.hunk_id = None;
        self.position.line_offset = 0;
    }

    /// Re-points the narrative at the chapter owning the current selection.
    ///
    /// Bounded stepping is scoped and can never leave a chapter, but an explicit jump (a click,
    /// next-unreviewed, a finding anchor, a search hit) can — and a rail describing chapter 01
    /// beside chapter 03's diff is worse than no narrative at all.
    pub(in crate::guided_review) fn sync_chapter_to_selection(&mut self, review: &GuidedReview) {
        let owning = review.chapters.iter().find(|chapter| {
            chapter.items.iter().any(|item| match item {
                ReviewItemRef::Hunk(hunk_id) => self.position.hunk_id.as_ref() == Some(hunk_id),
                ReviewItemRef::File(file_id) => {
                    self.position.hunk_id.is_none()
                        && self.position.file_id.as_ref() == Some(file_id)
                }
            })
        });
        if let Some(chapter) = owning {
            self.position.chapter_id = Some(chapter.id.clone());
        }
    }

    /// Opens collapsed content when an explicit jump targets a hunk the fold is hiding, so a
    /// selection always shows the thing it selected — and only that file, not the whole review.
    fn reveal_if_folded(&mut self, file: &ReviewFile, hunk: &ReviewHunk) {
        self.fold.reveal(file, hunk);
    }

    pub(in crate::guided_review) fn reconcile_folded_selection(&mut self, files: &[ReviewFile]) {
        let hunks = files
            .iter()
            .enumerate()
            .flat_map(|(file_index, file)| {
                file.hunks
                    .iter()
                    .enumerate()
                    .map(move |(hunk_index, hunk)| (file_index, hunk_index, hunk.id.clone()))
            })
            .collect::<Vec<_>>();
        let Some(start) = self
            .position
            .hunk_id
            .as_ref()
            .and_then(|selected| hunks.iter().position(|(_, _, hunk_id)| hunk_id == selected))
        else {
            return;
        };
        let Some((file_index, hunk_index, _)) =
            hunks.iter().cycle().skip(start).take(hunks.len()).find(
                |(file_index, hunk_index, _)| {
                    let file = &files[*file_index];
                    !self.fold.hides(file, &file.hunks[*hunk_index])
                },
            )
        else {
            return;
        };
        let (file_index, hunk_index) = (*file_index, *hunk_index);
        let file = &files[file_index];
        self.position.file_id = Some(file.id.clone());
        self.select_hunk(file, hunk_index);
    }

    /// Keeps a semantic chapter selection when its stable identity survives refresh.
    pub(crate) fn reconcile_chapter(&mut self, chapters: &[ReviewChapter]) {
        if self
            .position
            .chapter_id
            .as_ref()
            .is_some_and(|id| chapters.iter().any(|chapter| &chapter.id == id))
        {
            return;
        }
        self.position.chapter_id = chapters.first().map(|chapter| chapter.id.clone());
    }

    pub(crate) fn reconcile_position(&mut self, files: &[ReviewFile]) {
        self.fold.reconcile(files);
        let retained = self.position.file_id.as_ref().and_then(|file_id| {
            files.iter().find(|file| &file.id == file_id).and_then(|file| {
                match &self.position.hunk_id {
                    Some(hunk_id) => file
                        .hunks
                        .iter()
                        .any(|hunk| &hunk.id == hunk_id)
                        .then(|| (file_id.clone(), Some(hunk_id.clone()))),
                    None if file.non_text.is_some() => Some((file_id.clone(), None)),
                    None => None,
                }
            })
        });
        // Open on something the reviewer can actually read: the first hunk the fold leaves open,
        // then a non-text item, and only then a file that is nothing but collapsed churn. Landing
        // on a collapsed hunk would open the review on a blank pane.
        let next = retained.or_else(|| {
            files
                .iter()
                .find_map(|file| {
                    file.hunks
                        .iter()
                        .find(|hunk| !self.fold.hides(file, hunk))
                        .map(|hunk| (file.id.clone(), Some(hunk.id.clone())))
                })
                .or_else(|| {
                    files
                        .iter()
                        .find(|file| file.non_text.is_some())
                        .map(|file| (file.id.clone(), None))
                })
                .or_else(|| {
                    files
                        .iter()
                        .find(|file| file.is_text() && !file.hunks.is_empty())
                        .map(|file| (file.id.clone(), None))
                })
        });

        match next {
            Some((file_id, hunk_id))
                if self.position.file_id.as_ref() == Some(&file_id)
                    && self.position.hunk_id == hunk_id => {}
            Some((file_id, hunk_id)) => {
                self.position = ReviewPosition {
                    chapter_id: self.position.chapter_id.clone(),
                    file_id: Some(file_id),
                    hunk_id,
                    line_offset: 0,
                };
            }
            None => {
                self.position = ReviewPosition::default();
            }
        }
    }
}
