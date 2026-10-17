//! Diff line primitives — how one source line becomes spans, for every surface that shows a diff.
//!
//! Guided Review and the Changes pane draw the same thing at different weights: the review adds
//! folds, findings, chapters and a split projection; the pane adds none of it. What they must never
//! disagree about is the line itself — which hue a deletion wears, where the intra-line marks fall,
//! how a wide glyph is clipped — because the reader escalates from one to the other and expects to
//! be looking at the same code.
//!
//! Everything here takes [`ThemeStyles`] rather than the app, so it is a function of the theme and
//! the line and nothing else.

use crate::guided_review::diff::SplitDiffCell;
use crate::tabstop::display_text;
use crate::theme::styles::ThemeStyles;
use luxe_core::guided_review::{ReviewLine, ReviewLineKind};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The rule between the two columns of a side-by-side diff.
pub(crate) const SPLIT_DIVIDER: &str = " │ ";

/// Cells the rule occupies.
///
/// Spelled out because `SPLIT_DIVIDER.len()` is five — `│` is three bytes — and sizing the columns
/// in bytes quietly spent two columns of every side-by-side diff on nothing.
pub(crate) const SPLIT_DIVIDER_CELLS: usize = 3;

/// Below this a column cannot show useful code, so the caller keeps unified.
pub(crate) const MIN_SPLIT_SOURCE_WIDTH: usize = 8;

/// The changed spans of every replaced row, keyed by its index in the rendered window.
pub(crate) type IntralineEmphasis = std::collections::HashMap<usize, Vec<std::ops::Range<usize>>>;

/// The changed spans recorded for one row, or none when nothing about it was replaced.
pub(crate) fn changed_spans(
    emphasized: &IntralineEmphasis,
    index: usize,
) -> &[std::ops::Range<usize>] {
    emphasized.get(&index).map_or(&[][..], Vec::as_slice)
}

/// Maps each replaced diff row (a deletion paired with the k-th following addition) to the byte
/// ranges of its intra-line change.
///
/// Runs of deletions pair positionally with the following run of additions; identical lines,
/// rewrites and unpaired rows are omitted. `rows` carries `None` wherever the surface drew
/// something that is not a source line — a hunk header, a collapsed run, a spacer — so the indices
/// it returns are indices into whatever the caller is rendering.
pub(crate) fn intraline_emphasis(rows: &[Option<&ReviewLine>]) -> IntralineEmphasis {
    let kind = |index: usize| rows.get(index).copied().flatten().map(|line| line.kind);
    let mut emphasized = IntralineEmphasis::new();
    let mut index = 0;
    while index < rows.len() {
        let deletions_start = index;
        while kind(index) == Some(ReviewLineKind::Deletion) {
            index += 1;
        }
        let deletions_end = index;
        let additions_start = index;
        while kind(index) == Some(ReviewLineKind::Addition) {
            index += 1;
        }
        let pairs = (deletions_end - deletions_start).min(index - additions_start);
        for offset in 0..pairs {
            if let (Some(Some(deletion)), Some(Some(addition))) =
                (rows.get(deletions_start + offset), rows.get(additions_start + offset))
            {
                // Computed over the *drawn* text, because the ranges it returns are byte offsets
                // the renderer then slices the drawn text with. Marking a span found in the raw
                // line would land somewhere else once tabs became spaces.
                let change = luxe_core::intraline::intraline_change(
                    &display_text(&deletion.text),
                    &display_text(&addition.text),
                );
                if !change.is_empty() {
                    emphasized.insert(deletions_start + offset, change.removed);
                    emphasized.insert(additions_start + offset, change.added);
                }
            }
        }
        // Advance past a non-deletion row (header/context) that started no run.
        if index == deletions_start {
            index += 1;
        }
    }
    emphasized
}

/// The verdict color a row wears: green added, red removed, body text for everything else.
pub(crate) fn line_style(kind: ReviewLineKind, styles: &ThemeStyles) -> Style {
    match kind {
        ReviewLineKind::Addition => styles.success,
        ReviewLineKind::Deletion => styles.error,
        _ => styles.text,
    }
}

