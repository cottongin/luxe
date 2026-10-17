//! Guided Review navigation — line/page/column scrolling, hunk/chapter/file movement,
//! and position reconciliation after a refresh.

mod scroll;
mod search;
mod selection;

use super::*;

impl GuidedReviewWorkspace {
    /// Selects the following hunk in reading order and anchors the diff at its header.
    ///
    /// Past the chapter's last stop it carries on into the next chapter, short of generated churn.
    pub(crate) fn next_hunk(&mut self, review: &GuidedReview) {
        self.move_hunk(review, true);
    }

    /// Selects the preceding hunk in reading order and anchors the diff at its header.
    pub(crate) fn previous_hunk(&mut self, review: &GuidedReview) {
        self.move_hunk(review, false);
    }

    /// Whether the cursor sits on the chapter's last stop, so the footer can offer `]` instead of
    /// a dead key.
    pub(crate) fn at_scope_end(&self, review: &GuidedReview) -> bool {
        let stops = self.stops(review);
        step_stop(&stops, self.current_stop(review, &stops), true).is_none()
    }

    /// Whether the cursor sits on the first stop the current scope allows.
    pub(crate) fn at_scope_start(&self, review: &GuidedReview) -> bool {
        let stops = self.stops(review);
        step_stop(&stops, self.current_stop(review, &stops), false).is_none()
    }

    /// Selects the next semantic Guide chapter and its first reachable diff item.
    pub(crate) fn next_chapter(&mut self, review: &GuidedReview) {
        self.move_chapter(review, true);
    }

    /// Selects the previous semantic Guide chapter and its first reachable diff item.
    pub(crate) fn previous_chapter(&mut self, review: &GuidedReview) {
        self.move_chapter(review, false);
    }

    /// Selects the next file in scope.
    pub(crate) fn next_chapter_file(&mut self, review: &GuidedReview) {
        self.move_file(review, true);
    }

    /// Selects the previous file in scope.
    pub(crate) fn previous_chapter_file(&mut self, review: &GuidedReview) {
        self.move_file(review, false);
    }

    /// Enters the chapter under the guide cursor: its first stop, with the diff focused.
    pub(crate) fn enter_selected_chapter(&mut self, review: &GuidedReview) {
        let stops = self.stops(review);
        if let Some(first) = step_stop(&stops, None, true) {
            self.select_stop(review, stops[first]);
        }
    }

