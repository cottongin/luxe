//! Word-wrapping for styled lines, done ahead of render rather than by the widget.
//!
//! `Paragraph::wrap` is fine when all a surface does is print prose, but it wraps *during* the
//! draw, so the caller never learns which terminal row a given line ended up on. Any surface that
//! wants to publish click targets, or put a selection marker on a row, needs that mapping — so it
//! has to own the wrap. Wrapping here also keeps a row and a line the same thing, which makes
//! scroll offsets mean what they say.

use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Where each line starts, in screen rows, once `Paragraph`'s word wrap at `width` is applied.
///
/// `starts[index]` is the first row of `lines[index]`, and one entry past the end holds the total
/// height — so a line's own height is the gap between consecutive entries. A surface that
/// publishes click regions over a wrapped paragraph needs exactly that mapping.
///
/// One pass, because the obvious alternative is not: asking a `Paragraph` for the height of every
/// prefix is quadratic in the number of lines *and* clones the prefix to ask, thousands of
/// allocations a frame for a forty-row sidebar. Spans are borrowed for the
/// measurement, so nothing here allocates a string.
pub fn wrapped_row_starts(lines: &[Line<'_>], width: u16, trim: bool) -> Vec<u16> {
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut used = 0u16;
    for line in lines {
        starts.push(used);
        used = used.saturating_add(wrapped_height(line, width, trim));
    }
    starts.push(used);
    starts
}

/// The rows one line occupies under `Paragraph`'s word wrap at `width`.
fn wrapped_height(line: &Line<'_>, width: u16, trim: bool) -> u16 {
    let borrowed = Line::from(
        line.spans.iter().map(|span| Span::styled(&*span.content, span.style)).collect::<Vec<_>>(),
    );
    u16::try_from(Paragraph::new(borrowed).wrap(Wrap { trim }).line_count(width))
        .unwrap_or(u16::MAX)
}

/// Word-wraps one styled line to `width` cells, preserving each span's style.
///
/// Breaks at whitespace, hard-splits a single word too long to fit, and preserves leading
/// indentation on continuation rows so a wrapped bullet stays visually attached to its bullet.
/// An empty line yields one empty row, so blank spacing survives.
pub fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line.clone()];
    }
    let total: usize = line.spans.iter().map(|span| span.content.width()).sum();
    if total <= width {
        return vec![line.clone()];
    }

    let indent = continuation_indent(line, width);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    // A word held back until we know whether it fits on the current row.
    let mut word: Vec<Span<'static>> = Vec::new();
    let mut word_width = 0usize;

    let mut flush_row = |current: &mut Vec<Span<'static>>, used: &mut usize| {
        rows.push(std::mem::take(current));
        *used = indent.len();
        if indent.is_empty() {
            Vec::new()
        } else {
            vec![Span::raw(indent.clone())]
        }
    };

    for span in &line.spans {
        for chunk in split_keeping_whitespace(&span.content) {
            let chunk_width = chunk.width();
            if chunk.chars().all(char::is_whitespace) {
                // Whitespace closes the pending word and is dropped at a row break.
                if !word.is_empty() {
                    current.append(&mut word);
                    used += word_width;
                    word_width = 0;
                }
                if used + chunk_width <= width {
                    current.push(Span::styled(chunk.to_string(), span.style));
                    used += chunk_width;
                }
                continue;
            }
            if word_width + chunk_width > width.saturating_sub(indent.len()).max(1) {
                // A single word longer than a row: commit what fits and hard-split the rest.
                current.append(&mut word);
                used += word_width;
                word_width = 0;
                for character in chunk.chars() {
                    let cells = UnicodeWidthChar::width(character).unwrap_or(0);
                    if used + cells > width && used > 0 {
                        current = flush_row(&mut current, &mut used);
                    }
                    current.push(Span::styled(character.to_string(), span.style));
                    used += cells;
                }
                continue;
            }
            if used + word_width + chunk_width > width && used > 0 {
                current = flush_row(&mut current, &mut used);
            }
            word.push(Span::styled(chunk.to_string(), span.style));
            word_width += chunk_width;
        }
    }
    if !word.is_empty() {
        if used + word_width > width && used > 0 {
            current = flush_row(&mut current, &mut used);
        }
        current.append(&mut word);
    }
    rows.push(current);
    rows.into_iter().map(Line::from).collect()
}

