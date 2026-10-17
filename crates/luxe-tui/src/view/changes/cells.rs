//! Code cells — where each drawn row's code sits, so the pane's text can be selected and the
//! selection painted.
//!
//! The renderers know which source row a drawn row shows, where its code starts and which stretch of
//! a wrapped line it holds; this turns that into selectable text for the pointer and paints the
//! pick over the cells it covers. Gutters are never published, so a drag over line numbers is left
//! to the line pick and a copy never carries them.

use crate::app::App;
use crate::changes::ChangesView;
use crate::mouse::{PeekSide, TextRow, TextSurface};
use crate::view::transcript::selection::highlight_columns;
use ratatui::{layout::Rect, text::Line};
use std::ops::Range;

/// One drawn piece of a source row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CodeCell {
    /// Row in the pane's row space
    pub(super) row: usize,
    /// Column of a side-by-side diff it sits in, or `Whole`
    pub(super) side: PeekSide,
    /// Cells from the body's left edge to the code
    pub(super) x: u16,
    /// Cells of code the drawn row has room for
    pub(super) width: u16,
    /// Column of the whole line this piece starts at
    pub(super) first_column: usize,
    /// Cells the whole line takes, so a pick running to its end stops there
    pub(super) line_width: usize,
}

/// Where the code of each drawn piece of a source line sits: one piece unwrapped, one per wrapped
/// row otherwise, each `width` cells wide starting `x` cells in.
pub(super) fn code_pieces(
    text: &str,
    row: usize,
    side: PeekSide,
    (x, width): (usize, usize),
    wrap: bool,
) -> Vec<CodeCell> {
    let drawn = crate::tabstop::display_text(text);
    let line_width = crate::selection::cell_width(&drawn);
    let cells = |value: usize| u16::try_from(value).unwrap_or(u16::MAX);
    piece_starts(&drawn, width, wrap)
        .into_iter()
        .map(|first_column| CodeCell {
            row,
            side,
            x: cells(x),
            width: cells(width),
            first_column,
            line_width,
        })
        .collect()
}

/// Columns of the whole line each drawn piece starts at, when `text` wraps at `width` cells.
///
/// The break rule of [`crate::widgets::wrap_cells`], so the pieces line up with what it drew, but
/// counted rather than built: this runs for every row of every frame.
fn piece_starts(text: &str, width: usize, wrap: bool) -> Vec<usize> {
    let mut starts = vec![0];
    if !wrap {
        return starts;
    }
    let (mut column, mut used) = (0usize, 0usize);
    for character in text.chars() {
        let cells = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + cells > width && used > 0 {
            starts.push(column);
            used = 0;
        }
        used += cells;
        column += cells;
    }
    starts
}

/// Publishes every piece as selectable text and paints the pick over the pieces it covers.
///
/// `cells[i]` describes `lines[i]`, the `i`th row down from `body`'s top.
pub(super) fn publish_and_paint(
    app: &App,
    view: &ChangesView,
    lines: &mut [Line<'static>],
    cells: &[Vec<CodeCell>],
    body: Rect,
) {
    let mut hit = app.hit.borrow_mut();
    for (index, (line, pieces)) in lines.iter_mut().zip(cells).enumerate() {
        let y = body.y.saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y >= body.bottom() {
            break;
        }
        for piece in pieces {
            let x = body.x + piece.x;
            let width = piece.width.min(body.right().saturating_sub(x));
            let row = TextRow {
                surface: TextSurface::Peek(piece.side),
                line: piece.row,
                x,
                first_column: piece.first_column,
                pin: None,
            };
            hit.push_text(Rect::new(x, y, width, 1), row);
            if let Some(columns) = drawn_pick(view, piece) {
                *line = highlight_columns(line, columns, app.theme_styles.selected_bg);
            }
        }
    }
}

/// The pick's columns on `piece`, moved from the whole line onto the drawn row.
fn drawn_pick(view: &ChangesView, piece: &CodeCell) -> Option<Range<usize>> {
    let pick = view.cells?;
    let covered = pick.columns_on(view.view, piece.side, piece.row, piece.line_width)?;
    let start = covered.start.max(piece.first_column);
    let end = covered.end.min(piece.first_column + usize::from(piece.width));
    let shift = |column: usize| column - piece.first_column + usize::from(piece.x);
    (start < end).then(|| shift(start)..shift(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrapped line's pieces start where the one before ended; a wide glyph that will not fit
    /// moves whole to the next piece, so that piece starts a cell early.
    #[test]
    fn pieces_start_where_the_wrap_broke() {
        assert_eq!(piece_starts("abcdefgh", 3, true), vec![0, 3, 6]);
        // `ab`, then `日c`, then `d`
        assert_eq!(piece_starts("ab\u{65e5}cd", 3, true), vec![0, 2, 5]);
        assert_eq!(piece_starts("abcdefgh", 3, false), vec![0], "unwrapped is one piece");
        assert_eq!(piece_starts("", 3, true), vec![0], "an empty line is still a row");
    }
}