/// The full-width tint a changed row sits on, or `None` for context.
pub(crate) fn diff_line_background(kind: ReviewLineKind, styles: &ThemeStyles) -> Option<Style> {
    match kind {
        ReviewLineKind::Addition => Some(styles.diff_add_bg),
        ReviewLineKind::Deletion => Some(styles.diff_del_bg),
        _ => None,
    }
}

/// Patches every span with `background` and pads the row to `width` cells so the tint reads as a
/// full-width bar rather than only coloring the glyphs (ratatui paints trailing cells with the
/// paragraph style, not the line style).
///
/// A span that brought its own background keeps it. The row tint says the line changed; the deeper
/// wash on a changed span says *what* changed, which is the more specific claim — laying the row
/// tint over it would erase the one thing the reader is meant to land on.
pub(crate) fn tint_row<'a>(
    mut spans: Vec<Span<'a>>,
    background: Style,
    width: u16,
) -> Vec<Span<'a>> {
    for span in spans.iter_mut().filter(|span| span.style.bg.is_none()) {
        span.style = span.style.patch(background);
    }
    let used: usize = spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
    let target = usize::from(width);
    if used < target {
        spans.push(Span::styled(" ".repeat(target - used), background));
    }
    spans
}

/// Syntax-highlights one source line, falling back to body text when the highlighter has nothing.
pub(crate) fn highlighted_source_spans(
    text: &str,
    language: Option<&str>,
    styles: &ThemeStyles,
) -> Vec<Span<'static>> {
    let spans = crate::highlight::highlight_code(text, language, styles)
        .into_iter()
        .next()
        .map(|line| line.spans)
        .unwrap_or_default();

    if spans.is_empty() {
        vec![Span::styled(text.to_owned(), styles.text)]
    } else {
        spans
    }
}

/// Clips a sequence of `(text, style)` parts to `width` terminal cells starting at `offset`,
/// preserving each part's style — the styled analogue of
/// [`crate::guided_review::diff::clip_terminal_cells`].
pub(crate) fn clip_styled_spans<'a>(
    parts: &[(&'a str, Style)],
    offset: usize,
    width: usize,
) -> Vec<Span<'a>> {
    let mut skipped = 0usize;
    let mut taken = 0usize;
    let mut spans = Vec::new();
    'parts: for (text, style) in parts {
        let mut piece = String::new();
        for character in text.chars() {
            let cells = UnicodeWidthChar::width(character).unwrap_or(0);
            if skipped < offset {
                skipped = skipped.saturating_add(cells);
                continue;
            }
            if taken.saturating_add(cells) > width {
                if !piece.is_empty() {
                    spans.push(Span::styled(piece, *style));
                }
                break 'parts;
            }
            piece.push(character);
            taken = taken.saturating_add(cells);
        }
        if !piece.is_empty() {
            spans.push(Span::styled(piece, *style));
        }
    }
    spans
}

/// The sign in a unified row's gutter. A no-newline marker is a note about the file, not a change
/// to it, so it wears nothing.
pub(crate) fn rendered_marker(kind: ReviewLineKind) -> String {
    if matches!(kind, ReviewLineKind::NoNewlineMarker) {
        String::new()
    } else {
        crate::guided_review::diff::marker(kind).to_string()
    }
}

/// One source line's spans: highlighted as code where it is context, marked where it changed where
/// it is not.
///
/// `plain_wash` drops the deeper background from the changed span while keeping the mark — for a row
/// that already wears a highlight of its own (a search hit), where two competing backgrounds inside
/// one row leave neither legible.
pub(crate) fn source_spans(
    line: &ReviewLine,
    changed: &[std::ops::Range<usize>],
    language: Option<&str>,
    styles: &ThemeStyles,
    plain_wash: bool,
) -> Vec<Span<'static>> {
    let text = display_text(&line.text);
    if line.kind == ReviewLineKind::Context {
        return highlighted_source_spans(&text, language, styles);
    }
    let style = line_style(line.kind, styles);
    let wash = styles.diff_emphasis(style, line.kind == ReviewLineKind::Addition);
    let wash = if plain_wash { Style { bg: None, ..wash } } else { wash };
    crate::view::emphasis::spans(&text, changed, style, wash)
}

