//! Side-by-side card diff — old on the left, new on the right.
//!
//! Worth it only on a card wide enough to give each column a readable width; [`super::layout_for`]
//! decides that and falls back to unified below it. Two cramped columns are worse than one honest
//! one in every way.
//!
//! Both columns wrap and the shorter one is padded, so a long line stays visible and the rule holds
//! the same screen column on every row. OpenCode's side-by-side clipped instead, which on real code
//! meant the end of every line simply wasn't there.
//!
//! Pairing is exact here — a cell knows its counterpart — so the intra-line marks are exact with it,
//! without the positional run-matching the unified layout has to do.

use super::rows::{Row, RowKind, SourceRow};
use super::{CardContext, DiffRow, RowLines};
use crate::view::emphasis;
use luxe_core::intraline::IntralineChange;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// The rule between the two columns.
const DIVIDER: &str = " │ ";

/// One laid-out row: a pair of cells, or something that spans both columns.
enum SplitRow<'a> {
    Pair {
        old: Option<&'a SourceRow>,
        new: Option<&'a SourceRow>,
    },
    /// A note or an elision belongs to neither side.
    FullWidth(&'a Row),
}

impl SplitRow<'_> {
    /// Whether this row carries part of the change, on either side of the rule.
    fn is_change(&self) -> bool {
        let changed =
            |cell: Option<&SourceRow>| cell.is_some_and(|source| source.kind != RowKind::Context);
        match self {
            Self::Pair { old, new } => changed(*old) || changed(*new),
            Self::FullWidth(_) => false,
        }
    }
}

/// Cells each column gets on a card `width` wide, after the rule between them.
///
/// Shared with the layout gate, so it decides against the same arithmetic the renderer lays out with.
/// That gate is also what guarantees a column can seat its own gutter: below
/// [`luxe_core::config::SPLIT_FLOOR_COLUMN`] there is no split to lay out, and this renderer is
/// never asked.
pub(super) fn side_width(width: u16) -> usize {
    usize::from(width).saturating_sub(DIVIDER.width()) / 2
}

pub(super) fn render(rows: &[Row], language: Option<&str>, ctx: &CardContext<'_>) -> Vec<DiffRow> {
    let number_width = super::rows::number_width(rows);
    let side_width = side_width(ctx.width);
    // sign + number + the space before the code.
    let gutter_width = number_width + 2;
    let code_width = super::code_width(side_width, gutter_width);

    pair_rows(rows)
        .into_iter()
        .map(|row| {
            let changed = row.is_change();
            let lines = match row {
                SplitRow::FullWidth(Row::Note(text)) => super::note_lines(text, ctx),
                SplitRow::FullWidth(_) => vec![Line::from(Span::styled(
                    format!(
                        "{:>gutter_width$}{}",
                        crate::glyphs::Mark::Elision.glyph(),
                        " ".repeat(side_width.saturating_sub(gutter_width))
                    ),
                    ctx.styles.muted,
                ))],
                SplitRow::Pair { old, new } => {
                    let emphasis = pair_emphasis(old, new);
                    let left = cell(old, &emphasis.removed, language, ctx);
                    let right = cell(new, &emphasis.added, language, ctx);
                    pair_lines(left, right, code_width, gutter_width, side_width, ctx)
                }
            };
            DiffRow { lines, changed }
        })
        .collect()
}

/// One column's ingredients: its gutter, its code, and the wash its row wears.
struct Cell {
    gutter: Option<Span<'static>>,
    code: Vec<Span<'static>>,
    background: Option<Style>,
}

fn cell(
    source: Option<&SourceRow>,
    changed: &[std::ops::Range<usize>],
    language: Option<&str>,
    ctx: &CardContext<'_>,
) -> Cell {
    let Some(source) = source else {
        // The empty counterpart still occupies its column, or the rule would wander row to row.
        return Cell { gutter: None, code: Vec::new(), background: None };
    };
    let style = match source.kind {
        RowKind::Added => ctx.styles.success,
        RowKind::Removed => ctx.styles.error,
        RowKind::Context => ctx.styles.text,
    };
    let gutter = Span::styled(
        format!(
            "{}{} ",
            source.kind.marker(),
            source.number.map_or_else(String::new, |number| number.to_string())
        ),
        ctx.styles.muted,
    );
    let code = match source.kind {
        RowKind::Context => super::highlighted_spans(&source.code, language, ctx),
        _ => emphasis::spans(
            &source.code,
            changed,
            style,
            ctx.styles.diff_emphasis(style, source.kind == RowKind::Added),
        ),
    };
    // Without the sign column doing the talking, the wash is what tells the two sides apart at a
    // glance — and what makes an absent counterpart visibly absent.
    let background = match source.kind {
        RowKind::Added => Some(ctx.styles.diff_add_bg),
        RowKind::Removed => Some(ctx.styles.diff_del_bg),
        RowKind::Context => None,
    };
    Cell { gutter: Some(gutter), code, background }
}

