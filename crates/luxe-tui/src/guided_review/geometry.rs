//! Guided Review layout math — viewport widths, gutters, split columns, and item lookups.
//!
//! Pure geometry over the pinned review data. Kept apart from the workspace state machine so
//! the sizing rules read as one place.

use super::fold::{visible_groups, Fold, VisibleGroup};
use super::ReviewPosition;
use luxe_core::guided_review::{
    DiffSide, FileId, FindingAnchor, GuidedReview, HunkId, ReviewChapter, ReviewFile, ReviewHunk,
    ReviewItemRef, ReviewLine, ReviewLineKind,
};
use std::collections::{HashMap, HashSet};

/// One cursor stop in the diff pane: a file, plus its hunk when the file carries text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct NavStop {
    /// Index into the review's canonical file order.
    pub(crate) file: usize,
    /// Index into that file's hunks, or `None` for a non-text item.
    pub(crate) hunk: Option<usize>,
}

/// Row offset of a hunk's header inside the file's *visible* row space — the space the renderer
/// actually lays out. Counting collapsed hunks at full height drops the cursor rows past its target.
pub(super) fn visible_row_offset(file: &ReviewFile, hunk_index: usize, fold: &Fold) -> usize {
    let target = file.hunks.get(hunk_index.min(file.hunks.len().saturating_sub(1)));
    let mut rows = 0usize;
    for group in visible_groups(file, fold) {
        if let (VisibleGroup::Hunk(hunk), Some(target)) = (group, target) {
            if hunk.id == target.id {
                return rows;
            }
        }
        rows += group.rows();
    }
    rows
}

/// Where every file and hunk id sits in the review's canonical order.
///
/// A chapter names its items by id while a stop is a pair of indices, so resolving one is a lookup
/// — and doing it by scanning `review.files`, or for a hunk every file's every hunk, is quadratic
/// in the size of the review. [`super::ReviewWorkspace::walkthrough_step`] does it once per chapter
/// on every frame, which on a 784-file change is 23 ms a frame spent working out a breadcrumb that
/// reads `step 7/312`. Built once and asked per item instead.
pub(super) struct StopIndex<'a> {
    files: HashMap<&'a FileId, usize>,
    hunks: HashMap<&'a HunkId, NavStop>,
}

impl<'a> StopIndex<'a> {
    /// Indexes every file and hunk in `review`.
    pub(super) fn of(review: &'a GuidedReview) -> Self {
        let mut files = HashMap::with_capacity(review.files.len());
        let mut hunks = HashMap::new();
        for (index, file) in review.files.iter().enumerate() {
            files.insert(&file.id, index);
            for (hunk, entry) in file.hunks.iter().enumerate() {
                hunks.insert(&entry.id, NavStop { file: index, hunk: Some(hunk) });
            }
        }
        Self { files, hunks }
    }
}

/// Ordered cursor stops the current presentation allows.
///
/// Inside a section that is the order **the guide chose** for `chapter.items` — the schema before
/// the logic that consumes it, the call sites before the tests that cover them, which is the order
/// the work was reasoned in and the whole reason a walkthrough beats a file list. Outside one (All
/// Files) it is canonical source order.
///
/// A text file whose every hunk is collapsed still earns one stop. Skipping it would let a
/// walkthrough step through fifteen files showing nothing: the stop lands on the file's collapsed
/// rows, which say what is there and open in place.
pub(super) fn nav_stops(
    review: &GuidedReview,
    chapter: Option<&ReviewChapter>,
    fold: &Fold,
    index: &StopIndex<'_>,
) -> Vec<NavStop> {
    let candidates = match chapter {
        Some(chapter) => chapter_stops(review, chapter, index),
        None => canonical_stops(review),
    };
    apply_fold(review, fold, &candidates)
}

/// Every reviewable item in canonical source order, before folding.
fn canonical_stops(review: &GuidedReview) -> Vec<NavStop> {
    let mut stops = Vec::new();
    for (index, file) in review.files.iter().enumerate() {
        if file.non_text.is_some() {
            stops.push(NavStop { file: index, hunk: None });
            continue;
        }
        stops.extend((0..file.hunks.len()).map(|hunk| NavStop { file: index, hunk: Some(hunk) }));
    }
    stops
}