    /// The chapter that scopes cursor movement.
    fn nav_scope<'a>(&self, review: &'a GuidedReview) -> Option<&'a ReviewChapter> {
        self.position
            .chapter_id
            .as_ref()
            .and_then(|id| review.chapters.iter().find(|chapter| &chapter.id == id))
    }

    fn stops(&self, review: &GuidedReview) -> Vec<NavStop> {
        nav_stops(review, self.nav_scope(review), &self.fold, &StopIndex::of(review))
    }

    fn current_stop(&self, review: &GuidedReview, stops: &[NavStop]) -> Option<usize> {
        let file_id = self.position.file_id.as_ref()?;
        stops.iter().position(|stop| {
            let file = &review.files[stop.file];
            &file.id == file_id
                && match stop.hunk {
                    Some(index) => self.position.hunk_id.as_ref() == Some(&file.hunks[index].id),
                    None => self.position.hunk_id.is_none(),
                }
        })
    }

    fn move_hunk(&mut self, review: &GuidedReview, forward: bool) {
        let stops = self.stops(review);
        let Some(target) = step_stop(&stops, self.current_stop(review, &stops), forward) else {
            // Nothing further in this section: the reading of it is done, and stepping carries on
            // into the next one. A boundary that ate the keystroke made the reviewer discover `]`
            // to keep reading, which is not a walkthrough.
            self.note_chapter_walked(review);
            self.step_chapter_boundary(review, forward);
            return;
        };
        self.select_stop(review, stops[target]);
        self.note_chapter_walked(review);
    }

    /// Steps across a chapter boundary, landing on the neighboring section's first (or last)
    /// stop. The narration always owns the diff, because the chapter moves with the cursor.
    fn step_chapter_boundary(&mut self, review: &GuidedReview, forward: bool) {
        let Some((current, target)) = self.adjacent_chapter(review, forward) else {
            return;
        };
        // Stepping never *falls* into generated churn. The terminal bucket a large review folds its
        // leftovers into is real content `]` opens, but walking somebody through a lockfile because
        // they held `Space` is how a walkthrough loses its meaning.
        use luxe_core::guided_review::ChapterKind;
        if forward
            && review.chapters[target].kind == ChapterKind::Generated
            && review.chapters[current].kind != ChapterKind::Generated
        {
            return;
        }
        self.position.chapter_id = Some(review.chapters[target].id.clone());
        let stops = self.stops(review);
        if let Some(index) = step_stop(&stops, None, forward) {
            self.select_stop(review, stops[index]);
        }
    }

    /// Where the cursor is in the whole walkthrough: `(step, total)` over the stops every section
    /// offers, so the reader can see the end coming.
    pub(crate) fn walkthrough_step(&self, review: &GuidedReview) -> Option<(usize, usize)> {
        let mut before = 0usize;
        let mut total = 0usize;
        let mut found = None;
        // One index for every chapter, not one per chapter: this runs on every frame.
        let index = StopIndex::of(review);
        for chapter in &review.chapters {
            let stops = nav_stops(review, Some(chapter), &self.fold, &index);
            if self.position.chapter_id.as_ref() == Some(&chapter.id) {
                found = self.current_stop(review, &stops).map(|index| before + index + 1);
            }
            if found.is_none() {
                before += stops.len();
            }
            total += stops.len();
        }
        found.map(|step| (step, total.max(step)))
    }

    /// Moves the cursor onto a neighboring file within scope: forward lands on that file's first
    /// visible item, backward on the previous file's last one (reading up through the review).
    fn move_file(&mut self, review: &GuidedReview, forward: bool) {
        let stops = self.stops(review);
        let Some(current) = self.current_stop(review, &stops) else {
            let Some(target) = step_stop(&stops, None, forward) else {
                return;
            };
            self.select_stop(review, stops[target]);
            return;
        };
        let file = stops[current].file;
        let target = if forward {
            stops.iter().position(|stop| stop.file > file)
        } else {
            stops.iter().rposition(|stop| stop.file < file)
        };
        let Some(target) = target else {
            return;
        };
        self.select_stop(review, stops[target]);
    }

    fn select_stop(&mut self, review: &GuidedReview, stop: NavStop) {
        let file = &review.files[stop.file];
        self.position.file_id = Some(file.id.clone());
        match stop.hunk {
            Some(index) => self.select_hunk(file, index),
            None => self.clear_hunk_selection(),
        }
    }

    /// The current chapter's index and its neighbor's in the direction of travel, or `None` at
    /// either end. Whether the neighbor may be entered is the caller's policy, not this lookup's.
    fn adjacent_chapter(&self, review: &GuidedReview, forward: bool) -> Option<(usize, usize)> {
        let current = self
            .position
            .chapter_id
            .as_ref()
            .and_then(|id| review.chapters.iter().position(|chapter| &chapter.id == id))?;
        let target = if forward {
            current.checked_add(1).filter(|index| *index < review.chapters.len())
        } else {
            current.checked_sub(1)
        }?;
        Some((current, target))
    }

    fn move_chapter(&mut self, review: &GuidedReview, forward: bool) {
        let Some((_, target)) = self.adjacent_chapter(review, forward) else {
            return;
        };
        let chapter = &review.chapters[target];
        self.position.chapter_id = Some(chapter.id.clone());
        // Land on the new chapter's first stop; the scope is now the new chapter, so `step_stop`
        // with no current position gives exactly that. A chapter of nothing but collapsed churn
        // still has one stop per file, so the diff always follows the rail.
        let stops = self.stops(review);
        if let Some(first) = step_stop(&stops, None, true) {
            self.select_stop(review, stops[first]);
        }
    }
}

#[cfg(test)]
mod jump_tests {
    use super::*;
    use luxe_core::guided_review::{ChapterId, ChapterKind, ReviewChapter};

