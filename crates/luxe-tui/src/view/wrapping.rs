//! Conversation wrapping — cell-aware transcript wrapping and canvas indentation.

use super::{ConversationCanvasWidths, SAFE_GUTTER_WIDTH};
use crate::markdown::{FenceRule, MarkdownBlockKind, MarkdownLine};
use crate::tabstop::TAB_WIDTH;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

pub(super) fn conversation_line(
    layout: MarkdownLine,
    canvas: ConversationCanvasWidths,
) -> Vec<Line<'static>> {
    let (width, x) = match layout.kind {
        MarkdownBlockKind::Prose => (canvas.prose_width, canvas.prose_x),
        MarkdownBlockKind::Wide => (canvas.wide_width, canvas.wide_x),
    };
    // A rule is one line by construction -- built to the width it is handed, so there is nothing
    // for the wrapper to do to it.
    if let Some(rule) = layout.fence_rule {
        return vec![indent(fence_rule(rule, width), x)];
    }
    // A table lays itself out to the width: handed to the wrapper whole, its rows split mid-grid
    if let Some(table) = layout.table {
        return table
            .lines_at(usize::from(width))
            .into_iter()
            .map(|line| indent(line, x))
            .collect();
    }
    let line = match layout.ends_at {
        Some(column) => ends_at(layout.line, column.min(usize::from(width))),
        None => layout.line,
    };
    // A line that names its own column hangs there; otherwise prose only, where a wrapped list
    // item or indented run lines up under its own text. Code and wide rows keep their columns.
    let hang = match (layout.hang, layout.kind) {
        (Some(column), _) => named_hang(&line, column, usize::from(width)),
        (None, MarkdownBlockKind::Prose) => hanging_indent(&line, usize::from(width)),
        (None, MarkdownBlockKind::Wide) => None,
    };
    wrap_rows(line, width, hang).into_iter().map(|line| indent(line, x)).collect()
}

/// Where a prose line's wrapped rows should start: under the text, past its indent and marker.
///
/// A bullet (`•`, `-`, `*`), a number (`1.`, `2)`) and a task box (`[ ]`) all count as marker, so
/// the second row of a list item starts under its first word rather than back at the margin. So
/// does plain leading space, which is how a status view like `/tools` indents a list of names.
/// Nothing hangs past half the width: a row that narrow is better off using the room.
///
/// Returns the column and the style the leading cells were drawn in, so a card's band carries on
/// through the indent.
fn hanging_indent(line: &Line<'static>, width: usize) -> Option<(usize, Style)> {
    let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    let style = line.spans.first().map_or(line.style, |span| line.style.patch(span.style));
    let spaces = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[spaces..];
    // Every marker is ASCII but the bullet, so byte lengths and cell widths agree for the rest
    let marker: &str = ["• ", "- ", "* "]
        .into_iter()
        .find(|bullet| rest.starts_with(bullet))
        .or_else(|| {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            let after = &rest[digits..];
            (digits > 0 && (after.starts_with(". ") || after.starts_with(") ")))
                .then(|| &rest[..digits + 2])
        })
        .unwrap_or("");
    let boxed = ["[ ] ", "[x] ", "[X] "].iter().any(|task| rest[marker.len()..].starts_with(task));
    let column = spaces + marker.chars().map(char_width).sum::<usize>() + if boxed { 4 } else { 0 };
    (column > 0 && column < width / 2).then_some((column, style))
}

/// A hang at a column the line chose, in the style its leading cells were drawn in so a card's band
/// carries on through the indent. Never past half the width, by the same reasoning as
/// [`hanging_indent`]: a row that narrow is better off using the room.
fn named_hang(line: &Line<'static>, column: usize, width: usize) -> Option<(usize, Style)> {
    let style = line.spans.first().map_or(line.style, |span| line.style.patch(span.style));
    (column > 0 && column < width / 2).then_some((column, style))
}