/// A section's items in the order the guide listed them, before folding.
///
/// A chapter may name a file and one of its hunks both, so repeats are dropped — through a set,
/// because scanning what has been collected so far is quadratic in a chapter's own length.
fn chapter_stops(
    review: &GuidedReview,
    chapter: &ReviewChapter,
    index: &StopIndex<'_>,
) -> Vec<NavStop> {
    let mut stops: Vec<NavStop> = Vec::new();
    let mut seen: HashSet<NavStop> = HashSet::new();
    let mut push = |stops: &mut Vec<NavStop>, stop: NavStop| {
        if seen.insert(stop) {
            stops.push(stop);
        }
    };
    for item in &chapter.items {
        match item {
            ReviewItemRef::File(file_id) => {
                let Some(&file_index) = index.files.get(file_id) else {
                    continue;
                };
                let file = &review.files[file_index];
                if file.non_text.is_some() {
                    push(&mut stops, NavStop { file: file_index, hunk: None });
                } else {
                    for hunk in 0..file.hunks.len() {
                        push(&mut stops, NavStop { file: file_index, hunk: Some(hunk) });
                    }
                }
            }
            ReviewItemRef::Hunk(hunk_id) => {
                if let Some(&stop) = index.hunks.get(hunk_id) {
                    push(&mut stops, stop);
                }
            }
        }
    }
    stops
}

/// Drops collapsed hunks, keeping one stop per run of them so a file can never vanish.
fn apply_fold(review: &GuidedReview, fold: &Fold, candidates: &[NavStop]) -> Vec<NavStop> {
    let mut stops: Vec<NavStop> = Vec::with_capacity(candidates.len());
    let mut run = 0usize;
    while run < candidates.len() {
        let file_index = candidates[run].file;
        let end = candidates[run..]
            .iter()
            .position(|stop| stop.file != file_index)
            .map_or(candidates.len(), |offset| run + offset);
        let file = &review.files[file_index];
        let before = stops.len();
        stops.extend(
            candidates[run..end]
                .iter()
                .copied()
                .filter(|stop| stop.hunk.is_none_or(|hunk| !fold.hides(file, &file.hunks[hunk]))),
        );
        if stops.len() == before {
            stops.push(NavStop { file: file_index, hunk: None });
        }
        run = end;
    }
    stops
}

/// One bounded step through `stops`. Returns `None` at either end — movement never wraps, so a
/// chapter boundary stays a boundary the footer can explain.
pub(super) fn step_stop(stops: &[NavStop], current: Option<usize>, forward: bool) -> Option<usize> {
    if stops.is_empty() {
        return None;
    }
    match (current, forward) {
        (Some(index), true) => (index + 1 < stops.len()).then_some(index + 1),
        (Some(index), false) => index.checked_sub(1),
        (None, true) => Some(0),
        (None, false) => Some(stops.len() - 1),
    }
}

pub(super) fn selected_file_row_count(
    position: &ReviewPosition,
    files: &[ReviewFile],
    fold: &Fold,
) -> usize {
    position
        .file_id
        .as_ref()
        .and_then(|file_id| files.iter().find(|file| &file.id == file_id))
        .map(|file| visible_groups(file, fold).iter().map(VisibleGroup::rows).sum())
        .unwrap_or_default()
}

pub(super) fn finding_anchor_for_line(
    file: &ReviewFile,
    hunk: &ReviewHunk,
    line: &ReviewLine,
) -> Option<FindingAnchor> {
    let (side, line_number) = match line.kind {
        ReviewLineKind::Deletion => (DiffSide::Old, line.old_line),
        ReviewLineKind::Addition => (DiffSide::New, line.new_line),
        ReviewLineKind::Context => (DiffSide::New, line.new_line.or(line.old_line)),
        ReviewLineKind::NoNewlineMarker => return None,
    };

    line_number.map(|line_number| FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side,
        line: line_number,
        context_hash: line.context_hash(),
    })
}

/// The widest line `file` holds, in terminal cells.
pub(super) fn file_source_width(file: &ReviewFile) -> usize {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| crate::tabstop::display_width(&line.text))
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
