//! Markdown tables — a box-drawn grid laid out for the width it is drawn at.
//!
//! A table is built once, when the markdown is walked, and laid out every time the canvas it lands
//! in is measured, the same way a fence's frame is ([`super::FenceRule`]). Laying it out up front
//! at its natural width would break it: a grid wider than the column handed to the transcript's
//! word wrapper splits each row across two lines and scatters the borders, the header and every
//! cell. Opening the preview beside the conversation is enough to do it.
//!
//! So the width is the table's own problem. Columns shrink toward their longest word and cells
//! wrap inside them, row separators appear once a row needs more than one line, and a table too
//! narrow for even that is drawn as records — one `Header: value` line per cell — rather than a
//! grid nobody can read.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Widest a column asks to be once the table has to shrink, so one long cell does not keep the rest
/// of the grid narrow.
const MAX_COLUMN_WIDTH: usize = 48;

/// Narrowest a column is squeezed to while the grid still fits.
const MIN_COLUMN_WIDTH: usize = 3;

/// Longest word a column tries to keep whole when it has to shrink. Past this a word is split
/// anyway: a URL in one cell must not starve every other column.
const MAX_KEPT_WORD: usize = 16;

/// Cells a column needs for its content to be readable, below which the grid gives way to records.
const RECORD_FALLBACK_WIDTH: usize = 6;

/// Column alignment, as pulldown-cmark reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alignment {
    Left,
    Center,
    Right,
}

impl From<pulldown_cmark::Alignment> for Alignment {
    fn from(alignment: pulldown_cmark::Alignment) -> Self {
        match alignment {
            pulldown_cmark::Alignment::Center => Self::Center,
            pulldown_cmark::Alignment::Right => Self::Right,
            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => Self::Left,
        }
    }
}

/// A whole table: its cells, how each column aligns, and the styles it is drawn in.
#[derive(Debug, Clone)]
pub(crate) struct TableBlock {
    header: Vec<String>,
    body: Vec<Vec<String>>,
    alignments: Vec<Alignment>,
    border: Style,
    cell: Style,
}

impl TableBlock {
    /// A table of `header` over `body`.
    ///
    /// Args: `(header, body, alignments, border style, cell style)`.
    pub(crate) fn new(
        header: Vec<String>,
        body: Vec<Vec<String>>,
        alignments: &[pulldown_cmark::Alignment],
        border: Style,
        cell: Style,
    ) -> Option<Self> {
        let trim = |cells: Vec<String>| -> Vec<String> {
            cells.into_iter().map(|cell| cell.trim().to_string()).collect()
        };
        let table = Self {
            header: trim(header),
            body: body.into_iter().map(trim).collect(),
            alignments: alignments.iter().copied().map(Alignment::from).collect(),
            border,
            cell,
        };
        (table.columns() > 0).then_some(table)
    }

    fn columns(&self) -> usize {
        self.header
            .len()
            .max(self.body.iter().map(Vec::len).max().unwrap_or(0))
            .max(self.alignments.len())
    }

    fn rows(&self) -> impl Iterator<Item = &Vec<String>> {
        std::iter::once(&self.header).filter(|header| !header.is_empty()).chain(&self.body)
    }

    fn cell_text(cells: &[String], column: usize) -> &str {
        cells.get(column).map_or("", String::as_str)
    }