    /// Two files of two hunks each, the second hunk of each holding `dos needle`. Chapter `first`
    /// holds each file's first hunk and `second` each file's second, so landing on a file does not
    /// by itself pick the right chapter.
    fn review() -> GuidedReview {
        let diff = |path: &str| {
            format!(
                "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n\
                 @@ -1,1 +1,1 @@\n-one\n+uno\n@@ -20,1 +20,1 @@\n-two needle\n+dos needle\n"
            )
        };
        let files = luxe_core::guided_review::parse_unified_diff(&(diff("a.rs") + &diff("b.rs")))
            .expect("the fixture parses")
            .files;
        let chapter = |title: &str, hunk: usize| ReviewChapter {
            id: ChapterId::from_item_keys("test", &[title]),
            title: title.into(),
            rationale: String::new(),
            impact: None,
            kind: ChapterKind::Core,
            items: files
                .iter()
                .map(|file| ReviewItemRef::Hunk(file.hunks[hunk].id.clone()))
                .collect(),
            notes: Vec::new(),
        };
        let mut review = crate::fixture::fixture_demo_guided_review();
        review.chapters = vec![chapter("first", 0), chapter("second", 1)];
        review.files = files;
        review
    }

    fn at_start(review: &GuidedReview) -> GuidedReviewWorkspace {
        let mut workspace = GuidedReviewWorkspace::default();
        workspace.position.file_id = Some(review.files[0].id.clone());
        workspace.position.hunk_id = Some(review.files[0].hunks[0].id.clone());
        workspace.position.chapter_id = Some(review.chapters[0].id.clone());
        workspace
    }

    fn searching(workspace: &mut GuidedReviewWorkspace, query: &str) {
        workspace.begin_search();
        query.chars().for_each(|character| workspace.search_input(character));
    }

    /// Where the cursor sits: `(file, hunk, chapter, row)` as indices into the fixture.
    fn at(
        workspace: &GuidedReviewWorkspace,
        review: &GuidedReview,
    ) -> (usize, usize, usize, usize) {
        let position = &workspace.position;
        let file = review.files.iter().position(|f| Some(&f.id) == position.file_id.as_ref());
        let file = file.expect("on a file");
        let hunks = &review.files[file].hunks;
        let hunk = hunks.iter().position(|h| Some(&h.id) == position.hunk_id.as_ref());
        let chapter =
            review.chapters.iter().position(|c| Some(&c.id) == position.chapter_id.as_ref());
        (file, hunk.expect("on a hunk"), chapter.expect("in a chapter"), position.line_offset)
    }

    /// A search hit in a later hunk of the same file takes the hunk and chapter with the row, and
    /// the next hunk key steps on from there.
    ///
    /// The break this catches: search moved the row alone, so the hunk and the chapter stayed on
    /// the first hunk, and the next hunk key stepped from there.
    #[test]
    fn a_hit_in_a_later_hunk_moves_the_hunk_and_chapter_with_it() {
        let review = review();
        let mut workspace = at_start(&review);
        searching(&mut workspace, "dos needle");
        assert!(workspace.jump_search(&review, true));
        // header 0, two lines, header 3, `-two needle` 4, `+dos needle` 5
        assert_eq!(at(&workspace, &review), (0, 1, 1, 5));
        workspace.next_hunk(&review);
        let (file, hunk, chapter, _) = at(&workspace, &review);
        assert_eq!((file, hunk, chapter), (1, 1, 1), "the chapter's next hunk, in the next file");
    }

    /// A hit in another file's later hunk lands there whole: row, hunk and chapter.
    #[test]
    fn a_hit_across_files_lands_on_its_own_hunk() {
        let review = review();
        let mut workspace = at_start(&review);
        workspace.position.line_offset = 5; // past the first file's hit
        searching(&mut workspace, "dos needle");
        assert!(workspace.jump_search(&review, true));
        assert_eq!(at(&workspace, &review), (1, 1, 1, 5));
    }

    /// A clicked row is a jump like a search hit: its hunk and chapter follow it.
    #[test]
    fn a_clicked_row_moves_the_hunk_and_chapter_with_it() {
        let review = review();
        let mut app =
            crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewSplit)
                .expect("fixture");
        app.guided_review_workspace = at_start(&review);
        app.guided_review = Some(Box::new(review.clone()));
        app.update(crate::app::Msg::GuidedReviewSelectDiffRow(4));
        assert_eq!(at(&app.guided_review_workspace, &review), (0, 1, 1, 4));
    }
}
