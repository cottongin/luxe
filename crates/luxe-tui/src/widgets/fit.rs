//! Fit — trimming text to a cell budget.
//!
//! A terminal measures in cells, not characters: one CJK glyph costs two columns and a combining
//! mark costs none. Counting characters instead is what makes a column's right edge go ragged the
//! first time someone opens a file whose name is not ASCII — so every surface that prints into a
//! fixed width comes here rather than reaching for `chars().take(n)`.
//!
//! Every function here guarantees a result no wider than `max_width` and leaves text that already
//! fits completely alone — an ellipsis means something was actually dropped.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Trims `text` to `max_width` cells, marking the loss with a trailing `…`.
///
/// Used wherever the front of the string is the part worth keeping: a session label, a model id,
/// a todo subject.
///
/// ```ignore
/// assert_eq!(truncate_end("session-alpha", 8), "session…");
/// assert_eq!(truncate_end("short", 8), "short");
/// ```
pub fn truncate_end(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let budget = max_width - 1; // the `…` costs one cell
    let mut fitted = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let cell_width = character.width().unwrap_or(0);
        if used + cell_width > budget {
            break;
        }
        fitted.push(character);
        used += cell_width;
    }
    fitted.push('…');
    fitted
}

/// Trims `text` to `max_width` cells from the *left*, marking the loss with a leading `…`.
///
/// For paths and anything else whose tail identifies it: `…/luxe-tui/src/widgets/fit.rs` still
/// says which file it is, where the head-trimmed form would not.
///
/// Cuts wherever the budget runs out, mid-component included. When whole path components must
/// survive, the Guided Review context panel keeps its own component-aware variant.
///
/// ```ignore
/// assert_eq!(truncate_start("crates/luxe-tui/src/lib.rs", 12), "…c/lib.rs");
/// ```
pub fn truncate_start(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut fitted = String::new();
    let mut used = 1usize; // the `…` costs one cell
    for character in text.chars().rev() {
        let cell_width = character.width().unwrap_or(0);
        if used + cell_width > max_width {
            break;
        }
        fitted.insert(0, character);
        used += cell_width;
    }
    format!("…{fitted}")
}

/// Shortens a name from the middle, keeping its start and its extension, in cells, never cutting a
/// zero-width mark from its letter. (Not full graphemes: a joined emoji sequence can be split, which
/// a file name all but never holds.)
///
/// A left cut (`…ings/status.rs`) loses what disambiguates two files with the same tail; a right
/// cut loses the extension, which is how a reader tells a test from its subject. Both ends are
/// kept only when the whole extension and some of the start fit; otherwise this is
/// [`truncate_end`].
///
/// ```ignore
/// assert_eq!(truncate_middle("status_bar_view.rs", 10), "statu…w.rs");
/// ```
pub(crate) fn truncate_middle(text: &str, max_width: usize) -> String {
    if text.width() <= max_width || max_width < 5 {
        return truncate_end(text, max_width);
    }
    let budget = max_width - 1; // the `…` costs one cell
    let extension = text.rfind('.').filter(|&at| at > 0).map_or(0, |at| text[at..].width());
    let tail_width = (budget / 2).max(extension);
    if tail_width >= budget {
        return truncate_end(text, max_width);
    }
    let head = front_within(text, budget - tail_width);
    let tail = back_within(text, tail_width);
    format!("{head}…{tail}")
}

/// The longest start of `text` within `budget` cells, with any zero-width marks on its last
/// character.
fn front_within(text: &str, budget: usize) -> &str {
    let mut used = 0usize;
    for (at, character) in text.char_indices() {
        let width = character.width().unwrap_or(0);
        if width > 0 && used + width > budget {
            return &text[..at];
        }
        used += width;
    }
    text
}

/// The longest end of `text` within `budget` cells, never starting on a zero-width mark whose
/// character was left behind.
fn back_within(text: &str, budget: usize) -> &str {
    let mut used = 0usize;
    let mut start = text.len();
    for (at, character) in text.char_indices().rev() {
        let width = character.width().unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        start = at;
    }
    let kept = &text[start..];
    kept.trim_start_matches(|character: char| character.width() == Some(0))
}

