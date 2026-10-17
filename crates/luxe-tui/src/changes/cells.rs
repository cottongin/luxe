//! Cells — text picked with the mouse in the Changes pane, by the character rather than the line.
//!
//! The pane's rows are the file's lines in the File view, and in the diff a hunk's header followed
//! by its lines. A selection is held in those rows, with columns measured across the whole line, so
//! scrolling and wrapping only change where a row is drawn and cannot move what is picked. The copy
//! is cut from the source itself, so a tab comes back a tab.

use super::{ChangesView, PeekView, TextLoad};
use crate::mouse::PeekSide;
use crate::selection::{GridSelection, SelectionSpan, TextPoint};
use luxe_core::guided_review::{ReviewFile, ReviewLine, ReviewLineKind};
use std::ops::{Range, RangeInclusive};

/// A selection in the pane, with the face and the side of the file it was made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellPick {
    /// Anchor and cursor, as rows of the pane and columns of the whole line
    pub(crate) grid: GridSelection,
    /// The face its rows belong to; on the other face it means nothing and is not drawn
    pub(crate) view: PeekView,
    /// The column of a side-by-side diff it runs down, or `Whole`
    pub(crate) side: PeekSide,
}

impl CellPick {
    /// The columns it covers of `row`, a line `width` cells wide, when that row is drawn on
    /// `view`'s `side`.
    pub(crate) fn columns_on(
        &self,
        view: PeekView,
        side: PeekSide,
        row: usize,
        width: usize,
    ) -> Option<Range<usize>> {
        if self.view != view || self.side != side {
            return None;
        }
        self.grid.columns_on(row, width)
    }
}

/// The diff's rows in order: `None` for a hunk's header, then that hunk's lines.
///
/// The row space the diff renderer scrolls in, so a row index means the same line to both.
pub(crate) fn diff_row_lines(file: &ReviewFile) -> impl Iterator<Item = Option<&ReviewLine>> {
    file.hunks.iter().flat_map(|hunk| std::iter::once(None).chain(hunk.lines.iter().map(Some)))
}

/// Whether `side` draws a line of `kind`: the old file has no additions, the new one no deletions.
fn shows(side: PeekSide, kind: ReviewLineKind) -> bool {
    match side {
        PeekSide::Whole => true,
        PeekSide::Old => kind != ReviewLineKind::Addition,
        PeekSide::New => kind != ReviewLineKind::Deletion,
    }
}

impl ChangesView {
    /// Starts a selection at `point`. Drops the line pick, so only one selection shows at a time.
    pub(crate) fn begin_cells(&mut self, point: TextPoint, side: PeekSide, span: SelectionSpan) {
        self.selection = None;
        self.visual = false;
        let grid = match span {
            SelectionSpan::Caret => GridSelection::collapsed(point),
            SelectionSpan::Word => {
                self.word_at(point, side).unwrap_or(GridSelection::collapsed(point))
            }
            SelectionSpan::Line => GridSelection::whole_line(point.line),
        };
        self.cells = Some(CellPick { grid, view: self.view, side });
    }

    /// Moves the selection's free end to `point`.
    pub(crate) fn extend_cells(&mut self, point: TextPoint) {
        if let Some(pick) = self.cells.as_mut() {
            pick.grid.cursor = point;
        }
    }

    /// Whether the selection covers any text, rather than marking where a click landed.
    pub(crate) fn cells_picked(&self) -> bool {
        self.cells.is_some_and(|pick| !pick.grid.is_empty())
    }

    /// The selected text, cut from the source. Rows the selection's side does not draw are left
    /// out, so a drag down the new column of a side-by-side diff copies only new code.
    pub(crate) fn cells_text(&self) -> Option<String> {
        let pick = self.cells.filter(|pick| !pick.grid.is_empty() && pick.view == self.view)?;
        let (start, end) = pick.grid.bounds();
        let rows = self.row_sources(start.line..=end.line, pick.side);
        let picked: Vec<&str> = rows
            .into_iter()
            .enumerate()
            .filter_map(|(offset, source)| {
                let source = source?;
                let width = crate::tabstop::display_width(source);
                let columns = pick.grid.columns_on(start.line + offset, width);
                Some(columns.map_or("", |columns| crate::tabstop::source_slice(source, columns)))
            })
            .collect();
        Some(picked.join("\n"))
    }

    /// Sets the line pick a right-click's menu acts on. A pick already holding `line` stays; a click
    /// on selected text means the lines the text spans; anything else means the line clicked.
    pub(crate) fn pick_for_menu(&mut self, line: u32) {
        let holds = |(first, last): (u32, u32)| (first..=last).contains(&line);
        if self.selected_lines().is_some_and(holds) {
            return;
        }
        let spanned = self.cells_lines().filter(|lines| holds(*lines));
        self.selection = Some(spanned.unwrap_or((line, line)));
    }

    /// The File view lines the selected text spans, 1-based.
    fn cells_lines(&self) -> Option<(u32, u32)> {
        let pick =
            self.cells.filter(|pick| pick.view == PeekView::File && !pick.grid.is_empty())?;
        let (start, end) = pick.grid.bounds();
        let number = |row: usize| u32::try_from(row + 1).unwrap_or(u32::MAX);
        Some((number(start.line), number(end.line)))
    }

    /// The word under `point`, as a selection of it.
    fn word_at(&self, point: TextPoint, side: PeekSide) -> Option<GridSelection> {
        let source = self.row_sources(point.line..=point.line, side).into_iter().next()??;
        let word =
            crate::selection::word_columns(&crate::tabstop::display_text(source), point.column)?;
        Some(GridSelection {
            anchor: TextPoint::new(point.line, word.start),
            cursor: TextPoint::new(point.line, word.end),
        })
    }

    /// The source text of `rows`, one entry per row, `None` where `side` draws no source there.
    fn row_sources(&self, rows: RangeInclusive<usize>, side: PeekSide) -> Vec<Option<&str>> {
        let (skip, take) = (*rows.start(), rows.end().saturating_sub(*rows.start()) + 1);
        match self.view {
            PeekView::File => match &self.text {
                TextLoad::Ready(file) => {
                    file.text.lines().skip(skip).take(take).map(Some).collect()
                }
                _ => Vec::new(),
            },
            PeekView::Diff => self.file().map_or_else(Vec::new, |file| {
                diff_row_lines(file)
                    .skip(skip)
                    .take(take)
                    .map(|line| {
                        line.filter(|line| shows(side, line.kind)).map(|line| line.text.as_str())
                    })
                    .collect()
            }),
        }
    }
}