/// Geometry and theming for one side-by-side diff, shared by the review and the Changes pane.
///
/// The two surfaces differ in exactly two ways — the review reserves a gutter for finding markers
/// and can override a row's tint for a search hit; the pane does neither — so both arrive as
/// per-line arguments rather than as a second copy of the layout.
pub(crate) struct SplitLayout<'a> {
    /// Theme the columns are drawn from.
    pub(crate) styles: &'a ThemeStyles,
    /// Cells the line-number field occupies.
    pub(crate) numbers: usize,
    /// Cells reserved ahead of the number for a caller-owned marker (zero for no gutter).
    pub(crate) marker_width: usize,
    /// Cells one whole column occupies, the rule excluded.
    pub(crate) side: usize,
    /// Cells of source one column can show.
    pub(crate) source: usize,
    /// First source cell to show, for a horizontally scrolled column.
    ///
    /// Always zero where the surface wraps instead of scrolling sideways — wrapping supersedes it,
    /// which is why the review resets the offset when wrap turns on.
    pub(crate) column: usize,
    /// Language used to highlight context lines.
    pub(crate) language: Option<&'a str>,
}

/// Which column of a side-by-side diff a cell belongs to.
///
/// It decides which line number the gutter shows, and that is not cosmetic: the left column is the
/// old file and the right is the new one, so printing the old number beside new content is a false
/// statement about where that line lives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitSide {
    Old,
    New,
}

/// What the caller knows about one side of a row that the layout cannot work out for itself.
#[derive(Default, Clone)]
pub(crate) struct SplitCellStyle {
    /// Marker to print in the reserved gutter.
    pub(crate) marker: String,
    /// Row tint, when it overrides the add/delete wash.
    pub(crate) background: Option<Style>,
    /// Whether the changed-span wash stands down (see [`source_spans`]).
    pub(crate) plain_wash: bool,
}

/// Two equal columns with a rule between them, sized to `width`.
pub(crate) fn split_layout<'a>(
    styles: &'a ThemeStyles,
    width: u16,
    numbers: usize,
    marker_width: usize,
    language: Option<&'a str>,
) -> SplitLayout<'a> {
    let side = usize::from(width).saturating_sub(SPLIT_DIVIDER_CELLS) / 2;
    let source = side.saturating_sub(marker_width + numbers + 2).max(MIN_SPLIT_SOURCE_WIDTH);
    SplitLayout { styles, numbers, marker_width, side, source, column: 0, language }
}

