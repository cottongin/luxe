//! Unified diff row projection — bounded rendering data over immutable review hunks.
//!
//! The renderer asks for a visible row window instead of materializing every line in a large
//! review. Source lines remain borrowed from the pinned review and retain both line anchors.

use super::fold::{FoldedRun, VisibleGroup};
use luxe_core::guided_review::{HunkId, LineRange, ReviewHunk, ReviewLine, ReviewLineKind};
use unicode_width::UnicodeWidthChar;

/// A visible unified-diff row borrowed from the pinned review source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnifiedDiffRow<'a> {
    /// An orientation header for one source-anchored hunk.
    HunkHeader {
        /// Stable source identity used by review navigation.
        hunk_id: &'a HunkId,
        /// Old-side range shown in the canonical unified header.
        old_range: LineRange,
        /// New-side range shown in the canonical unified header.
        new_range: LineRange,
        /// Optional trailing context supplied by the diff source.
        heading: Option<&'a str>,
    },
    /// One exact source line with old/new anchors and a semantic diff role.
    Source(&'a ReviewLine),
    /// A run of collapsed mechanical hunks, standing in for itself on one row.
    Folded(FoldedRun),
}

/// One side of a synchronized split-diff row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitDiffCell<'a> {
    /// An exact immutable source line for this side.
    Source(&'a ReviewLine),
    /// An intentionally empty counterpart for an asymmetric change.
    Empty,
}

/// One row of the side-by-side projection.
///
/// Every row carries the **unified** row index it starts at, which is what makes side-by-side a
/// *view* of the same row space rather than a second one: scroll position, hunk selection, click
/// mapping, the scrollbar and search all keep working against `position.line_offset` unchanged. The
/// old renderer paged a single hunk against its own private offset, which had to be kept in sync with
/// the real one and could not show a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitRow<'a> {
    /// A hunk header, spanning both columns.
    Header {
        /// Stable source identity used by review navigation.
        hunk_id: &'a HunkId,
        /// Old-side range shown in the canonical unified header.
        old_range: LineRange,
        /// New-side range shown in the canonical unified header.
        new_range: LineRange,
        /// Optional trailing context supplied by the diff source.
        heading: Option<&'a str>,
        /// Index of this row in the unified row space.
        unified_index: usize,
    },
    /// Old and new content side by side.
    Pair {
        /// Hunk this row belongs to.
        hunk_id: &'a HunkId,
        /// Old-side content or an explicit gap.
        old: SplitDiffCell<'a>,
        /// New-side content or an explicit gap.
        new: SplitDiffCell<'a>,
        /// Index of the first unified row this pair covers.
        unified_index: usize,
        /// Index of the last, so a window starting at the pair's other half still holds it
        last_index: usize,
    },
    /// A run of collapsed mechanical hunks, spanning both columns.
    Folded {
        /// What the run stands for.
        run: FoldedRun,
        /// Index of this row in the unified row space.
        unified_index: usize,
    },
}

impl SplitRow<'_> {
    /// The unified row index this row starts at.
    pub(crate) fn unified_index(&self) -> usize {
        match self {
            Self::Header { unified_index, .. }
            | Self::Pair { unified_index, .. }
            | Self::Folded { unified_index, .. } => *unified_index,
        }
    }

    /// The last unified row this row covers: its own index, or a pair's later half.
    fn last_index(&self) -> usize {
        match self {
            Self::Pair { last_index, .. } => *last_index,
            other => other.unified_index(),
        }
    }

    /// The hunk this row belongs to, when it belongs to one.
    #[cfg(test)]
    pub(crate) fn hunk_id(&self) -> Option<&HunkId> {
        match self {
            Self::Header { hunk_id, .. } | Self::Pair { hunk_id, .. } => Some(hunk_id),
            Self::Folded { .. } => None,
        }
    }
}