/// Lays one pair out across both columns, padding the shorter side so the rule stays put.
fn pair_lines(
    old: Cell,
    new: Cell,
    code_width: usize,
    gutter_width: usize,
    side_width: usize,
    ctx: &CardContext<'_>,
) -> RowLines {
    let wrap = |cell: &Cell| crate::widgets::wrap_cells(cell.code.clone(), code_width.max(1));
    let (left, right) = (wrap(&old), wrap(&new));
    let height = left.len().max(right.len()).max(1);
    (0..height)
        .map(|index| {
            let mut spans = side(&old, left.get(index), index, gutter_width, side_width, ctx);
            spans.push(Span::styled(DIVIDER, ctx.styles.border_muted));
            spans.extend(side(&new, right.get(index), index, gutter_width, side_width, ctx));
            Line::from(spans)
        })
        .collect()
}

/// One column of one visual line: the gutter on the first, a `↳` on a continuation that has text.
///
/// A pair is as tall as its longer side, so the shorter side runs out of segments partway down. Those
/// lines are padding — they keep the wash so a replaced row reads as one block, but they must not
/// wear a continuation marker for a line that never wrapped.
fn side(
    cell: &Cell,
    segment: Option<&Vec<Span<'static>>>,
    index: usize,
    gutter_width: usize,
    side_width: usize,
    ctx: &CardContext<'_>,
) -> Vec<Span<'static>> {
    let Some(gutter) = cell.gutter.as_ref() else {
        return vec![Span::styled(" ".repeat(side_width), ctx.styles.muted)];
    };
    let mut spans = match (index, segment) {
        (0, _) => vec![Span::styled(format!("{:>gutter_width$}", gutter.content), gutter.style)],
        (_, Some(_)) => {
            vec![Span::styled(format!("{:>gutter_width$}", super::continuation()), ctx.styles.dim)]
        }
        (_, None) => vec![Span::raw(" ".repeat(gutter_width))],
    };
    if let Some(segment) = segment {
        spans.extend(segment.iter().cloned());
    }
    super::pad_to(spans, side_width, cell.background)
}

/// Pairs rows into columns: context on both sides, a replacement zipped, a surplus left one-sided.
fn pair_rows(rows: &[Row]) -> Vec<SplitRow<'_>> {
    let replacements = super::rows::replacements(rows);
    let mut laid_out = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        let replacement =
            replacements.iter().find(|found| found.removed.start == index && !found.is_empty());
        if let Some(replacement) = replacement {
            let source = |at: usize| match &rows[at] {
                Row::Source(source) => Some(source),
                _ => None,
            };
            let (removed, added) = (replacement.removed.clone(), replacement.added.clone());
            // Zip as far as both runs go, then let the surplus stand alone: an outright deletion has
            // nothing on the right, an insertion nothing on the left.
            let height = removed.len().max(added.len());
            for offset in 0..height {
                laid_out.push(SplitRow::Pair {
                    old: removed.clone().nth(offset).and_then(source),
                    new: added.clone().nth(offset).and_then(source),
                });
            }
            index = replacement.added.end.max(replacement.removed.end);
            continue;
        }
        match &rows[index] {
            // Context is the same line on both sides, so the eye can follow it across the rule.
            //
            // The guard reads as a filter and is really a label: `replacements` emits a run for
            // every maximal removal-then-addition stretch, and the jump above always lands on the
            // next run's start, so a changed row is consumed there and never reaches this arm.
            // Relaxing it to `true` is therefore a mutation nothing can catch — it is equivalent,
            // not an untested branch.
            Row::Source(source) if source.kind == RowKind::Context => {
                laid_out.push(SplitRow::Pair { old: Some(source), new: Some(source) });
            }
            row => laid_out.push(SplitRow::FullWidth(row)),
        }
        index += 1;
    }
    laid_out
}

/// The changed spans of each side of a pair, when both sides exist and differ.
fn pair_emphasis(old: Option<&SourceRow>, new: Option<&SourceRow>) -> IntralineChange {
    // Pairing is exact here, so the emphasis is too — no run-matching heuristic needed.
    let (Some(old), Some(new)) = (old, new) else {
        return IntralineChange::default();
    };
    if old.kind == new.kind {
        return IntralineChange::default();
    }
    luxe_core::intraline::intraline_change(&old.code, &new.code)
}

#[cfg(test)]
mod tests;