/// Wraps every line, flattening the result — one entry per rendered terminal row.
pub fn wrap_lines(lines: &[Line<'static>], width: usize) -> Vec<Line<'static>> {
    lines.iter().flat_map(|line| wrap_line(line, width)).collect()
}

/// Hard-wraps styled spans at exactly `width` cells, preserving styles and never splitting a wide
/// glyph. Empty input yields one empty row.
///
/// The cell-exact counterpart to [`wrap_line`], for content where a word boundary is the wrong place
/// to break: source code in a diff column, where breaking early leaves a ragged gap and the reader
/// is counting on columns lining up.
pub fn wrap_cells(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![spans];
    }
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let style = span.style;
        let mut piece = String::new();
        for character in span.content.chars() {
            let cells = UnicodeWidthChar::width(character).unwrap_or(0);
            if used + cells > width && used > 0 {
                if !piece.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut piece), style));
                }
                rows.push(std::mem::take(&mut current));
                used = 0;
            }
            piece.push(character);
            used += cells;
        }
        if !piece.is_empty() {
            current.push(Span::styled(piece, style));
        }
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

/// The leading whitespace continuation rows inherit, capped so an indent can never consume the row.
fn continuation_indent(line: &Line<'static>, width: usize) -> String {
    let leading: String = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .take_while(|character| *character == ' ')
        .collect();
    if leading.width() >= width / 2 {
        return String::new();
    }
    leading
}

/// Splits into alternating runs of whitespace and non-whitespace.
fn split_keeping_whitespace(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut previous: Option<bool> = None;
    for (index, character) in text.char_indices() {
        let is_space = character.is_whitespace();
        if previous.is_some_and(|was| was != is_space) {
            parts.push(&text[start..index]);
            start = index;
        }
        previous = Some(is_space);
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn a_short_line_is_left_alone() {
        let line = Line::from("short");
        assert_eq!(wrap_line(&line, 20).len(), 1);
    }

    #[test]
    fn prose_breaks_at_word_boundaries() {
        let line = Line::from("the quick brown fox jumps over the lazy dog");
        let rows = wrap_line(&line, 20);
        assert!(rows.len() > 1);
        for row in &rows {
            assert!(plain(row).width() <= 20, "row overflows: {:?}", plain(row));
            assert!(!plain(row).starts_with(' '), "a break should not strand whitespace");
        }
        let rejoined: String =
            rows.iter().map(|row| plain(row).trim_end().to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(rejoined, "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn a_word_longer_than_the_row_is_split_rather_than_dropped() {
        let line = Line::from("x".repeat(50));
        let rows = wrap_line(&line, 10);
        assert!(rows.len() >= 5);
        let rejoined: String = rows.iter().map(plain).collect();
        assert_eq!(rejoined, "x".repeat(50));
    }

    #[test]
    fn styles_survive_the_wrap() {
        let line = Line::from(vec![
            Span::styled("alpha ", Style::default().fg(Color::Red)),
            Span::styled("beta gamma delta", Style::default().fg(Color::Blue)),
        ]);
        let rows = wrap_line(&line, 12);
        assert!(rows.len() > 1);
        let reds: usize = rows
            .iter()
            .flat_map(|row| row.spans.iter())
            .filter(|span| span.style.fg == Some(Color::Red))
            .count();
        assert!(reds > 0, "the first span keeps its color");
    }

    /// Continuation rows keep a bullet's indent so the wrapped tail stays under its text.
    #[test]
    fn continuation_rows_inherit_the_indent() {
        let line = Line::from("    a bullet whose text runs past the available width");
        let rows = wrap_line(&line, 24);
        assert!(rows.len() > 1);
        assert!(plain(&rows[1]).starts_with("    "), "second row: {:?}", plain(&rows[1]));
    }

    #[test]
    fn wide_glyphs_are_never_split_across_rows() {
        let line = Line::from("日本語のテキストがここにあります");
        let rows = wrap_line(&line, 9);
        for row in &rows {
            assert!(plain(row).width() <= 9, "row overflows: {:?}", plain(row));
        }
    }

    #[test]
    fn every_line_yields_at_least_one_row_so_blank_spacing_survives() {
        let lines = vec![Line::from("one"), Line::default(), Line::from("two")];
        assert_eq!(wrap_lines(&lines, 20).len(), 3);
    }
}
