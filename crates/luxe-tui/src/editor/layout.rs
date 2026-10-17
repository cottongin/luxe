//! Layout — soft-wrapping the draft into visual rows, and the cursor and selection on them.

use super::parts::PromptPart;
use super::Editor;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisualSegment {
    pub(crate) text: String,
    pub(crate) is_chip: bool,
    /// Covered by the active selection (mouse drag or shift-select).
    pub(crate) is_selected: bool,
    /// Part of a slash command the composer lights, when it is
    pub(crate) mark: Option<crate::slash_args::Mark>,
}

/// Cursor location in the wrapped editor viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualCursor {
    /// Zero-based visual row
    pub row: usize,
    /// Zero-based terminal-cell column
    pub column: usize,
}

#[derive(Debug, Clone, Copy)]
struct CursorPoint {
    offset: usize,
    row: usize,
    column: usize,
}

/// One wrap of the draft: its rows, the styled runs on each, and where every offset landed.
///
/// Handed out whole rather than re-derived per question, because the composer asks three — how
/// tall am I, what do I draw, where is the caret — and each would otherwise walk the draft again.
#[derive(Debug)]
pub(crate) struct TextLayout {
    lines: Vec<String>,
    segments: Vec<Vec<VisualSegment>>,
    points: Vec<CursorPoint>,
}

impl TextLayout {
    /// Visual rows the draft occupies. Never zero: an empty draft still has a row to type into.
    pub(crate) fn row_count(&self) -> usize {
        self.lines.len().max(1)
    }

    /// The styled runs on each row, in order.
    pub(crate) fn rows(&self) -> &[Vec<VisualSegment>] {
        &self.segments
    }
}

/// One laid-out character: where it came from, what it draws as, and the cells it consumes.
struct LaidCell<'a> {
    offset: usize,
    len: usize,
    /// The cell's glyphs, borrowed from the draft. Owned only by a tab, which becomes spaces.
    text: std::borrow::Cow<'a, str>,
    /// Width the character wants; a tab's depends on the column it lands in.
    natural_width: usize,
    /// Width it actually consumed (clamped when whitespace hangs past the wrap column).
    width: usize,
    is_chip: bool,
    is_selected: bool,
    mark: Option<crate::slash_args::Mark>,
    is_tab: bool,
    /// Whitespace: a soft wrap may break right after it.
    breakable: bool,
}

/// A visual row under construction: the source offset it starts at plus its characters.
struct LaidRow<'a> {
    start: usize,
    cells: Vec<LaidCell<'a>>,
}

/// Cells a laid row consumes, after tab expansion and wrap-column clamping.
fn row_width(row: &LaidRow<'_>) -> usize {
    row.cells.iter().map(|cell| cell.width).sum()
}

/// Soft-wrap `text` to `width`, breaking at word boundaries.
///
/// A word that would overflow moves to the next row whole; only a word longer than the whole row
/// is split mid-character (nothing to wrap it to). Whitespace at a wrap column hangs off the end
/// rather than being orphaned at the head of the next row — it draws blank and the composer's
/// paragraph clips it.
///
/// A final row filled to its last cell gets an empty row after it. The caret has to live
/// somewhere once a row is full, and clamping it onto the last glyph both hides the caret and
/// defers the composer's growth until the *next* word actually moves down — which reads as the
/// box expanding a keystroke late, with the first line scrolled out of view in the meantime.
fn layout_text(
    text: &str,
    width: u16,
    parts: &[PromptPart],
    selection: Option<std::ops::Range<usize>>,
    marks: &[(std::ops::Range<usize>, crate::slash_args::Mark)],
) -> TextLayout {
    let width = usize::from(width.max(1));
    let mut rows = vec![LaidRow { start: 0, cells: Vec::new() }];
    let mut column = 0usize;

    for (offset, character) in text.char_indices() {
        if character == '\n' {
            rows.push(LaidRow { start: offset + 1, cells: Vec::new() });
            column = 0;
            continue;
        }

        let natural_width =
            if character == '\t' { 4 - (column % 4) } else { character.width().unwrap_or(0) };
        let overflows = column > 0 && column + natural_width > width;
        if overflows && !character.is_whitespace() {
            // Carry the word this character belongs to onto the next row, unless the row is one
            // long word (nothing to break at) or already ends in whitespace.
            let current = rows.last_mut().expect("rows is never empty");
            let split =
                current.cells.iter().rposition(|cell| cell.breakable).map_or(0, |at| at + 1);
            let carried = if split == 0 || split == current.cells.len() {
                Vec::new()
            } else {
                current.cells.split_off(split)
            };
            let start = carried.first().map_or(offset, |cell| cell.offset);
            rows.push(LaidRow { start, cells: Vec::new() });
            column = 0;
            for cell in carried {
                push_cell(rows.last_mut().expect("row just pushed"), &mut column, cell, width);
            }
        }

        let cell = LaidCell {
            offset,
            len: character.len_utf8(),
            text: std::borrow::Cow::Borrowed(&text[offset..offset + character.len_utf8()]),
            natural_width,
            width: natural_width,
            is_chip: parts.iter().any(|part| offset >= part.start && offset < part.end),
            is_selected: selection.as_ref().is_some_and(|range| range.contains(&offset)),
            mark: marks.iter().find(|(range, _)| range.contains(&offset)).map(|(_, mark)| *mark),
            is_tab: character == '\t',
            breakable: character.is_whitespace(),
        };
        push_cell(rows.last_mut().expect("rows is never empty"), &mut column, cell, width);
    }

    if rows.last().is_some_and(|row| row_width(row) >= width) {
        rows.push(LaidRow { start: text.len(), cells: Vec::new() });
    }

    let mut lines = Vec::with_capacity(rows.len());
    let mut segments = Vec::with_capacity(rows.len());
    let mut points: Vec<CursorPoint> = Vec::new();
    for (row, laid) in rows.iter().enumerate() {
        let mut line = String::new();
        let mut row_segments = Vec::new();
        push_point(&mut points, CursorPoint { offset: laid.start, row, column: 0 });
        let mut column = 0;
        for cell in &laid.cells {
            line.push_str(&cell.text);
            push_visual_segment(&mut row_segments, cell);
            column += cell.width;
            push_point(&mut points, CursorPoint { offset: cell.offset + cell.len, row, column });
        }
        lines.push(line);
        segments.push(row_segments);
    }

    TextLayout { lines, segments, points }
}

