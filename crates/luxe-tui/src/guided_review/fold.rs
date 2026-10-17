//! Fold — the one answer to "is this hunk on screen right now".
//!
//! Folding exists for mechanical churn: a reformatted import block, a regenerated file, a lockfile,
//! a rename. Nothing else. A hunk is never hidden because a test covers it or because someone other
//! than the agent wrote it — that is a *claim*, and a claim belongs on the hunk, not in place of it.
//!
//! There is exactly one predicate because two would disagree: if the renderer folded everything the
//! review budget accounted for while navigation folded only mechanical noise, the cursor could
//! settle on a hunk the renderer refused to draw and the pane would go blank under a breadcrumb.

use luxe_core::guided_review::{FileId, NoiseKind, ReviewFile, ReviewHunk};
use std::collections::HashSet;

/// Which mechanical changes the reviewer currently has open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fold {
    /// Every mechanical hunk in the review is open.
    all: bool,
    /// Files opened one at a time, from their collapsed row.
    files: HashSet<FileId>,
}

impl Fold {
    /// Whether this hunk is collapsed into its file's placeholder row.
    pub fn hides(&self, file: &ReviewFile, hunk: &ReviewHunk) -> bool {
        !self.all && !self.files.contains(&file.id) && is_mechanical(hunk.noise)
    }

    /// Whether every mechanical hunk is open.
    pub const fn is_open(&self) -> bool {
        self.all
    }

    /// Whether this file's mechanical hunks are open.
    pub fn file_is_open(&self, file: &FileId) -> bool {
        self.all || self.files.contains(file)
    }

    /// Opens or re-collapses every mechanical hunk in the review.
    pub fn toggle_all(&mut self) {
        self.all = !self.all;
        if !self.all {
            self.files.clear();
        }
    }

    /// Opens or re-collapses one file's mechanical hunks.
    ///
    /// A file opened while everything is open collapses just that file, which is what the row the
    /// reviewer clicked implies.
    pub fn toggle_file(&mut self, file: &FileId) {
        if self.files.contains(file) {
            self.files.remove(file);
        } else {
            self.files.insert(file.clone());
        }
    }

    /// Opens whatever is needed for this hunk to be on screen.
    pub fn reveal(&mut self, file: &ReviewFile, hunk: &ReviewHunk) {
        if self.hides(file, hunk) {
            self.files.insert(file.id.clone());
        }
    }

    /// Drops per-file state for files a refreshed review no longer carries.
    pub fn reconcile(&mut self, files: &[ReviewFile]) {
        self.files.retain(|id| files.iter().any(|file| &file.id == id));
    }
}

/// Whether a classification describes mechanical churn rather than a decision.
pub const fn is_mechanical(noise: NoiseKind) -> bool {
    matches!(
        noise,
        NoiseKind::FormattingOnly
            | NoiseKind::Generated
            | NoiseKind::Lockfile
            | NoiseKind::Mechanical
    )
}

/// Whether this review contains any mechanical churn at all, open or collapsed.
///
/// With none, the fold controls are dead keys — nothing to collapse, and nothing for `d` to
/// explain. A control that cannot do anything is worse than an absent one: the reviewer presses
/// it, watches nothing happen, and learns that the footer is not to be trusted.
pub fn has_mechanical_churn(files: &[ReviewFile]) -> bool {
    files.iter().flat_map(|file| file.hunks.iter()).any(|hunk| is_mechanical(hunk.noise))
}

/// The reviewer-facing reason a class of change collapses.
pub const fn mechanical_label(noise: NoiseKind) -> &'static str {
    match noise {
        NoiseKind::FormattingOnly => "formatting only",
        NoiseKind::Generated => "generated",
        NoiseKind::Lockfile => "lockfile",
        NoiseKind::Mechanical => "moved or renamed",
        NoiseKind::CommentOnly => "comments only",
        NoiseKind::Semantic | NoiseKind::Unknown => "low signal",
    }
}

/// One collapsed run of consecutive mechanical hunks in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldedRun {
    /// Hunks the run stands for.
    pub hunks: usize,
    /// Source lines the run stands for.
    pub lines: usize,
    /// Classification of the first hunk in the run, which names the collapse.
    pub noise: NoiseKind,
}

impl FoldedRun {
    /// `3 hunks · 148 lines · lockfile`.
    pub fn summary(&self) -> String {
        let hunks =
            format!("{} {}", self.hunks, if self.hunks == 1 { "change" } else { "changes" });
        let lines = format!("{} {}", self.lines, if self.lines == 1 { "line" } else { "lines" });
        format!("{hunks} · {lines} · {}", mechanical_label(self.noise))
    }
}

/// One entry of a file's visible row space: a hunk, or a collapsed run standing in for several.
///
/// Everything that counts rows — the renderer, the scrollbar, cursor offsets, scroll bounds, click
/// mapping — walks this, so they cannot disagree about what is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleGroup<'a> {
    /// An open hunk, occupying its header plus one row per source line.
    Hunk(&'a ReviewHunk),
    /// A collapsed run of consecutive mechanical hunks, occupying exactly one row.
    Folded(FoldedRun),
}