/// Projects a window of side-by-side rows for one file, in the unified row space.
///
/// `offset`/`height` mean exactly what they mean for [`visible_rows`]: the first unified row to
/// show and how many *rendered* rows fit. Pairing collapses a deletion and its replacement onto one
/// row, so a window of `height` rows can cover more than `height` unified rows — which is the point
/// of the layout.
pub(crate) fn visible_split_rows<'a>(
    groups: &[VisibleGroup<'a>],
    offset: usize,
    height: usize,
) -> Vec<SplitRow<'a>> {
    if height == 0 {
        return Vec::new();
    }
    let mut rows: Vec<SplitRow<'a>> = Vec::with_capacity(height);
    let mut unified = 0usize;
    for group in groups {
        let hunk = match group {
            VisibleGroup::Folded(run) => {
                if unified >= offset && rows.len() < height {
                    rows.push(SplitRow::Folded { run: *run, unified_index: unified });
                }
                unified += 1;
                if rows.len() == height && unified > offset {
                    return rows;
                }
                continue;
            }
            VisibleGroup::Hunk(hunk) => hunk,
        };
        if unified >= offset && rows.len() < height {
            rows.push(SplitRow::Header {
                hunk_id: &hunk.id,
                old_range: hunk.old_range,
                new_range: hunk.new_range,
                heading: hunk.heading.as_deref(),
                unified_index: unified,
            });
        }
        unified += 1;
        if rows.len() == height && unified > offset {
            return rows;
        }

        let mut deletions: Vec<(usize, &ReviewLine)> = Vec::new();
        let mut additions: Vec<(usize, &ReviewLine)> = Vec::new();

        for line in &hunk.lines {
            let index = unified;
            unified += 1;
            match line.kind {
                ReviewLineKind::Deletion => deletions.push((index, line)),
                ReviewLineKind::Addition => additions.push((index, line)),
                ReviewLineKind::Context | ReviewLineKind::NoNewlineMarker => {
                    project_change_run(
                        &mut rows,
                        &hunk.id,
                        (&mut deletions, &mut additions),
                        (offset, height),
                    );
                    push_split_row(
                        &mut rows,
                        SplitRow::Pair {
                            hunk_id: &hunk.id,
                            old: SplitDiffCell::Source(line),
                            new: SplitDiffCell::Source(line),
                            unified_index: index,
                            last_index: index,
                        },
                        offset,
                        height,
                    );
                }
            }
            if rows.len() == height {
                return rows;
            }
        }
        project_change_run(&mut rows, &hunk.id, (&mut deletions, &mut additions), (offset, height));
        if rows.len() == height {
            return rows;
        }
    }
    rows
}

/// Changed lines of one kind in a run, each with its unified index.
type ChangeRun<'a> = Vec<(usize, &'a ReviewLine)>;

/// One run of changes as split rows: deletions paired positionally with the additions after
/// them, which is also what makes the intra-line marks exact here, then whatever is unpaired.
fn project_change_run<'a>(
    rows: &mut Vec<SplitRow<'a>>,
    hunk_id: &'a HunkId,
    (deletions, additions): (&mut ChangeRun<'a>, &mut ChangeRun<'a>),
    (offset, height): (usize, usize),
) {
    let paired = deletions.len().min(additions.len());
    for index in 0..paired {
        let (old_index, old) = deletions[index];
        let (new_index, new) = additions[index];
        push_split_row(
            rows,
            SplitRow::Pair {
                hunk_id,
                old: SplitDiffCell::Source(old),
                new: SplitDiffCell::Source(new),
                unified_index: old_index.min(new_index),
                last_index: old_index.max(new_index),
            },
            offset,
            height,
        );
    }
    for (index, line) in deletions.drain(..).skip(paired) {
        push_split_row(
            rows,
            SplitRow::Pair {
                hunk_id,
                old: SplitDiffCell::Source(line),
                new: SplitDiffCell::Empty,
                unified_index: index,
                last_index: index,
            },
            offset,
            height,
        );
    }
    for (index, line) in additions.drain(..).skip(paired) {
        push_split_row(
            rows,
            SplitRow::Pair {
                hunk_id,
                old: SplitDiffCell::Empty,
                new: SplitDiffCell::Source(line),
                unified_index: index,
                last_index: index,
            },
            offset,
            height,
        );
    }
    deletions.clear();
    additions.clear();
}

fn push_split_row<'a>(
    rows: &mut Vec<SplitRow<'a>>,
    row: SplitRow<'a>,
    offset: usize,
    height: usize,
) {
    // Once the window has started, every row after it belongs. It starts at the row holding the
    // offset, which for a pair may be its later half, or else the first row past it.
    let starts = row.unified_index() >= offset || row.last_index() == offset;
    if (!rows.is_empty() || starts) && rows.len() < height {
        rows.push(row);
    }
}

