//! Tab stops — how wide a tab is, everywhere luxe draws one.
//!
//! A tab is the one character the two halves of a render disagree about. `unicode-width` scores it
//! zero, ratatui's buffer drops it, and the terminal would advance the cursor to its own stop if it
//! ever saw one. So anything that measures a line before drawing it — a diff column, a card gutter,
//! a padded wash — is measuring a different string from the one that lands on screen, and the
//! picture drifts from the arithmetic by up to [`TAB_WIDTH`] cells per tab.
//!
//! One owner: text becomes drawable *here*, once, and no tab survives into a measurement. Every
//! surface that lays out source — the guided review, the peeked diff, a tool card — goes through
//! [`display_text`] on the way in.

use unicode_width::UnicodeWidthChar;

/// Cells a tab advances to, everywhere luxe draws one.
///
/// Four rather than eight because a diff already spends cells on two line-number columns and a
/// sign, and a Go or Make file indented four levels at eight cells apiece has nothing left for the
/// code. The value is fixed rather than configurable: it decides screen geometry, not file content.
pub(crate) const TAB_WIDTH: usize = 4;

/// One line of text as the terminal will actually draw it, with every tab already spent.
///
/// Expansion is to tab stops rather than a fixed run of spaces, so a tab means what it means in the
/// editor the file came from, and columns are measured from the start of the line — which is where
/// an editor measures them too, so a gutter prepended afterwards cannot shift the indentation.
/// Borrows when there is nothing to expand, which is nearly always.
///
/// Display only. Whatever holds the original keeps the bytes it captured: a review's pinned lines
/// are re-emitted into a patch by `commit_review_chapters`, and a patch whose tabs became spaces
/// does not apply to the file it came from.
pub(crate) fn display_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\t') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut expanded = String::with_capacity(text.len() + TAB_WIDTH);
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\t' {
            let advance = TAB_WIDTH - (column % TAB_WIDTH);
            expanded.extend(std::iter::repeat_n(' ', advance));
            column += advance;
        } else {
            expanded.push(character);
            column += UnicodeWidthChar::width(character).unwrap_or(0);
        }
    }
    std::borrow::Cow::Owned(expanded)
}

/// The width one line occupies once drawn, tabs included.
pub(crate) fn display_width(text: &str) -> usize {
    display_text(text)
        .chars()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}

/// The part of `text` drawn in display cells `range`, as the original bytes: a tab comes back as a
/// tab, which is what a copy of source should hold.
///
/// A tab the range only partly covers is kept, since it is one character however many cells it
/// spans; a wide glyph straddling either edge is dropped, the way [`crate::selection::slice_columns`]
/// drops one.
pub(crate) fn source_slice(text: &str, range: std::ops::Range<usize>) -> &str {
    let mut column = 0usize;
    let (mut from, mut to) = (None, 0usize);
    for (offset, character) in text.char_indices() {
        let width = match character {
            '\t' => TAB_WIDTH - (column % TAB_WIDTH),
            _ => UnicodeWidthChar::width(character).unwrap_or(0),
        };
        let fits = column + width <= range.end || (character == '\t' && column < range.end);
        if column >= range.end || !fits {
            break;
        }
        if column >= range.start {
            from.get_or_insert(offset);
            to = offset + character.len_utf8();
        }
        column += width;
    }
    from.map_or("", |from| &text[from..to])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    /// A copy of drawn cells gives back the bytes that drew them: tabs stay tabs.
    #[test]
    fn a_slice_of_cells_is_the_source_that_drew_them() {
        assert_eq!(source_slice("\tone", 0..7), "\tone");
        assert_eq!(source_slice("\tone", 4..6), "on", "cells 4 and 5 are the o and the n");
        assert_eq!(source_slice("a\tb", 2..4), "", "inside the tab, past its start");
        assert_eq!(source_slice("a\tb", 0..2), "a\t", "a tab half covered still comes whole");
        assert_eq!(source_slice("plain", 1..99), "lain");
        assert_eq!(source_slice("plain", 9..12), "");
    }

    /// A wide glyph is all or nothing at either edge.
    #[test]
    fn a_slice_drops_a_wide_glyph_it_cuts() {
        assert_eq!(source_slice("a\u{65e5}b", 0..2), "a");
        assert_eq!(source_slice("a\u{65e5}b", 2..4), "b", "starts inside the glyph");
        assert_eq!(source_slice("a\u{65e5}b", 1..3), "\u{65e5}");
    }

    /// Tabs advance to the next stop rather than becoming a fixed run, so a line indented with
    /// tabs lands where the editor it came from put it.
    #[test]
    fn a_tab_advances_to_the_next_stop() {
        assert_eq!(display_text("\tone"), "    one");
        assert_eq!(display_text("\t\ttwo"), "        two");
        assert_eq!(display_text("ab\tc"), "ab  c", "two cells in, the stop is two cells away");
        assert_eq!(display_text("abcd\te"), "abcd    e", "flush against a stop takes a full one");
        // The second tab is what proves the column is *carried* through the line rather than
        // recomputed: it lands on a stop the first one moved the text off.
        assert_eq!(display_text("a\tb\tc"), "a   b   c");
    }

    /// The common line has no tab in it and must not be copied to find that out.
    #[test]
    fn a_line_without_tabs_is_borrowed() {
        assert!(matches!(display_text("    let total = a + b;"), Cow::Borrowed(_)));
        assert!(matches!(display_text("\tlet total = a + b;"), Cow::Owned(_)));
    }

    /// The whole point: what the width arithmetic thinks a line is worth has to be what the
    /// terminal draws. `unicode-width` scores a tab zero and ratatui drops it, so the raw string
    /// measured one cell short per tab and every row tint stopped there.
    #[test]
    fn display_width_counts_what_the_terminal_will_draw() {
        let raw = "\t\t<span>";
        assert_eq!(display_width(raw), 8 + "<span>".len());
        assert_eq!(
            display_width(raw),
            display_text(raw).chars().count(),
            "every drawn character is one cell here, so the count is the width"
        );
    }

    /// Wide glyphs still count for two, and a tab after one lands on the right stop.
    #[test]
    fn expansion_measures_wide_glyphs_in_cells() {
        assert_eq!(display_text("\u{65e5}\tx"), "\u{65e5}  x", "two cells in, two to the stop");
        assert_eq!(display_width("\u{65e5}\tx"), 5);
    }
}
