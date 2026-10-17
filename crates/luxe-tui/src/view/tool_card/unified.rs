//! Unified card diff — one column, `<sign><line> <code>`.
//!
//! The default shape, and the only one a narrow card can hold. Context is syntax-highlighted so a
//! diff reads as code; a replaced line keeps its add/remove color and marks the spans that actually
//! changed, so a one-token edit reads as one token rather than two rewritten lines. A long line wraps
//! under its gutter with a `↳` rather than running off the edge, because a card has no way to scroll
//! sideways.

use super::rows::{Row, RowKind};
use super::{CardContext, DiffRow};
use crate::view::emphasis;
use ratatui::text::{Line, Span};

pub(super) fn render(rows: &[Row], language: Option<&str>, ctx: &CardContext<'_>) -> Vec<DiffRow> {
    let number_width = super::rows::number_width(rows);
    let emphasized = super::rows::intraline_emphasis(rows);
    // sign + number + the space before the code.
    let gutter_width = number_width + 2;
    let code_width = super::code_width(usize::from(ctx.width), super::row_lead_width(gutter_width));

    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let lines = match row {
                Row::Note(text) => super::note_lines(text, ctx),
                Row::Elision => {
                    vec![Line::from(Span::styled(
                        format!("{:>gutter_width$}", crate::glyphs::Mark::Elision.glyph()),
                        ctx.styles.muted,
                    ))]
                }
                Row::Source(source) => {
                    let style = match source.kind {
                        RowKind::Added => ctx.styles.success,
                        RowKind::Removed => ctx.styles.error,
                        RowKind::Context => ctx.styles.text,
                    };
                    let gutter = Span::styled(
                        format!(
                            "{}{:>number_width$} ",
                            source.kind.marker(),
                            source.number.map_or_else(String::new, |number| number.to_string())
                        ),
                        ctx.styles.muted,
                    );
                    let code = match source.kind {
                        RowKind::Context => super::highlighted_spans(&source.code, language, ctx),
                        _ => emphasis::spans(
                            &source.code,
                            emphasized.get(&index).map_or(&[][..], Vec::as_slice),
                            style,
                            ctx.styles.diff_emphasis(style, source.kind == RowKind::Added),
                        ),
                    };
                    super::wrapped_rows(gutter, code, code_width, gutter_width, ctx)
                }
            };
            let changed = matches!(row, Row::Source(source) if source.kind != RowKind::Context);
            DiffRow { lines, changed }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tool_card::test_support::{context, plain, theme};
    use ratatui::style::Modifier;

    #[test]
    fn a_row_reads_as_gutter_then_code() {
        let styles = theme();
        let rows = super::super::rows::parse(" 11 fn main() {\n-12     old();\n+12     new();");
        let rendered = render(&rows, Some("rs"), &context(&styles, 80));

        assert_eq!(rendered.len(), 3, "one group per row");
        assert_eq!(plain(&rendered[0].lines[0]), " 11 fn main() {");
        assert_eq!(plain(&rendered[1].lines[0]), "-12     old();");
        assert_eq!(plain(&rendered[2].lines[0]), "+12     new();");

        // The gutter is muted so the eye lands on the code, not the numbering.
        assert_eq!(rendered[1].lines[0].spans[0].style.fg, styles.muted.fg);
        assert_eq!(rendered[1].lines[0].spans[1].style.fg, styles.error.fg);
        assert_eq!(rendered[2].lines[0].spans[1].style.fg, styles.success.fg);
    }

    /// Context is code, so it is highlighted; a changed line keeps its verdict color instead.
    #[test]
    fn context_is_highlighted_and_changed_lines_are_not() {
        let styles = theme();
        let rows = super::super::rows::parse(" 1 fn main() { let x = 1; }\n+2 fn other() {}");
        let rendered = render(&rows, Some("rs"), &context(&styles, 80));

        let context_colors: std::collections::HashSet<_> =
            rendered[0].lines[0].spans.iter().skip(1).map(|span| span.style.fg).collect();
        assert!(context_colors.len() > 1, "context row is syntax highlighted: {context_colors:?}");

        let added_colors: std::collections::HashSet<_> =
            rendered[1].lines[0].spans.iter().skip(1).map(|span| span.style.fg).collect();
        assert_eq!(added_colors, [styles.success.fg].into(), "an addition reads as an addition");
    }

    /// Only what changed is marked, in bold on either side and never struck through, since the old
    /// text is what a reader compares the new against. The shared prefix and suffix stay plain.
    #[test]
    fn a_replacement_marks_only_the_changed_span() {
        let styles = theme();
        let rows = super::super::rows::parse("-1 let a = one();\n+1 let a = two();");
        let rendered = render(&rows, Some("rs"), &context(&styles, 80));

        let wearing = |row: usize, modifier: Modifier| -> String {
            rendered[row].lines[0]
                .spans
                .iter()
                .filter(|span| span.style.add_modifier.contains(modifier))
                .map(|span| span.content.as_ref())
                .collect()
        };
        assert_eq!(wearing(0, Modifier::BOLD), "one", "what left is bold");
        assert_eq!(wearing(1, Modifier::BOLD), "two", "what arrived is bold");
        assert_eq!(wearing(0, Modifier::CROSSED_OUT), "", "nothing is struck through");
        assert_eq!(wearing(1, Modifier::CROSSED_OUT), "");

        // And the changed span alone wears the deeper wash that makes the eye land on it.
        let washed: Vec<_> =
            rendered[1].lines[0].spans.iter().filter(|span| span.style.bg.is_some()).collect();
        assert_eq!(washed.len(), 1, "one washed span");
        assert_eq!(washed[0].content.as_ref(), "two");
        assert_eq!(washed[0].style.bg, styles.diff_add_emphasis.bg);
    }

    /// Two edits in one line are two spans, with the text between them left alone — the whole point of
    /// a word-level diff over a single contiguous "the middle differs" range.
    #[test]
    fn separate_edits_in_one_line_are_marked_separately() {
        let styles = theme();
        let rows = super::super::rows::parse(
            "-1 /// takes a string like '2'\n+1 /// takes a number like 2",
        );
        let rendered = render(&rows, Some("rs"), &context(&styles, 80));

        let marked: Vec<&str> = rendered[0].lines[0]
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(marked, ["string", "'2'"], "two spans, not one blob over `string like '2'`");
    }

    /// A card cannot scroll sideways, so a long line wraps under its gutter and nothing is lost.
    #[test]
    fn a_long_line_wraps_under_its_gutter() {
        let styles = theme();
        let code = "let values = compute(alpha, beta, gamma, delta, epsilon, zeta, eta, theta);";
        let rows = super::super::rows::parse(&format!("+42 {code}"));
        let rendered = render(&rows, Some("rs"), &context(&styles, 30));

        assert!(rendered[0].lines.len() > 1, "the line wrapped");
        assert!(
            rendered[0].lines.iter().all(|line| line.width() <= 30),
            "no row overflows the card"
        );
        let joined: String = rendered[0]
            .lines
            .iter()
            .map(plain)
            .map(|text| text.trim_start_matches([' ', '↳']).to_string())
            .collect();
        assert!(joined.ends_with("theta);"), "every character survives: {joined}");
        assert!(plain(&rendered[0].lines[1]).contains('↳'), "continuations are marked");
    }

    #[test]
    fn an_elision_is_a_single_muted_marker() {
        let styles = theme();
        let rows = super::super::rows::parse(" 10 kept\n    ...\n 40 kept");
        let rendered = render(&rows, None, &context(&styles, 80));
        assert_eq!(plain(&rendered[1].lines[0]).trim(), "⋯");
        assert_eq!(rendered[1].lines[0].spans[0].style.fg, styles.muted.fg);
    }
}