/// Returns the minimum fixed gutter width that can display every visible source line number.
pub(crate) fn gutter_width(hunks: &[ReviewHunk]) -> usize {
    hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .flat_map(|line| [line.old_line, line.new_line])
        .flatten()
        .map(u32::ilog10)
        .map(|digits| digits as usize + 1)
        .max()
        .unwrap_or(1)
}

/// Projects a window of a file's visible row space, retaining bounded allocation and source
/// identity.
///
/// `offset` counts rows in that space — a hunk header plus its source lines, or one row per
/// collapsed run — which is the same space navigation and the scrollbar count in. A zero-sized
/// window returns no rows, so a collapsed terminal body never forces source traversal allocation.
pub(crate) fn visible_rows<'a>(
    groups: &[VisibleGroup<'a>],
    offset: usize,
    height: usize,
) -> Vec<UnifiedDiffRow<'a>> {
    if height == 0 {
        return Vec::new();
    }

    let mut skipped = 0usize;
    let mut visible = Vec::with_capacity(height);
    for group in groups {
        let hunk = match group {
            VisibleGroup::Folded(run) => {
                append_visible_row(
                    &mut visible,
                    UnifiedDiffRow::Folded(*run),
                    offset,
                    height,
                    &mut skipped,
                );
                if visible.len() == height {
                    break;
                }
                continue;
            }
            VisibleGroup::Hunk(hunk) => hunk,
        };
        let header = UnifiedDiffRow::HunkHeader {
            hunk_id: &hunk.id,
            old_range: hunk.old_range,
            new_range: hunk.new_range,
            heading: hunk.heading.as_deref(),
        };
        append_visible_row(&mut visible, header, offset, height, &mut skipped);
        if visible.len() == height {
            break;
        }

        for line in &hunk.lines {
            append_visible_row(
                &mut visible,
                UnifiedDiffRow::Source(line),
                offset,
                height,
                &mut skipped,
            );
            if visible.len() == height {
                break;
            }
        }
        if visible.len() == height {
            break;
        }
    }
    visible
}

fn append_visible_row<'a>(
    visible: &mut Vec<UnifiedDiffRow<'a>>,
    row: UnifiedDiffRow<'a>,
    offset: usize,
    height: usize,
    skipped: &mut usize,
) {
    if *skipped >= offset {
        visible.push(row);
    }
    *skipped += 1;
    debug_assert!(visible.len() <= height);
}

/// Clips a source line by terminal cells without splitting a wide glyph.
pub(crate) fn clip_terminal_cells(text: &str, offset: usize, width: usize) -> String {
    let mut skipped = 0usize;
    let mut visible_width = 0usize;
    let mut clipped = String::new();

    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if skipped < offset {
            skipped = skipped.saturating_add(character_width);
            continue;
        }
        if character_width == 0 && clipped.is_empty() {
            continue;
        }
        if visible_width.saturating_add(character_width) > width {
            break;
        }
        clipped.push(character);
        visible_width = visible_width.saturating_add(character_width);
    }
    clipped
}

/// Clips `text` to `width` terminal cells, appending an ellipsis (`…`) when it truncates.
///
/// Unlike [`clip_terminal_cells`] (offset-based horizontal scrolling for diff source), this is
/// for fixed-width labels/titles: it never scrolls and signals truncation with a trailing `…`
/// rather than silently cutting mid-word. Returns the text unchanged when it already fits.
pub(crate) fn clip_terminal_cells_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let full_width: usize =
        text.chars().map(|character| UnicodeWidthChar::width(character).unwrap_or(0)).sum();
    if full_width <= width {
        return text.to_string();
    }
    let mut clipped = clip_terminal_cells(text, 0, width.saturating_sub(1));
    clipped.push('…');
    clipped
}

/// Returns the readable, color-independent marker for one source line.
pub(crate) const fn marker(kind: ReviewLineKind) -> char {
    match kind {
        ReviewLineKind::Context => ' ',
        ReviewLineKind::Deletion => '-',
        ReviewLineKind::Addition => '+',
        ReviewLineKind::NoNewlineMarker => '\\',
    }
}

#[cfg(test)]
mod tests;