    /// The table laid out to fit `width` cells: a grid when one fits, records when none does.
    pub(crate) fn lines_at(&self, width: usize) -> Vec<Line<'static>> {
        match self.column_widths(width) {
            Some(widths) => self.grid(&widths),
            None => self.records(width.max(1)),
        }
    }

    /// Each column's width in a grid at most `width` cells wide, or `None` when no grid fits.
    ///
    /// Every column first asks for all of its content, which is the grid whenever it fits. Short of
    /// that, a column asks for at most [`MAX_COLUMN_WIDTH`]. When those do not fit either,
    /// each keeps its longest word (up to [`MAX_KEPT_WORD`]) and the room left goes to the columns
    /// still short of what they want ([`share`]). When even the words do not fit, the widest
    /// columns give way first ([`fit`]), and with less than [`RECORD_FALLBACK_WIDTH`] a column the
    /// grid is given up.
    fn column_widths(&self, width: usize) -> Option<Vec<usize>> {
        let columns = self.columns();
        // `│` before each column and after the last, and a space either side of every cell
        let budget = width.checked_sub(3 * columns + 1)?;
        let natural: Vec<usize> = (0..columns)
            .map(|column| {
                self.rows()
                    .map(|row| UnicodeWidthStr::width(Self::cell_text(row, column)))
                    .max()
                    .unwrap_or(0)
                    .max(MIN_COLUMN_WIDTH)
            })
            .collect();
        if natural.iter().sum::<usize>() <= budget {
            return Some(natural);
        }
        let wants: Vec<usize> = natural.iter().map(|&width| width.min(MAX_COLUMN_WIDTH)).collect();
        if wants.iter().sum::<usize>() <= budget {
            return Some(wants);
        }
        let words: Vec<usize> = (0..columns)
            .map(|column| {
                let longest = self
                    .rows()
                    .flat_map(|row| Self::cell_text(row, column).split_whitespace())
                    .map(UnicodeWidthStr::width)
                    .max()
                    .unwrap_or(0);
                longest.clamp(MIN_COLUMN_WIDTH, MAX_KEPT_WORD).min(wants[column])
            })
            .collect();
        let floor: usize = words.iter().sum();
        if floor <= budget {
            return Some(share(&words, &wants, budget - floor));
        }
        // Not even every column's longest word fits: take from the widest until it does, which
        // splits the fewest words, down to a floor below which a grid is not worth drawing
        if budget < columns * RECORD_FALLBACK_WIDTH {
            return None;
        }
        Some(fit(words, budget))
    }

    /// The grid: borders, the bold header, and each row, wrapped inside its columns.
    fn grid(&self, widths: &[usize]) -> Vec<Line<'static>> {
        let wrapped_body: Vec<Vec<Vec<String>>> =
            self.body.iter().map(|row| self.wrap_row(row, widths)).collect();
        // Rules between rows only once a row takes more than one line; a one-line row is its own
        // separator, and a ruled grid of short rows is twice as tall for nothing
        let tall = wrapped_body.iter().any(|row| row.len() > 1);
        let mut lines = vec![self.rule(widths, '┌', '┬', '┐')];
        if !self.header.is_empty() {
            let header = self.wrap_row(&self.header, widths);
            let style = self.cell.add_modifier(Modifier::BOLD);
            lines.extend(header.iter().map(|line| self.content_line(line, widths, style)));
            lines.push(self.rule(widths, '├', '┼', '┤'));
        }
        for (index, row) in wrapped_body.iter().enumerate() {
            if tall && index > 0 {
                lines.push(self.rule(widths, '├', '┼', '┤'));
            }
            lines.extend(row.iter().map(|line| self.content_line(line, widths, self.cell)));
        }
        lines.push(self.rule(widths, '└', '┴', '┘'));
        lines
    }

    /// One row's cells wrapped to their columns, as the lines it takes: each line holds every
    /// column's text for that line, empty where a shorter cell has run out.
    fn wrap_row(&self, cells: &[String], widths: &[usize]) -> Vec<Vec<String>> {
        let wrapped: Vec<Vec<String>> = widths
            .iter()
            .enumerate()
            .map(|(column, &width)| wrap(Self::cell_text(cells, column), width))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        (0..height)
            .map(|line| {
                wrapped.iter().map(|cell| cell.get(line).cloned().unwrap_or_default()).collect()
            })
            .collect()
    }

    fn content_line(&self, cells: &[String], widths: &[usize], style: Style) -> Line<'static> {
        let mut spans = Vec::with_capacity(widths.len() * 2 + 1);
        spans.push(Span::styled("│", self.border));
        for (column, &width) in widths.iter().enumerate() {
            let alignment = self.alignments.get(column).copied().unwrap_or(Alignment::Left);
            let text = cells.get(column).map_or("", String::as_str);
            spans.push(Span::styled(format!(" {} ", pad(text, width, alignment)), style));
            spans.push(Span::styled("│", self.border));
        }
        Line::from(spans)
    }

    fn rule(&self, widths: &[usize], left: char, middle: char, right: char) -> Line<'static> {
        let mut rule = String::new();
        rule.push(left);
        for (index, width) in widths.iter().enumerate() {
            if index > 0 {
                rule.push(middle);
            }
            rule.extend(std::iter::repeat_n('─', width + 2));
        }
        rule.push(right);
        Line::from(Span::styled(rule, self.border))
    }

    /// The table as records, for a column too narrow for any grid: each row's cells as
    /// `Header: value`, the value wrapped under itself, a rule between rows.
    fn records(&self, width: usize) -> Vec<Line<'static>> {
        let label_style = self.cell.add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        for (index, row) in self.body.iter().enumerate() {
            if index > 0 {
                lines.push(Line::from(Span::styled("─".repeat(width), self.border)));
            }
            for column in 0..self.columns() {
                let label = Self::cell_text(&self.header, column);
                let value = Self::cell_text(row, column);
                let lead = match label.is_empty() {
                    true => String::new(),
                    false => format!("{label}: "),
                };
                let lead_width = UnicodeWidthStr::width(lead.as_str());
                let indented = |text: String, indent: usize| {
                    Line::from(vec![Span::raw(" ".repeat(indent)), Span::styled(text, self.cell)])
                };
                if lead_width * 2 <= width {
                    let mut value_lines = wrap(value, width - lead_width).into_iter();
                    let head = value_lines.next().unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::styled(lead, label_style),
                        Span::styled(head, self.cell),
                    ]));
                    lines.extend(value_lines.map(|line| indented(line, lead_width)));
                } else {
                    // A label wider than half the row takes a line of its own, the value under it
                    let label_lines = wrap(lead.trim_end(), width);
                    lines.extend(
                        label_lines
                            .into_iter()
                            .map(|line| Line::from(Span::styled(line, label_style))),
                    );
                    let indent = usize::from(width > 2) * 2;
                    lines.extend(
                        wrap(value, width - indent)
                            .into_iter()
                            .filter(|line| !line.is_empty())
                            .map(|line| indented(line, indent)),
                    );
                }
            }
        }
        lines
    }
}

