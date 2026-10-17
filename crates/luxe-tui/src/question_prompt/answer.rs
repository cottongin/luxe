//! Answer — the typed answer while it is being written: its rows, its selection, and the caret.
//!
//! The answer is the composer's [`crate::editor::Editor`], so it is drawn from the editor's own
//! wrap: the rows the reader sees are the rows up and down move through.

use super::QuestionPromptState;
use crate::editor::{Editor, TextLayout, VisualSegment};
use ratatui::style::Style;
use ratatui::text::Span;

/// The glyph drawn where typing lands.
const CARET: &str = "▏";

/// The rows of the answer being typed that fit in `max_rows`, the caret drawn where typing lands,
/// and whether rows above were scrolled off.
///
/// Each row keeps a cell for the caret, and a scrolled answer one more for the mark that says so.
/// The width it wrapped at is recorded on the state, so up and down move through these rows.
pub(super) fn editing_rows(
    state: &QuestionPromptState,
    room: usize,
    max_rows: usize,
    (text, selected): (Style, Style),
) -> (Vec<Vec<Span<'static>>>, bool) {
    let editor = state.answer();
    let max_rows = max_rows.max(1);
    let (width, layout) = fit(editor, room, max_rows);
    state.set_answer_width(width);
    let caret = editor.cursor_in(&layout);
    // The window follows the caret, so the row being typed in is always on screen
    let first = caret.row.saturating_sub(max_rows - 1);
    let rows = layout.rows()[first..]
        .iter()
        .take(max_rows)
        .enumerate()
        .map(|(offset, segments)| {
            let at = (first + offset == caret.row).then_some(caret.column);
            row_spans(segments, at, text, selected)
        })
        .collect();
    (rows, first > 0)
}

/// The width the answer wraps at, and its wrap: a cell short of `room` for the caret, and one more
/// when it takes more than `max_rows` and needs the scrolled mark.
fn fit(editor: &Editor, room: usize, max_rows: usize) -> (u16, TextLayout) {
    let cells = |width: usize| u16::try_from(width.max(1)).unwrap_or(u16::MAX);
    let width = cells(room.saturating_sub(1));
    let layout = editor.layout(width);
    if layout.row_count() <= max_rows {
        return (width, layout);
    }
    let width = cells(room.saturating_sub(2));
    (width, editor.layout(width))
}

/// One row's spans, the selection lit and the caret set before the cell at column `caret`.
fn row_spans(
    segments: &[VisualSegment],
    caret: Option<usize>,
    text: Style,
    selected: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut column = 0usize;
    for segment in segments {
        let style = if segment.is_selected { text.patch(selected) } else { text };
        let mut run = String::new();
        for character in segment.text.chars() {
            if caret == Some(column) {
                spans.push(Span::styled(std::mem::take(&mut run), style));
                spans.push(Span::styled(CARET, text));
            }
            run.push(character);
            column += unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        }
        spans.push(Span::styled(run, style));
    }
    if caret.is_some_and(|at| at >= column) {
        spans.push(Span::styled(CARET, text));
    }
    spans.retain(|span| !span.content.is_empty());
    spans
}