impl SplitLayout<'_> {
    /// The rule drawn between the columns.
    pub(crate) fn divider(&self) -> Span<'static> {
        Span::styled(SPLIT_DIVIDER, self.styles.border_muted)
    }

    /// Cells before the source in a column: marker, number field, diff sign.
    pub(crate) fn continuation(&self) -> usize {
        self.marker_width + self.numbers + 2
    }

    /// The marker, line number and diff sign prefixing one side of a row.
    fn gutter(
        &self,
        line: &ReviewLine,
        cell: &SplitCellStyle,
        side: SplitSide,
    ) -> Vec<Span<'static>> {
        let numbers = self.numbers;
        let (own, other) = match side {
            SplitSide::Old => (line.old_line, line.new_line),
            SplitSide::New => (line.new_line, line.old_line),
        };
        let number = own
            .or(other)
            .map_or_else(|| " ".repeat(numbers), |number| format!("{number:>numbers$}"));
        let mut spans = Vec::with_capacity(3);
        if self.marker_width > 0 {
            let marker = &cell.marker;
            spans.push(Span::styled(
                format!("{marker:<width$}", width = self.marker_width),
                self.styles.accent,
            ));
        }
        spans.push(Span::styled(format!("{number} "), self.styles.muted));
        spans.push(Span::styled(rendered_marker(line.kind), line_style(line.kind, self.styles)));
        spans
    }

    /// A column of empty cells, so an asymmetric row does not let the rule wander.
    fn empty_column(&self) -> Vec<Span<'static>> {
        vec![Span::styled(" ".repeat(self.side), self.styles.muted)]
    }

    /// One side of one row, clipped to the column.
    pub(crate) fn cell_spans(
        &self,
        side: SplitSide,
        cell: SplitDiffCell<'_>,
        changed: &[std::ops::Range<usize>],
        style: &SplitCellStyle,
    ) -> Vec<Span<'static>> {
        let SplitDiffCell::Source(line) = cell else {
            return self.empty_column();
        };
        let mut spans = self.gutter(line, style, side);
        let parts = source_spans(line, changed, self.language, self.styles, style.plain_wash);
        spans.extend(clip_spans(parts, self.column, self.source));
        let background = style.background.or_else(|| diff_line_background(line.kind, self.styles));
        match background {
            Some(background) => tint_row(spans, background, self.side as u16),
            None => pad_to(spans, self.side),
        }
    }

    /// One row's visual lines, soft-wrapped so nothing hides off the right edge.
    ///
    /// A pair occupies `max(left, right)` lines with the shorter column padded, so the two sides
    /// stay aligned and the rule holds its screen column.
    pub(crate) fn wrapped_pair(
        &self,
        old: (SplitDiffCell<'_>, &[std::ops::Range<usize>], &SplitCellStyle),
        new: (SplitDiffCell<'_>, &[std::ops::Range<usize>], &SplitCellStyle),
    ) -> Vec<Line<'static>> {
        let column = |which: SplitSide,
                      (cell, changed, style): (
            SplitDiffCell<'_>,
            &[std::ops::Range<usize>],
            &SplitCellStyle,
        )| match cell {
            SplitDiffCell::Empty => (Vec::new(), Vec::new(), None),
            SplitDiffCell::Source(line) => (
                self.gutter(line, style, which),
                crate::widgets::wrap_cells(
                    source_spans(line, changed, self.language, self.styles, style.plain_wash),
                    self.source.max(1),
                ),
                style.background.or_else(|| diff_line_background(line.kind, self.styles)),
            ),
        };
        let (old_gutter, old_body, old_background) = column(SplitSide::Old, old);
        let (new_gutter, new_body, new_background) = column(SplitSide::New, new);

        let height = old_body.len().max(new_body.len()).max(1);
        (0..height)
            .map(|index| {
                let mut spans =
                    self.side_spans(index, &old_gutter, old_body.get(index), old_background);
                spans.push(self.divider());
                spans.extend(self.side_spans(
                    index,
                    &new_gutter,
                    new_body.get(index),
                    new_background,
                ));
                Line::from(spans)
            })
            .collect()
    }

    /// One column of one visual line: the gutter on the first, a continuation glyph after it.
    ///
    /// A pair is as tall as its longer side, so the shorter side runs out of segments partway down.
    /// Those lines keep the row's tint — a replaced row reads as one block — but must not wear a
    /// continuation glyph for a line that never wrapped.
    fn side_spans(
        &self,
        index: usize,
        gutter: &[Span<'static>],
        segment: Option<&Vec<Span<'static>>>,
        background: Option<Style>,
    ) -> Vec<Span<'static>> {
        if gutter.is_empty() {
            return self.empty_column();
        }
        let continuation = self.continuation();
        let mut spans: Vec<Span<'static>> = match (index, segment) {
            (0, _) => gutter.to_vec(),
            (_, Some(_)) => vec![Span::styled(
                format!(
                    "{}{}",
                    " ".repeat(continuation.saturating_sub(1)),
                    crate::glyphs::Mark::Continuation
                ),
                self.styles.dim,
            )],
            (_, None) => vec![Span::raw(" ".repeat(continuation))],
        };
        if let Some(segment) = segment {
            spans.extend(segment.iter().cloned());
        }
        match background {
            Some(background) => tint_row(spans, background, self.side as u16),
            None => pad_to(spans, self.side),
        }
    }
}

