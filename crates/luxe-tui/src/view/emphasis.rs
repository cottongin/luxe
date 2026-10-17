//! Intra-line diff emphasis — dressing a replaced line in the spans that actually changed.
//!
//! [`luxe_core::intraline`] decides *what* changed; this decides how one line carries that answer.
//! Four renderers need it — tool cards unified and side by side, Guided Review the same two ways —
//! and they differ only in whether they then clip, wrap or tint the result, so the split itself lives
//! here once. Ranges arrive from a computation the renderer did not do; an unusable one leaves the
//! line plain rather than panicking on a byte boundary mid-frame.

use ratatui::{style::Style, text::Span};
use std::ops::Range;

/// Splits `text` into styled parts, with each range in `changed` wearing `emphasis`.
///
/// Borrows from `text` so a caller can clip the parts to a column window before allocating. Returns
/// one plain part when `changed` is empty or cannot be applied.
pub(super) fn parts<'a>(
    text: &'a str,
    changed: &[Range<usize>],
    base: Style,
    emphasis: Style,
) -> Vec<(&'a str, Style)> {
    if !applicable(text, changed) {
        return vec![(text, base)];
    }
    let mut parts = Vec::with_capacity(changed.len() * 2 + 1);
    let mut at = 0usize;
    for range in changed {
        if range.start > at {
            parts.push((&text[at..range.start], base));
        }
        parts.push((&text[range.clone()], emphasis));
        at = range.end;
    }
    if at < text.len() {
        parts.push((&text[at..], base));
    }
    parts
}

/// The same split, as owned spans for a renderer that wraps or keeps them past the source.
pub(super) fn spans(
    text: &str,
    changed: &[Range<usize>],
    base: Style,
    emphasis: Style,
) -> Vec<Span<'static>> {
    parts(text, changed, base, emphasis)
        .into_iter()
        .map(|(slice, style)| Span::styled(slice.to_string(), style))
        .collect()
}

/// Whether `changed` can slice `text`: ascending, non-empty, disjoint, in bounds, on char boundaries.
fn applicable(text: &str, changed: &[Range<usize>]) -> bool {
    let mut previous = 0usize;
    for range in changed {
        if range.start < previous || range.start >= range.end || range.end > text.len() {
            return false;
        }
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return false;
        }
        previous = range.end;
    }
    !changed.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn base() -> Style {
        Style::new().fg(Color::Red)
    }

    fn emphasis() -> Style {
        Style::new().fg(Color::Red).bg(Color::Blue).add_modifier(Modifier::BOLD)
    }

    /// A range built rather than written, so a deliberately malformed one is test data instead of a
    /// literal clippy reads as an iteration mistake.
    fn span(start: usize, end: usize) -> Range<usize> {
        Range { start, end }
    }

    /// Several spans in one line, each keeping the plain text around it plain.
    #[test]
    fn every_changed_span_is_emphasized_and_nothing_else_is() {
        let text = "takes a string like '2'";
        let at = |needle: &str| {
            let start = text.find(needle).expect("needle is in the line");
            start..start + needle.len()
        };
        let changed = [at("string"), at("'2'")];

        assert_eq!(
            parts(text, &changed, base(), emphasis()),
            [("takes a ", base()), ("string", emphasis()), (" like ", base()), ("'2'", emphasis()),]
        );
    }

    /// A span at either end of the line does not produce an empty part beside it.
    #[test]
    fn a_span_flush_against_an_edge_leaves_no_empty_part() {
        assert_eq!(
            parts("abcdef", &[span(0, 3)], base(), emphasis()),
            [("abc", emphasis()), ("def", base())]
        );
        assert_eq!(
            parts("abcdef", &[span(3, 6)], base(), emphasis()),
            [("abc", base()), ("def", emphasis())]
        );
        assert_eq!(parts("abc", &[span(0, 3)], base(), emphasis()), [("abc", emphasis())]);
    }

    /// Ranges come from elsewhere, so a bad set degrades to a plain line instead of panicking.
    #[test]
    fn an_unusable_range_set_renders_plain() {
        // `é` occupies bytes 7..9, so 7..8 lands inside it.
        let text = "let café = 1;";
        let unusable = [
            vec![],                       // nothing changed
            vec![span(4, 2)],             // inverted
            vec![span(3, 3)],             // empty
            vec![span(0, 99)],            // past the end
            vec![span(7, 8)],             // splits a character
            vec![span(0, 4), span(2, 6)], // overlapping
        ];
        for changed in unusable {
            assert_eq!(
                parts(text, &changed, base(), emphasis()),
                [(text, base())],
                "{changed:?} should have rendered plain"
            );
        }
    }

    #[test]
    fn owned_spans_carry_the_same_split() {
        let spans = spans("old value", &[span(0, 3)], base(), emphasis());

        assert_eq!(spans[0].content.as_ref(), "old");
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].content.as_ref(), " value");
        assert_eq!(spans[1].style, base());
    }
}