/// Spreads `spare` cells over columns already at `floor`, one at a time, each to the column still
/// furthest from what it `wants`; a column never gets more than it wants.
///
/// Filling from the most wanting evens out what each column is short of, so a column of long
/// sentences grows before a column of short labels, and both stop at what their content needs.
fn share(floor: &[usize], wants: &[usize], spare: usize) -> Vec<usize> {
    let mut widths = floor.to_vec();
    for _ in 0..spare {
        let neediest = widths
            .iter()
            .zip(wants)
            .enumerate()
            .filter(|(_, (have, want))| have < want)
            .max_by_key(|(index, (have, want))| (*want - *have, std::cmp::Reverse(*index)));
        let Some((column, _)) = neediest else {
            break;
        };
        widths[column] += 1;
    }
    widths
}

/// Trims `widths` until their sum is at most `budget`, taking from the widest first.
fn fit(mut widths: Vec<usize>, budget: usize) -> Vec<usize> {
    while widths.iter().sum::<usize>() > budget {
        let Some(widest) = widths.iter_mut().filter(|width| **width > 1).max_by_key(|w| **w) else {
            break;
        };
        *widest -= 1;
    }
    widths
}

/// `text` broken into lines of at most `width` cells: at spaces where it can be, through a word
/// where one is longer than the line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let gap = usize::from(!line.is_empty());
        if used + gap + word_width <= width {
            if gap == 1 {
                line.push(' ');
            }
            line.push_str(word);
            used += gap + word_width;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        // Too long for any line: split it at the width, carrying the tail on
        for character in word.chars() {
            let cells = character.width().unwrap_or(0);
            if used + cells > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(character);
            used += cells;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text`, already no wider than `width`, padded to exactly `width` cells.
fn pad(text: &str, width: usize, alignment: Alignment) -> String {
    let room = width.saturating_sub(UnicodeWidthStr::width(text));
    match alignment {
        Alignment::Left => format!("{text}{}", " ".repeat(room)),
        Alignment::Right => format!("{}{text}", " ".repeat(room)),
        Alignment::Center => {
            let left = room / 2;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(room - left))
        }
    }
}

/// A table being collected as the walk passes over it: its alignments, the header, the body
/// rows, and the row and cell in progress.
#[derive(Default)]
pub(super) struct TableBuilder {
    active: bool,
    alignments: Vec<pulldown_cmark::Alignment>,
    cell: String,
    row: Vec<String>,
    header: Vec<String>,
    body: Vec<Vec<String>>,
}

impl TableBuilder {
    pub(super) fn begin(&mut self, alignments: Vec<pulldown_cmark::Alignment>) {
        self.alignments = alignments;
        self.active = true;
        self.header.clear();
        self.body.clear();
    }

    /// Whether the walk is inside a table, where text and code go to the cell in progress.
    pub(super) fn active(&self) -> bool {
        self.active
    }

    pub(super) fn has_cell(&self) -> bool {
        !self.cell.is_empty()
    }

    pub(super) fn has_row(&self) -> bool {
        !self.row.is_empty()
    }

    pub(super) fn begin_cell(&mut self) {
        self.cell.clear();
    }

    pub(super) fn push_cell(&mut self, text: &str) {
        self.cell.push_str(text);
    }

    pub(super) fn end_cell(&mut self) {
        self.row.push(std::mem::take(&mut self.cell));
    }

    pub(super) fn end_head(&mut self) {
        self.header = std::mem::take(&mut self.row);
    }

    pub(super) fn end_row(&mut self) {
        self.body.push(std::mem::take(&mut self.row));
    }

    /// Closes the table, buffering any trailing cell or row, and emits it whole so column widths
    /// and borders can be computed across every row.
    pub(super) fn finish(
        &mut self,
        theme: &crate::theme::styles::ThemeStyles,
        voice: super::MarkdownVoice,
    ) -> Option<super::MarkdownLine> {
        if !self.cell.is_empty() {
            self.end_cell();
        }
        if !self.row.is_empty() {
            self.end_row();
        }
        let table = TableBlock::new(
            std::mem::take(&mut self.header),
            std::mem::take(&mut self.body),
            &self.alignments,
            super::voice::build_table_border_style(theme, voice),
            super::voice::build_table_style(theme, voice),
        );
        self.active = false;
        self.alignments.clear();
        table.map(super::MarkdownLine::table)
    }
}

#[cfg(test)]
mod tests;