/// The glyph a code fence's frame is drawn with -- a box-drawing horizontal, which every terminal
/// renders itself against the cell box rather than scaling a font outline into it.
const FENCE_RULE: char = '\u{2500}';

/// Cells the rule spends seating a label: one rule glyph and a space outside it, a space inside.
/// Without the inner space the label butts into the rule and the two read as one word.
pub(crate) const LABEL_FRAME: usize = 3;

/// Rule glyphs a frame keeps between its labels, so a short block reads as framed rather than as
/// two labels that happen to have collided.
pub(crate) const MIN_FENCE_FILL: usize = 4;

/// Expands a [`FenceRule`] into the line it describes, out to `width`.
///
/// The labels sit *on* the rule rather than above it, so the fill is whatever is left between them.
/// A block too narrow to seat both keeps its labels and loses the rule: the copy affordance is a
/// control and the rule is decoration, so the decoration is what yields.
fn fence_rule(rule: FenceRule, width: u16) -> Line<'static> {
    let total = rule.ends_at.min(usize::from(width));
    let label_width = |spans: &[Span<'static>]| -> usize {
        spans.iter().map(|span| span.content.chars().map(char_width).sum::<usize>()).sum()
    };
    let lead = label_width(&rule.lead);
    let tail = label_width(&rule.tail);
    let spent = lead
        + tail
        + if lead > 0 { LABEL_FRAME } else { 0 }
        + if tail > 0 { LABEL_FRAME } else { 0 };

    let Some(fill) = total.checked_sub(spent).filter(|fill| *fill > 0) else {
        // No room to frame them: the labels alone, pushed right
        let mut spans = rule.lead;
        spans.extend(rule.tail);
        return ends_at(Line::from(spans), total);
    };

    let dash = |count: usize| Span::styled(FENCE_RULE.to_string().repeat(count), rule.style);
    let mut spans = Vec::with_capacity(rule.lead.len() + rule.tail.len() + 6);
    if lead > 0 {
        spans.push(dash(1));
        spans.push(Span::styled(" ", rule.style));
        spans.extend(rule.lead);
        spans.push(Span::styled(" ", rule.style));
    }
    spans.push(dash(fill));
    if tail > 0 {
        spans.push(Span::styled(" ", rule.style));
        spans.extend(rule.tail);
        spans.push(Span::styled(" ", rule.style));
        spans.push(dash(1));
    }
    Line::from(spans)
}

/// One character's cell width, with the unprintable treated as zero.
fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// Pushes `line` right so it finishes on `column`, or leaves it where it is when it cannot fit.
///
/// The padding is a leading span rather than a caller's format width, so it survives the wrapping
/// below intact: a right-aligned line is a single short label by construction, and one that has
/// outgrown its column has nowhere to be pushed to anyway.
fn ends_at(line: Line<'static>, column: usize) -> Line<'static> {
    let Some(lead) = column.checked_sub(line.width()) else {
        return line;
    };
    if lead == 0 {
        return line;
    }
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(" ".repeat(lead)));
    spans.extend(line.spans);
    Line::from(spans)
}

pub(super) fn wide_line(
    line: Line<'static>,
    canvas: ConversationCanvasWidths,
) -> Vec<Line<'static>> {
    styled_line(line, canvas.wide_width)
        .into_iter()
        .map(|line| indent(line, canvas.wide_x))
        .collect()
}

pub(super) fn styled_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    wrap_rows(line, width, None)
}