impl VisibleGroup<'_> {
    /// Rows this group occupies.
    pub fn rows(&self) -> usize {
        match self {
            Self::Hunk(hunk) => hunk.lines.len() + 1,
            Self::Folded(_) => 1,
        }
    }
}

/// The unified rows one hunk owns — its header and every line under it — or `None` when the fold
/// has it collapsed.
///
/// The renderer needs this to mark the hunk being read down its whole height, and cannot get it
/// from the rows alone: a window that starts mid-hunk has no header in it to attribute the lines to.
pub fn hunk_row_span(
    groups: &[VisibleGroup<'_>],
    hunk_id: &luxe_core::guided_review::HunkId,
) -> Option<std::ops::Range<usize>> {
    let mut row = 0usize;
    for group in groups {
        let rows = group.rows();
        if matches!(group, VisibleGroup::Hunk(hunk) if &hunk.id == hunk_id) {
            return Some(row..row + rows);
        }
        row += rows;
    }
    None
}

/// A file's visible row space: open hunks in source order, with each run of collapsed mechanical
/// hunks standing as one row where it sits.
pub fn visible_groups<'a>(file: &'a ReviewFile, fold: &Fold) -> Vec<VisibleGroup<'a>> {
    let mut groups: Vec<VisibleGroup<'a>> = Vec::with_capacity(file.hunks.len());
    for hunk in &file.hunks {
        if !fold.hides(file, hunk) {
            groups.push(VisibleGroup::Hunk(hunk));
            continue;
        }
        match groups.last_mut() {
            Some(VisibleGroup::Folded(run)) => {
                run.hunks += 1;
                run.lines += hunk.lines.len();
            }
            _ => groups.push(VisibleGroup::Folded(FoldedRun {
                hunks: 1,
                lines: hunk.lines.len(),
                noise: hunk.noise,
            })),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::guided_review::{HunkId, LineRange, ReviewFileStatus};
    use std::path::Path;

    fn file(path: &str, noise: NoiseKind) -> ReviewFile {
        let id = FileId::from_path_lineage(None, Some(Path::new(path)));
        let hunk = ReviewHunk {
            id: HunkId::from_content(&id, "h", "h"),
            old_range: LineRange::new(1, 1),
            new_range: LineRange::new(1, 1),
            heading: None,
            symbol: None,
            lines: Vec::new(),
            noise,
        };
        ReviewFile {
            id,
            old_path: None,
            new_path: Some(path.into()),
            status: ReviewFileStatus::Modified,
            language: None,
            additions: Some(1),
            deletions: Some(0),
            hunks: vec![hunk],
            binary: false,
            generated: false,
            non_text: None,
        }
    }

    #[test]
    fn only_mechanical_change_folds() {
        let fold = Fold::default();
        let lockfile = file("Cargo.lock", NoiseKind::Lockfile);
        let source = file("src/lib.rs", NoiseKind::Semantic);
        assert!(fold.hides(&lockfile, &lockfile.hunks[0]));
        assert!(!fold.hides(&source, &source.hunks[0]), "a decision is never hidden");
    }

    #[test]
    fn a_file_opens_on_its_own_and_closes_again() {
        let mut fold = Fold::default();
        let lockfile = file("Cargo.lock", NoiseKind::Lockfile);
        let other = file("pnpm-lock.yaml", NoiseKind::Lockfile);

        fold.toggle_file(&lockfile.id);
        assert!(!fold.hides(&lockfile, &lockfile.hunks[0]));
        assert!(fold.hides(&other, &other.hunks[0]), "opening one file opens only that file");

        fold.toggle_file(&lockfile.id);
        assert!(fold.hides(&lockfile, &lockfile.hunks[0]));
    }

    #[test]
    fn revealing_a_hidden_hunk_opens_its_file_and_nothing_else() {
        let mut fold = Fold::default();
        let lockfile = file("Cargo.lock", NoiseKind::Lockfile);
        let other = file("pnpm-lock.yaml", NoiseKind::Lockfile);

        fold.reveal(&lockfile, &lockfile.hunks[0]);

        assert!(!fold.hides(&lockfile, &lockfile.hunks[0]));
        assert!(fold.hides(&other, &other.hunks[0]));
        assert!(!fold.is_open(), "one file is not the whole review");
    }

    #[test]
    fn a_refreshed_review_drops_state_for_files_it_no_longer_carries() {
        let mut fold = Fold::default();
        let gone = file("Cargo.lock", NoiseKind::Lockfile);
        let kept = file("pnpm-lock.yaml", NoiseKind::Lockfile);
        fold.toggle_file(&gone.id);
        fold.toggle_file(&kept.id);

        fold.reconcile(std::slice::from_ref(&kept));

        assert!(fold.file_is_open(&kept.id));
        assert!(!fold.file_is_open(&gone.id));
    }

    #[test]
    fn a_run_says_how_much_it_stands_for() {
        assert_eq!(
            FoldedRun { hunks: 3, lines: 148, noise: NoiseKind::Lockfile }.summary(),
            "3 changes · 148 lines · lockfile"
        );
        assert_eq!(
            FoldedRun { hunks: 1, lines: 1, noise: NoiseKind::FormattingOnly }.summary(),
            "1 change · 1 line · formatting only"
        );
    }
}