/// Append `cell` to a row, expanding a tab against its final column and clamping the row to
/// `width` (only reached by whitespace hanging past the wrap column).
fn push_cell<'a>(row: &mut LaidRow<'a>, column: &mut usize, mut cell: LaidCell<'a>, width: usize) {
    if cell.is_tab {
        cell.natural_width = 4 - (*column % 4);
        cell.text = std::borrow::Cow::Owned(" ".repeat(cell.natural_width));
    }
    cell.width = cell.natural_width.min(width.saturating_sub(*column));
    *column += cell.width;
    row.cells.push(cell);
}

/// Record a cursor position, letting a later row claim a boundary offset from the row before it —
/// so the caret sits at the head of the wrapped row rather than off the right edge of the one
/// above.
fn push_point(points: &mut Vec<CursorPoint>, point: CursorPoint) {
    match points.last_mut() {
        Some(last) if last.offset == point.offset => *last = point,
        _ => points.push(point),
    }
}

fn push_visual_segment(segments: &mut Vec<VisualSegment>, cell: &LaidCell<'_>) {
    let LaidCell { is_chip, is_selected, mark, .. } = *cell;
    match segments.last_mut().filter(|segment| {
        segment.is_chip == is_chip && segment.is_selected == is_selected && segment.mark == mark
    }) {
        Some(segment) => segment.text.push_str(&cell.text),
        None => {
            segments.push(VisualSegment { text: cell.text.to_string(), is_chip, is_selected, mark })
        }
    }
}

fn point_for_offset(points: &[CursorPoint], offset: usize) -> Option<CursorPoint> {
    points.iter().find(|point| point.offset == offset).copied()
}

impl Editor {
    /// Updates the width used by vertical cursor navigation.
    pub fn set_viewport_width(&mut self, width: u16) {
        self.viewport_width = width.max(1);
    }

    /// Soft-wrapped display lines at `width`. The composer reads its rows off a [`TextLayout`]
    /// instead; this is how the wrap's own invariants are checked.
    #[cfg(test)]
    pub(crate) fn visual_lines(&self, width: u16) -> Vec<String> {
        self.layout(width).lines
    }

    /// Moves the cursor to a visual cell and starts a selection there (mouse press).
    pub fn begin_selection_at_visual(&mut self, row: usize, column: usize) {
        self.place_cursor_at_visual(row, column);
        self.begin_selection();
    }

    /// Selects the word at a visual cell (a double-click), or just places the cursor there when the
    /// cell holds no word.
    pub fn select_word_at_visual(&mut self, row: usize, column: usize) {
        self.place_cursor_at_visual(row, column);
        match crate::selection::word_bytes(&self.buffer, self.cursor) {
            Some(word) => {
                self.selection_anchor = Some(word.start);
                self.cursor = word.end;
            }
            None => self.begin_selection(),
        }
        self.preferred_column = None;
    }

    /// Selects a whole visual (wrapped) row — a triple-click.
    ///
    /// The *visual* row rather than the logical line, because the grid is what the reader can see,
    /// and it is the rule the transcript's own triple-click follows.
    pub fn select_visual_line(&mut self, row: usize) {
        let layout = self.layout(self.viewport_width);
        let Some(max_row) = layout.points.iter().map(|point| point.row).max() else {
            return;
        };
        let target = row.min(max_row);
        let mut offsets =
            layout.points.iter().filter(|point| point.row == target).map(|point| point.offset);
        let Some(first) = offsets.next() else {
            return;
        };
        self.selection_anchor = Some(first);
        self.cursor = offsets.next_back().unwrap_or(first);
        self.preferred_column = None;
    }