/// [`styled_line`], starting every row after the first at `hang`'s column, in its style.
fn wrap_rows(line: Line<'static>, width: u16, hang: Option<(usize, Style)>) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut wrapped = Vec::new();
    let mut spans = Vec::<Span<'static>>::new();
    let mut whitespace = Vec::<(char, Style)>::new();
    let mut word = Vec::<(char, Style)>::new();
    let mut current_width = 0;

    for span in line.spans {
        let style = line.style.patch(span.style);
        for character in span.content.chars() {
            if character == '\n' {
                append_word(
                    &mut wrapped,
                    &mut spans,
                    &mut current_width,
                    &mut whitespace,
                    &mut word,
                    (width, hang),
                );
                break_row(&mut wrapped, &mut spans, &mut current_width, hang);
                whitespace.clear();
            } else if character.is_whitespace() {
                append_word(
                    &mut wrapped,
                    &mut spans,
                    &mut current_width,
                    &mut whitespace,
                    &mut word,
                    (width, hang),
                );
                whitespace.push((character, style));
            } else {
                word.push((character, style));
            }
        }
    }
    append_word(
        &mut wrapped,
        &mut spans,
        &mut current_width,
        &mut whitespace,
        &mut word,
        (width, hang),
    );
    if !spans.is_empty() || wrapped.is_empty() {
        wrapped.push(Line::from(spans));
    }
    wrapped
}

fn indent(line: Line<'static>, canvas_x: u16) -> Line<'static> {
    let padding = usize::from(canvas_x.saturating_sub(SAFE_GUTTER_WIDTH / 2));
    if padding == 0 {
        return line;
    }
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(" ".repeat(padding)));
    spans.extend(line.spans);
    Line::from(spans)
}

/// The row width, and where a continuation row starts. Args: `(width, hang)`.
type RowShape = (usize, Option<(usize, Style)>);

fn append_word(
    wrapped: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    whitespace: &mut Vec<(char, Style)>,
    word: &mut Vec<(char, Style)>,
    shape: RowShape,
) {
    if word.is_empty() {
        return;
    }
    let (width, hang) = shape;
    let whitespace_width = symbols_width(whitespace, *current_width);
    let word_width = symbols_width(word, *current_width + whitespace_width);
    // Text starts at the hang on every row, the first included: a marker alone on its row is
    // worse than a word split under it
    let row_start = hang.map_or(0, |(column, _)| column);
    if *current_width > row_start && *current_width + whitespace_width + word_width > width {
        break_row(wrapped, spans, current_width, hang);
        whitespace.clear();
    } else {
        append_symbols(wrapped, spans, current_width, whitespace, shape);
    }
    append_symbols(wrapped, spans, current_width, word, shape);
}

/// Ends the row in progress and opens the next one at the hanging indent.
fn break_row(
    wrapped: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    hang: Option<(usize, Style)>,
) {
    wrapped.push(Line::from(std::mem::take(spans)));
    *current_width = 0;
    if let Some((column, style)) = hang {
        spans.push(Span::styled(" ".repeat(column), style));
        *current_width = column;
    }
}

fn append_symbols(
    wrapped: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    symbols: &mut Vec<(char, Style)>,
    (width, hang): RowShape,
) {
    for (character, style) in symbols.drain(..) {
        // The last line of defense: anything laid out against a measured width has already spent
        // its tabs through `tabstop::display_text`, and a tab reaching a buffer cell would move the
        // terminal's cursor rather than draw. Prose that never went through a measurement lands
        // here, where the expansion and the wrap are the same pass and so cannot disagree.
        let expanded = if character == '\t' {
            " ".repeat(TAB_WIDTH - (*current_width % TAB_WIDTH))
        } else {
            character.to_string()
        };
        for symbol in expanded.chars() {
            let symbol_width = symbol.width().unwrap_or(0);
            let row_start = hang.map_or(0, |(column, _)| column);
            if *current_width > row_start && *current_width + symbol_width > width {
                break_row(wrapped, spans, current_width, hang);
            }
            push_styled_char(spans, symbol, style);
            *current_width += symbol_width;
        }
    }
}

fn symbols_width(symbols: &[(char, Style)], start: usize) -> usize {
    let mut column = start;
    for (character, _) in symbols {
        column += if *character == '\t' {
            TAB_WIDTH - (column % TAB_WIDTH)
        } else {
            character.width().unwrap_or(0)
        };
    }
    column - start
}