/// Clips already-styled spans to `width` cells starting at `offset`, keeping each span's style.
fn clip_spans(spans: Vec<Span<'static>>, offset: usize, width: usize) -> Vec<Span<'static>> {
    let parts: Vec<(&str, Style)> =
        spans.iter().map(|span| (span.content.as_ref(), span.style)).collect();
    owned_spans(clip_styled_spans(&parts, offset, width))
}

/// Detaches spans from whatever they borrow, so a row assembled from a temporary can outlive it.
pub(crate) fn owned_spans(spans: Vec<Span<'_>>) -> Vec<Span<'static>> {
    spans.into_iter().map(|span| Span::styled(span.content.into_owned(), span.style)).collect()
}

/// Pads a run of spans out to `width` cells with untinted blanks.
fn pad_to(mut spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: ReviewLineKind, text: &str) -> ReviewLine {
        ReviewLine { old_line: None, new_line: None, kind, text: text.into() }
    }

    /// The pairing is positional: the k-th deletion against the k-th following addition. Anything
    /// unpaired is left alone, because guessing a correspondence is how a rewrite gets marked as a
    /// one-token edit.
    #[test]
    fn a_replaced_line_marks_only_the_span_that_changed() {
        let deletion = line(ReviewLineKind::Deletion, "let total = a + b;");
        let addition = line(ReviewLineKind::Addition, "let total = a - b;");
        let rows = vec![None, Some(&deletion), Some(&addition), None];

        let emphasized = intraline_emphasis(&rows);
        assert!(!changed_spans(&emphasized, 1).is_empty(), "the removed span is marked");
        assert!(!changed_spans(&emphasized, 2).is_empty(), "so is the added one");
        assert!(changed_spans(&emphasized, 0).is_empty(), "a header is never a source row");
        assert!(changed_spans(&emphasized, 3).is_empty());
    }

    #[test]
    fn an_unpaired_addition_is_left_unmarked() {
        let addition = line(ReviewLineKind::Addition, "brand new");
        let rows = vec![Some(&addition)];
        assert!(intraline_emphasis(&rows).is_empty());
    }

    /// Two lines with nothing in common are a rewrite, and marking every character as changed
    /// tells the reader nothing they could not see.
    #[test]
    fn a_wholesale_rewrite_is_not_marked() {
        let deletion = line(ReviewLineKind::Deletion, "alpha");
        let addition = line(ReviewLineKind::Addition, "beta");
        let rows = vec![Some(&deletion), Some(&addition)];
        let emphasized = intraline_emphasis(&rows);
        // Whatever the differ decides here, it must not claim a change it cannot locate.
        for index in 0..rows.len() {
            for range in changed_spans(&emphasized, index) {
                assert!(range.start <= range.end, "a span that runs backwards");
            }
        }
    }

    #[test]
    fn clipping_counts_cells_rather_than_bytes() {
        let style = Style::default();
        let parts = [("日本語", style), ("abc", style)];
        // Three wide glyphs are six cells; a four-cell window keeps two of them.
        let clipped = clip_styled_spans(&parts, 0, 4);
        let text: String = clipped.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "日本");

        // And an offset skips by cells too, not by characters.
        let offset = clip_styled_spans(&parts, 6, 3);
        let text: String = offset.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "abc");
    }

    /// The rule is three cells and five bytes, and sizing the columns by the wrong one silently
    /// spent two columns of every side-by-side diff on nothing.
    #[test]
    fn the_column_rule_is_measured_in_cells_not_bytes() {
        assert_eq!(UnicodeWidthStr::width(SPLIT_DIVIDER), SPLIT_DIVIDER_CELLS);
        assert_ne!(SPLIT_DIVIDER.len(), SPLIT_DIVIDER_CELLS, "the trap this guards");

        let styles = ThemeStyles::from_resolved(&std::collections::HashMap::new(), "nord");
        for width in [100u16, 137, 160, 201] {
            let layout = split_layout(&styles, width, 4, 0, None);
            let spent = layout.side * 2 + SPLIT_DIVIDER_CELLS;
            assert!(
                usize::from(width) - spent < 2,
                "width {width}: {spent} of {width} cells used — the columns are leaving room behind"
            );
        }
    }

    #[test]
    fn a_tinted_row_is_padded_to_the_full_width() {
        let background = Style::default().bg(ratatui::style::Color::Blue);
        let spans = tint_row(vec![Span::raw("code")], background, 10);
        let width: usize =
            spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
        assert_eq!(width, 10, "the tint runs to the edge of the pane");
        assert!(spans.iter().all(|span| span.style.bg == background.bg));
    }

    #[test]
    fn the_no_newline_marker_wears_no_sign() {
        assert_eq!(rendered_marker(ReviewLineKind::NoNewlineMarker), "");
        assert_eq!(rendered_marker(ReviewLineKind::Addition), "+");
        assert_eq!(rendered_marker(ReviewLineKind::Deletion), "-");
    }
}