/// Fits a path into `max_width` cells by dropping whole leading components behind a `…/` prefix.
///
/// Cutting at directory boundaries rather than mid-name is what keeps a truncated path readable:
/// `…/guided_review/context.rs` still says where the file lives, where a blind cut would leave a
/// half-word. Falls back to [`truncate_start`] when even the filename overflows.
pub(crate) fn elide_path_components(path: &str, max_width: usize) -> String {
    if path.width() <= max_width {
        return path.to_string();
    }
    // below the `…/` prefix's two cells there is no room for a whole component, only a marker
    if max_width < 2 {
        return truncate_start(path, max_width);
    }
    let component_budget = max_width - 2;
    let mut kept = String::new();
    for component in path.rsplit('/') {
        let candidate =
            if kept.is_empty() { component.to_string() } else { format!("{component}/{kept}") };
        if candidate.width() > component_budget {
            break;
        }
        kept = candidate;
    }
    if !kept.is_empty() {
        return format!("…/{kept}");
    }
    truncate_start(path.rsplit('/').next().unwrap_or(path), max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wide name is cut in cells, so the result fits however many columns its glyphs take.
    ///
    /// The break this catches: the dock's middle elision counted characters, so a run of CJK
    /// glyphs came out wider than its budget and spilled into the column beside it.
    #[test]
    fn middle_elision_fits_wide_names() {
        let name = "界界界界界界界界.rs";
        for width in 0..24 {
            assert!(truncate_middle(name, width).width() <= width, "budget {width}");
        }
    }

    /// The extension survives the cut whenever it and some of the start fit.
    #[test]
    fn middle_elision_keeps_the_extension_when_it_fits() {
        assert_eq!(truncate_middle("界界界界界界界界.rs", 12), "界界界…界.rs");
        assert_eq!(truncate_middle("status_bar_view.rs", 10), "statu…w.rs");
        assert_eq!(truncate_middle("short.rs", 10), "short.rs");
        assert!(truncate_middle("component.verylongextension", 8).ends_with('…'), "falls back");
        // a combining mark stays on its letter
        assert!(!truncate_middle("cafe\u{301}cafe\u{301}cafe\u{301}.txt", 9).contains("…\u{301}"));
    }

    #[test]
    fn text_that_fits_is_returned_untouched() {
        assert_eq!(truncate_end("abc", 3), "abc");
        assert_eq!(truncate_end("abc", 9), "abc");
        assert_eq!(truncate_start("abc", 3), "abc");
        assert_eq!(truncate_start("", 0), "");
    }

    #[test]
    fn an_ellipsis_replaces_what_was_dropped() {
        assert_eq!(truncate_end("abcdef", 4), "abc…");
        assert_eq!(truncate_start("abcdef", 4), "…def");
    }

    /// The budget is cells, so a wide glyph that would overhang is dropped whole rather than
    /// counted as one column and allowed to spill into the next.
    #[test]
    fn wide_glyphs_are_measured_in_cells() {
        assert_eq!("日本語".width(), 6);
        assert_eq!(truncate_end("日本語", 6), "日本語");
        // Budget 5 leaves 4 cells of content: two glyphs fit, the third would reach 6.
        assert_eq!(truncate_end("日本語", 5), "日本…");
        assert_eq!(truncate_start("日本語", 5), "…本語");
        for width in 0..10 {
            assert!(truncate_end("日本語ab", width).width() <= width, "end budget {width}");
            assert!(truncate_start("日本語ab", width).width() <= width, "start budget {width}");
        }
    }

    #[test]
    fn a_zero_budget_yields_nothing_rather_than_a_bare_ellipsis() {
        assert_eq!(truncate_end("abc", 0), "");
        assert_eq!(truncate_start("abc", 0), "");
    }

    /// A one-cell budget has room for the marker and nothing else.
    #[test]
    fn a_single_cell_holds_only_the_marker() {
        assert_eq!(truncate_end("abc", 1), "…");
        assert_eq!(truncate_start("abc", 1), "…");
    }

    #[test]
    fn path_truncation_keeps_the_filename_and_cuts_at_component_boundaries() {
        let path = "crates/luxe-tui/src/view/guided_review/context.rs";
        // Fits: returned unchanged.
        assert_eq!(elide_path_components(path, 60), path);
        // Overflow: cut behind a `…/` prefix at a directory boundary, filename preserved, budget kept.
        let short = elide_path_components(path, 30);
        assert!(short.starts_with("…/"), "has ellipsis prefix: {short}");
        assert!(short.ends_with("context.rs"), "filename preserved: {short}");
        assert!(!short.contains("luxe-tui"), "deep prefix elided: {short}");
        assert!(UnicodeWidthStr::width(short.as_str()) <= 30, "within budget: {short}");
        // Boundary cut never leaves a partial component after the ellipsis.
        let after_ellipsis = short.trim_start_matches('…');
        assert!(after_ellipsis.starts_with('/'), "cut at a component boundary: {short}");
        // A single filename wider than the budget is char-truncated behind a bare ellipsis.
        let long_name = "a_very_long_single_filename_component.rs";
        let clipped = elide_path_components(long_name, 12);
        assert!(clipped.starts_with('…'));
        assert!(UnicodeWidthStr::width(clipped.as_str()) <= 12, "within budget: {clipped}");
    }

    /// The narrowest budgets a context row can hand over still fit, with no underflow.
    ///
    /// The break this catches: one cell left for the path subtracted two for the `…/` prefix, which
    /// panics with overflow checks and returns an overwide path without them.
    #[test]
    fn component_elision_handles_zero_one_and_two_cells() {
        let path = "crates/luxe-tui/src/lib.rs";
        assert_eq!(elide_path_components(path, 0), "");
        assert_eq!(elide_path_components(path, 1), "…");
        let two = elide_path_components(path, 2);
        assert!(UnicodeWidthStr::width(two.as_str()) <= 2, "{two}");
    }
}