fn push_styled_char(spans: &mut Vec<Span<'static>>, character: char, style: Style) {
    if let Some(last) = spans.last_mut().filter(|span| span.style == style) {
        last.content.to_mut().push(character);
    } else {
        spans.push(Span::styled(character.to_string(), style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: u16, kind: MarkdownBlockKind) -> Vec<String> {
        let line = Line::from(text.to_string());
        let hang = match kind {
            MarkdownBlockKind::Prose => hanging_indent(&line, usize::from(width)),
            MarkdownBlockKind::Wide => None,
        };
        wrap_rows(line, width, hang).iter().map(Line::to_string).collect()
    }

    /// A line that names its own hang keeps it whatever its kind, every row, but never past half the
    /// width.
    #[test]
    fn a_named_hang_holds_on_every_row_of_a_wide_line() {
        let line = Line::from("question  one two three four five six seven");
        let hang = named_hang(&line, 10, 22);
        let rows: Vec<String> =
            wrap_rows(line.clone(), 22, hang).iter().map(Line::to_string).collect();
        assert_eq!(
            rows,
            ["question  one two", "          three four", "          five six", "          seven"]
        );
        assert_eq!(named_hang(&line, 11, 22), None, "half the width or more is too far");
    }

    /// A wrapped list item carries on under its own first word, not back at the margin.
    #[test]
    fn a_wrapped_list_item_hangs_under_its_text() {
        let bullet = rows("• alpha beta gamma delta", 14, MarkdownBlockKind::Prose);
        assert_eq!(bullet, ["• alpha beta", "  gamma delta"]);
        let numbered = rows("12. alpha beta gamma", 14, MarkdownBlockKind::Prose);
        assert_eq!(numbered, ["12. alpha beta", "    gamma"]);
        let nested = rows("  - [ ] alpha beta gamma", 20, MarkdownBlockKind::Prose);
        assert_eq!(nested, ["  - [ ] alpha beta", "        gamma"]);
    }

    /// An indented run of names, the way `/tools` lists them, keeps its indent on every row.
    #[test]
    fn an_indented_run_keeps_its_indent() {
        let tools = rows("    read  write  edit  bash  grep", 21, MarkdownBlockKind::Prose);
        assert_eq!(tools, ["    read  write  edit", "    bash  grep"]);
    }

    /// A long word still splits at the width, and the split half hangs too.
    #[test]
    fn a_hard_split_word_hangs_and_fits() {
        let split = rows("• abcdefghijklmnop", 10, MarkdownBlockKind::Prose);
        assert_eq!(split, ["• abcdefgh", "  ijklmnop"]);
    }

    /// Code and wide rows keep exactly the columns they were drawn with.
    #[test]
    fn wide_rows_and_plain_prose_do_not_hang() {
        assert_eq!(rows("    alpha beta gamma", 12, MarkdownBlockKind::Wide)[1], "beta gamma");
        assert_eq!(rows("alpha beta gamma", 12, MarkdownBlockKind::Prose)[1], "gamma");
        assert_eq!(rows("-5 is alpha beta", 10, MarkdownBlockKind::Prose)[1], "alpha beta");
    }

    /// Past half the width a hang would leave too little row to be worth it.
    #[test]
    fn a_deep_indent_does_not_hang_on_a_narrow_row() {
        let deep = rows("          alpha beta", 16, MarkdownBlockKind::Prose);
        assert_eq!(deep[1], "beta");
    }

    /// The indent wears the style the line began in, so a card's band has no hole in it.
    #[test]
    fn the_indent_carries_the_leading_style() {
        let band = Style::default().bg(ratatui::style::Color::Blue);
        let line = Line::from(Span::styled("• alpha beta gamma delta", band));
        let hang = hanging_indent(&line, 14);
        let wrapped = wrap_rows(line, 14, hang);
        assert!(wrapped[1].spans[0].content.starts_with("  gamma"), "{:?}", wrapped[1]);
        assert_eq!(wrapped[1].spans[0].style, band);
    }
}
