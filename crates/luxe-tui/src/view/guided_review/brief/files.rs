//! Brief files — which chapter owns each file, and the file rows the brief lists.

use super::super::context::review_file_status_style;
use crate::app::App;
use luxe_core::guided_review::{
    Accounted, FileId, GuidedReview, HunkId, ItemVerdict, ReviewChapter, ReviewFile, ReviewHunk,
    ReviewItemRef,
};
use ratatui::text::{Line, Span};
use std::collections::HashMap;

/// Which file owns each id a chapter can reference.
///
/// A chapter names files and hunks by id, and resolving one by scanning every file's every hunk
/// is quadratic in the review — a forty-file change spent a few hundred thousand comparisons a
/// frame answering the same question. Built once per pass and asked per item.
pub(crate) struct FileOwners<'a> {
    by_file: HashMap<&'a FileId, &'a ReviewFile>,
    by_hunk: HashMap<&'a HunkId, &'a ReviewFile>,
}

impl<'a> FileOwners<'a> {
    /// Indexes every file and hunk in `review`.
    pub(crate) fn of(review: &'a GuidedReview) -> Self {
        let mut by_file = HashMap::with_capacity(review.files.len());
        let mut by_hunk = HashMap::new();
        for file in &review.files {
            by_file.insert(&file.id, file);
            for hunk in &file.hunks {
                by_hunk.insert(&hunk.id, file);
            }
        }
        Self { by_file, by_hunk }
    }

    /// The file one chapter item lives in.
    fn owner(&self, item: &ReviewItemRef) -> Option<&'a ReviewFile> {
        match item {
            ReviewItemRef::File(file_id) => self.by_file.get(file_id).copied(),
            ReviewItemRef::Hunk(hunk_id) => self.by_hunk.get(hunk_id).copied(),
        }
    }

    /// The files a chapter owns, in the order it lists them.
    ///
    /// A chapter references hunks as often as whole files, so a hunk resolves to the file that
    /// holds it and repeats are dropped — otherwise a chapter that touches one file six times
    /// lists it six times.
    pub(crate) fn chapter_files(&self, chapter: &ReviewChapter) -> Vec<&'a ReviewFile> {
        let mut files: Vec<&ReviewFile> = Vec::new();
        for item in &chapter.items {
            if let Some(file) = self.owner(item) {
                if !files.iter().any(|seen| seen.id == file.id) {
                    files.push(file);
                }
            }
        }
        files
    }
}

/// One file under its chapter: status, path, its share of the change, and why it is accounted for.
pub(in crate::view::guided_review) fn file_line(
    app: &App,
    file: &ReviewFile,
    selected: bool,
    width: usize,
) -> Line<'static> {
    use luxe_core::guided_review::ReviewFileStatus as Status;
    let styles = &app.theme_styles;
    let letter = match file.status {
        Status::Added => "A",
        Status::Modified => "M",
        Status::Deleted => "D",
        Status::Renamed => "R",
        Status::Copied => "C",
        Status::Untracked => "?",
    };
    let path = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map_or_else(|| "(unknown path)".to_owned(), |path| path.display().to_string());
    // The path is elided from the left where it must be: the basename is what identifies a file,
    // and a pane sized for prose has little room for the rest.
    let shown = crate::widgets::elide_path_components(
        &path,
        width.saturating_sub(FILE_ROW_LEAN + crate::widgets::SELECTION_FRAME + 14).max(8),
    );
    let mut spans = vec![
        Span::styled(format!("{letter} "), review_file_status_style(file.status, app)),
        Span::styled(shown, styles.text),
    ];
    spans.extend(change_spans(app, file, &file.hunks.iter().collect::<Vec<_>>()));
    // One reason, said once, when it covers the whole file. Repeating it per hunk was how a
    // forty-file listing turned into four hundred identical claims.
    if let Some(reason) = shared_reason(app, file) {
        spans.push(Span::styled(
            format!("  {reason}"),
            if selected { styles.muted } else { styles.dim },
        ));
    }
    // Attention rides on the file it belongs to, where the decision to open it is made — but not
    // on the file being read, where the detail pane is already spelling the same concern out in a
    // sentence beside its code. A badge there is the warning said twice in two widths.
    let mut flags = app.review.flags.iter().filter(|_| !selected);
    if let Some(signal) = flags.find(|signal| signal.path.as_deref() == Some(path.as_str())) {
        spans.push(Span::styled(format!("  {}", signal.kind.badge()), styles.warning));
    }
    // Cut from the slab that means *chosen* everywhere else, over the pane's own fill rather than
    // the color behind it. The frame is spent on every row, which is what indents the list under
    // the chapter that owns it and stops names sliding sideways as the cursor arrives.
    crate::widgets::selection_row_on(
        spans,
        // The lean is spent *outside* the slab, so the room it gets is what is left after it.
        width.saturating_sub(FILE_ROW_LEAN),
        FILE_ROW_LEAN,
        selected,
        styles.tool_bg.bg.unwrap_or(styles.slab.canvas),
        styles,
    )
}

/// Cells a file row leans in under the chapter heading that owns it.
const FILE_ROW_LEAN: usize = 2;

/// The one thing that accounts for *every* hunk in this file, when there is one.
fn shared_reason(app: &App, file: &ReviewFile) -> Option<String> {
    let mut reasons =
        file.hunks.iter().map(|hunk| match app.review.budget.hunk_verdict(&hunk.id) {
            ItemVerdict::Accounted(accounted) => Some(accounted),
            ItemVerdict::NeedsReview => None,
        });
    let first: Accounted = reasons.next()??;
    reasons.all(|reason| reason.as_ref() == Some(&first)).then(|| format!("all {}", first.label()))
}

/// `+12 −3` for a text file, or what the change actually was for one with no lines to count.
fn change_spans(app: &App, file: &ReviewFile, owned: &[&ReviewHunk]) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    if let Some(change) = file.non_text.as_ref() {
        return vec![Span::styled(format!(" {}", change.summary), styles.muted)];
    }
    // Git's own figures where it gave them, so the guide and the dock agree about the same file.
    // Summing the visible hunks would report the *shown* change, which is a different number the
    // moment anything is folded.
    let (additions, deletions) = match (file.additions, file.deletions) {
        (Some(additions), Some(deletions)) => (additions as usize, deletions as usize),
        _ if owned.is_empty() => return Vec::new(),
        _ => owned.iter().flat_map(|hunk| hunk.lines.iter()).fold(
            (0usize, 0usize),
            |(add, del), line| match line.kind {
                luxe_core::guided_review::ReviewLineKind::Addition => (add + 1, del),
                luxe_core::guided_review::ReviewLineKind::Deletion => (add, del + 1),
                _ => (add, del),
            },
        ),
    };
    vec![
        Span::styled(format!(" +{additions}"), styles.success),
        Span::styled(format!(" \u{2212}{deletions}"), styles.error),
    ]
}
