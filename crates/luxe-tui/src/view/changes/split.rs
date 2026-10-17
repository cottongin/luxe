//! Changes split — the diff side by side: old on the left, new on the right.
//!
//! The same row space as the unified list, so toggling the layout keeps the reader where they were,
//! and each column's code published on its own side so a selection stays in the file it began in.

use super::cells::{code_pieces, CodeCell};
use super::diff::{hunk_header_line, DrawnRows};
use crate::changes::ChangesView;
use crate::guided_review::diff::SplitDiffCell;
use crate::mouse::PeekSide;
use crate::view::diff_lines::{SplitLayout, SplitSide};
use luxe_core::guided_review::ReviewFile;
use ratatui::text::Line;

/// The side-by-side view from `offset`, `viewport` rows of it.
pub(super) fn split_lines(
    file: &ReviewFile,
    layout: &SplitLayout<'_>,
    view: &ChangesView,
    offset: usize,
    viewport: usize,
) -> DrawnRows {
    use crate::guided_review::diff::{visible_split_rows, SplitRow};
    use crate::guided_review::VisibleGroup;

    let groups: Vec<VisibleGroup<'_>> = file.hunks.iter().map(VisibleGroup::Hunk).collect();
    let rows = visible_split_rows(&groups, offset, viewport);
    let mut drawn: DrawnRows = (Vec::with_capacity(viewport), Vec::with_capacity(viewport));
    for row in &rows {
        if drawn.0.len() >= viewport {
            break;
        }
        match row {
            // A collapsed run cannot happen here: the pane folds nothing.
            SplitRow::Folded { .. } => {}
            SplitRow::Header { old_range, new_range, heading, .. } => {
                if !drawn.0.is_empty() {
                    drawn.0.push(Line::default());
                    drawn.1.push(Vec::new());
                }
                let header =
                    hunk_header_line(*old_range, *new_range, *heading, layout.styles, None);
                drawn.0.push(header);
                drawn.1.push(Vec::new());
            }
            SplitRow::Pair { old, new, unified_index, last_index, .. } => {
                // A pair's first row is its old line and its last the new one; a context line, or
                // a line with nothing beside it, is one row and both
                let rows = (*unified_index, *last_index);
                let pair = SplitPair { old: *old, new: *new, rows, wrap: view.word_wrap };
                push_pair(&mut drawn, layout, &pair);
            }
        }
    }
    drawn.0.truncate(viewport);
    drawn.1.truncate(viewport);
    drawn
}

/// One row of a side-by-side diff: the old line and the new one, either possibly missing.
struct SplitPair<'a> {
    old: SplitDiffCell<'a>,
    new: SplitDiffCell<'a>,
    /// The old and the new line's rows in the pane's row space
    rows: (usize, usize),
    wrap: bool,
}

/// Draws one side-by-side pair, wrapped or clipped, with where each side's code sits.
fn push_pair(drawn: &mut DrawnRows, layout: &SplitLayout<'_>, pair: &SplitPair<'_>) {
    let plain = crate::view::diff_lines::SplitCellStyle::default();
    // Pairing is exact here, so the marks are too — no run-matching heuristic.
    let changed = match (pair.old, pair.new) {
        (SplitDiffCell::Source(old), SplitDiffCell::Source(new)) if old.kind != new.kind => {
            luxe_core::intraline::intraline_change(
                &crate::tabstop::display_text(&old.text),
                &crate::tabstop::display_text(&new.text),
            )
        }
        _ => luxe_core::intraline::IntralineChange::default(),
    };
    let old = side_pieces(pair.old, (SplitSide::Old, pair.rows.0), layout, pair.wrap);
    let new = side_pieces(pair.new, (SplitSide::New, pair.rows.1), layout, pair.wrap);
    let lines = match pair.wrap {
        true => layout
            .wrapped_pair((pair.old, &changed.removed, &plain), (pair.new, &changed.added, &plain)),
        false => {
            let mut spans = layout.cell_spans(SplitSide::Old, pair.old, &changed.removed, &plain);
            spans.push(layout.divider());
            spans.extend(layout.cell_spans(SplitSide::New, pair.new, &changed.added, &plain));
            vec![Line::from(spans)]
        }
    };
    for (index, line) in lines.into_iter().enumerate() {
        drawn.0.push(line);
        drawn.1.push(old.get(index).into_iter().chain(new.get(index)).copied().collect());
    }
}

/// Where the code of one side of a pair sits on each row it wraps to.
fn side_pieces(
    cell: SplitDiffCell<'_>,
    (side, row): (SplitSide, usize),
    layout: &SplitLayout<'_>,
    wrap: bool,
) -> Vec<CodeCell> {
    let SplitDiffCell::Source(line) = cell else { return Vec::new() };
    let (x, peek_side) = match side {
        SplitSide::Old => (layout.continuation(), PeekSide::Old),
        SplitSide::New => (
            layout.side + crate::view::diff_lines::SPLIT_DIVIDER_CELLS + layout.continuation(),
            PeekSide::New,
        ),
    };
    code_pieces(&line.text, row, peek_side, (x, layout.source.max(1)), wrap)
}