    /// Extends the active selection to a visual cell (mouse drag), starting one if needed.
    pub fn extend_selection_to_visual(&mut self, row: usize, column: usize) {
        let anchor = self.selection_anchor.unwrap_or(self.cursor);
        self.place_cursor_at_visual(row, column);
        self.selection_anchor = Some(anchor);
    }

    /// Places the cursor at a visual (wrapped) row/column, snapping to the nearest character on
    /// that row — the inverse of `visual_cursor`, used by a mouse click in the composer. A row past
    /// the end clamps to the last visual row; laid out at the current viewport width.
    pub fn place_cursor_at_visual(&mut self, row: usize, column: usize) {
        let layout = self.layout(self.viewport_width);
        let Some(max_row) = layout.points.iter().map(|point| point.row).max() else {
            return;
        };
        let target_row = row.min(max_row);
        if let Some(point) = layout
            .points
            .iter()
            .filter(|point| point.row == target_row)
            .min_by_key(|point| point.column.abs_diff(column))
        {
            self.cursor = self.snap_out_of_part(point.offset, point.column >= column);
        }
        self.preferred_column = None;
    }

    #[cfg(test)]
    pub(crate) fn visual_segments(&self, width: u16) -> Vec<Vec<VisualSegment>> {
        self.layout(width).segments
    }

    /// Returns the cursor's visual row and terminal-cell column.
    pub fn visual_cursor(&self, width: u16) -> VisualCursor {
        self.cursor_in(&self.layout(width))
    }

    /// The cursor's row and column within a wrap already computed for this frame.
    pub(crate) fn cursor_in(&self, layout: &TextLayout) -> VisualCursor {
        let point = point_for_offset(&layout.points, self.cursor).unwrap_or(CursorPoint {
            offset: 0,
            row: 0,
            column: 0,
        });
        VisualCursor { row: point.row, column: point.column }
    }

    /// Visual rows the draft needs at `width`. Same note as [`Self::visual_lines`].
    #[cfg(test)]
    pub(crate) fn visual_line_count(&self, width: u16) -> usize {
        self.layout(width).row_count()
    }

    /// Wraps the draft to `width`. One call answers everything the composer draws itself from.
    pub(crate) fn layout(&self, width: u16) -> TextLayout {
        layout_text(&self.buffer, width, &self.parts, self.selection(), &[])
    }

    /// [`Self::layout`], with stretches of the draft marked for the composer to light: a slash
    /// command and the words of its that the draft spells out ([`crate::slash_args::marks`]).
    /// Marks change only how a run is drawn, never where the draft wraps.
    pub(crate) fn layout_marked(
        &self,
        width: u16,
        marks: &[(std::ops::Range<usize>, crate::slash_args::Mark)],
    ) -> TextLayout {
        layout_text(&self.buffer, width, &self.parts, self.selection(), marks)
    }

    /// Whether the cursor sits on the draft's first visual row, so `↑` has no line above to reach.
    pub(crate) fn cursor_on_first_row(&self) -> bool {
        let layout = self.layout(self.viewport_width);
        point_for_offset(&layout.points, self.cursor).is_none_or(|point| point.row == 0)
    }

    /// Whether the cursor sits on the draft's last visual row, so `↓` has no line below to reach.
    pub(crate) fn cursor_on_last_row(&self) -> bool {
        let layout = self.layout(self.viewport_width);
        point_for_offset(&layout.points, self.cursor)
            .is_none_or(|point| point.row + 1 >= layout.lines.len())
    }

    pub(super) fn move_up(&mut self) {
        let layout = self.layout(self.viewport_width);
        let Some(current) = point_for_offset(&layout.points, self.cursor) else {
            return;
        };
        if current.row > 0 {
            self.move_to_visual_row(&layout, current.row - 1, current.column);
        } else if self.cursor == 0 {
            self.history_previous();
        } else {
            self.cursor = 0;
            self.preferred_column = None;
        }
    }

    pub(super) fn move_down(&mut self) {
        let layout = self.layout(self.viewport_width);
        let Some(current) = point_for_offset(&layout.points, self.cursor) else {
            return;
        };
        if current.row + 1 < layout.lines.len() {
            self.move_to_visual_row(&layout, current.row + 1, current.column);
        } else if self.cursor == self.buffer.len() {
            self.history_next();
        } else {
            self.cursor = self.buffer.len();
            self.preferred_column = None;
        }
    }

    fn move_to_visual_row(&mut self, layout: &TextLayout, row: usize, current_column: usize) {
        let target_column = self.preferred_column.unwrap_or(current_column);
        self.preferred_column = Some(target_column);
        if let Some(point) = layout
            .points
            .iter()
            .filter(|point| point.row == row)
            .min_by_key(|point| point.column.abs_diff(target_column))
        {
            self.cursor = self.snap_out_of_part(point.offset, point.column >= target_column);
        }
    }
}
