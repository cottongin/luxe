//! panel — the shared chrome every bordered overlay wears.
//!
//! One frame, one title convention: rounded corners in the accent border color, an accent
//! nerd-font glyph naming the surface, then the label in bold. Panels differ in what they
//! *contain*, never in how they are framed — a reader should be able to identify a surface
//! from its icon before reading a word.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::glyphs::{icon_cells, icon_gap};
use crate::theme::styles::ThemeStyles;

/// Shared corner style for every full-box panel and overlay — the promptbox, pickers,
/// command palette, and dialogs all reference this so the TUI reads as one surface.
///
/// Flip this single value to restyle every bordered panel at once (e.g.
/// [`BorderType::Plain`] for square corners).
pub const PANEL_BORDER_TYPE: BorderType = BorderType::Rounded;

/// A full-box panel/overlay block in the shared surface language: all four borders with
/// the rounded [`PANEL_BORDER_TYPE`] corners. Callers layer their own `border_style`,
/// `title`, and `title_bottom` on top.
///
/// Prefer [`titled_panel`] — reach for this only where a panel genuinely has no title.
pub fn panel_block<'a>() -> Block<'a> {
    Block::default().borders(Borders::ALL).border_type(PANEL_BORDER_TYPE)
}

/// The shared overlay title: `" {glyph} {label} "`, glyph in `accent`, label in bold body text.
///
/// Pass a `glyph` constant so a surface's icon stays declared in one place.
pub fn panel_title<'a>(glyph: &str, label: &str, theme: &ThemeStyles) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {glyph}{}", icon_gap()), theme.accent),
        Span::styled(label.to_owned(), theme.text.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ])
}

/// Terminal-cell width of the [`panel_title`] for `glyph`/`label`.
///
/// Modals that size themselves to their content measure the title through this, so the box
/// can never be narrower than its own heading.
pub fn panel_title_width(glyph: &str, label: &str) -> usize {
    // The icon and its clearance, the label, and the literal space either side of the pair.
    icon_cells(glyph) + UnicodeWidthStr::width(label) + 2
}

/// A [`panel_block`] wearing the accent border and a [`panel_title`] — the one call a
/// bordered overlay makes.
///
/// The fill matters: an overlay renders over [`ratatui::widgets::Clear`], which resets cells to the
/// *terminal's* background rather than the theme's. Repainting the canvas color here keeps a light
/// theme from punching a dark hole through every modal in a dark terminal. Overlays that want a
/// raised surface layer their own `.style()` on top.
pub fn titled_panel<'a>(glyph: &str, label: &str, theme: &ThemeStyles) -> Block<'a> {
    // The glyph follows the border, so a panel tinted for the session's mode is tinted whole. In a
    // theme where the two are one color, which is every built-in, nothing moves.
    let title = Line::from(vec![
        Span::styled(format!(" {glyph}{}", icon_gap()), theme.border_accent),
        Span::styled(label.to_owned(), theme.text.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ]);
    panel_block().style(theme.background).border_style(theme.border_accent).title(title)
}

/// A key hint, ` Enter open · d draft · Esc cancel `: each key in the accent and what it does in
/// the body text, right-aligned. [`render_footer`] is how a modal shows one.
///
/// Segments are split on ` · `, and each one's first word is its key. A segment of one word that
/// ends in `…` is a state rather than a key — `opening…` — and so is a count like `3/12`; both
/// read as text.
pub fn key_hint<'a>(hint: &str, theme: &ThemeStyles) -> Line<'a> {
    let mut spans = vec![Span::raw(" ")];
    for (index, segment) in hint.trim().split(" · ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", theme.muted));
        }
        match segment.split_once(' ') {
            Some((key, words)) => {
                spans.push(Span::styled(key.to_owned(), theme.accent));
                spans.push(Span::styled(format!(" {words}"), theme.text));
            }
            None if segment.ends_with('…') || is_count(segment) => {
                spans.push(Span::styled(segment.to_owned(), theme.text))
            }
            None => spans.push(Span::styled(segment.to_owned(), theme.accent)),
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans).alignment(ratatui::layout::Alignment::Right)
}

/// A [`key_hint`] for a row of its own under a list, where it starts at the left.
pub fn key_hint_row<'a>(hint: &str, theme: &ThemeStyles) -> Line<'a> {
    key_hint(hint, theme).alignment(ratatui::layout::Alignment::Left)
}

// === Footer ===

/// Rows the [`render_footer`] takes inside a panel: its rule, then its hints.
pub const FOOTER_ROWS: u16 = 2;

/// Splits a panel's inside into its body and the footer's [`FOOTER_ROWS`] at the bottom.
///
/// Args: `(inner)` → `(body, footer)`. A panel too short for both gives the footer what there is.
pub fn split_footer(inner: Rect) -> (Rect, Rect) {
    let footer = FOOTER_ROWS.min(inner.height);
    let body = Rect { height: inner.height - footer, ..inner };
    let footer = Rect { y: inner.y + body.height, height: footer, ..inner };
    (body, footer)
}

/// Every modal's footer: a muted rule across the panel, then the keys on the left and `count`,
/// where the cursor stands, on the right.
///
/// The keys live inside the panel rather than on its border, so the frame stays a frame and every
/// modal puts them in the same place. `count` is any short right-hand note: `3/12`, or the
/// thinking picker's model.
pub fn render_footer(
    frame: &mut Frame,
    area: Rect,
    hint: &str,
    count: Option<&str>,
    theme: &ThemeStyles,
) {
    if area.height == 0 {
        return;
    }
    let keys = Rect { y: area.bottom() - 1, height: 1, ..area };
    if area.height > 1 {
        let rule = "─".repeat(usize::from(area.width));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule, theme.border_muted))),
            Rect { y: keys.y - 1, height: 1, ..area },
        );
    }
    let count = count.filter(|count| !count.is_empty());
    // Room for the keys: the row, less the count and a gap before it, and a cell at each end
    let room = usize::from(area.width)
        .saturating_sub(count.map_or(0, |count| UnicodeWidthStr::width(count) + 3))
        .saturating_sub(2);
    frame.render_widget(Paragraph::new(key_hint_row(&fit_hint(hint, room), theme)), keys);
    if let Some(count) = count {
        let line = Line::from(Span::styled(format!("{count} "), theme.muted))
            .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(Paragraph::new(line), keys);
    }
}

/// Draws `block` over `area` with [`render_footer`] on its last rows, and returns what is left
/// for the body: the block's own inside, padding and all, above the rule.
///
/// The rule spans the border's inside whatever the padding, so it meets the frame the way the
/// palette's search rule does.
pub fn render_footed(
    frame: &mut Frame,
    block: Block<'_>,
    area: Rect,
    hint: &str,
    count: Option<&str>,
    theme: &ThemeStyles,
) -> Rect {
    let body = footed_body(&block, area);
    frame.render_widget(block, area);
    let (_, footer) = split_footer(panel_block().inner(area));
    render_footer(frame, footer, hint, count, theme);
    body
}

/// The body [`render_footed`] will leave, for a panel that has to know it before drawing.
pub fn footed_body(block: &Block<'_>, area: Rect) -> Rect {
    let padded = block.inner(area);
    let (_, footer) = split_footer(panel_block().inner(area));
    Rect { height: footer.y.saturating_sub(padded.y).min(padded.height), ..padded }
}

/// `hint` with keys dropped from the end until it fits `room`, keeping the first, and a closing
/// `Esc` while there is room for it: on a narrow terminal the keys ran into the count beside them,
/// and the way out is the one key a reader must always be able to find.
fn fit_hint(hint: &str, room: usize) -> String {
    let mut keys: Vec<&str> = hint.trim().split(" · ").collect();
    let width = |keys: &[&str]| UnicodeWidthStr::width(keys.join(" · ").as_str());
    while keys.len() > 1 && width(&keys) > room {
        let last_is_esc = keys.last().is_some_and(|key| key.starts_with("esc "));
        let at = if last_is_esc && keys.len() > 2 { keys.len() - 2 } else { keys.len() - 1 };
        keys.remove(at);
    }
    keys.join(" · ")
}

/// `3/12`: where a cursor stands in a list, not a key.
fn is_count(segment: &str) -> bool {
    segment.split_once('/').is_some_and(|(at, of)| {
        [at, of].iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// `text` as spans, with each `` `quoted` `` stretch drawn in the inline-code style and its
/// backticks dropped, the way the transcript draws inline code. An unmatched backtick stays.
pub fn code_spans<'a>(text: &str, base: Style, theme: &ThemeStyles) -> Vec<Span<'a>> {
    let pieces: Vec<&str> = text.split('`').collect();
    // An odd count of backticks leaves the last one unmatched: it is a character, not markup
    let unmatched = pieces.len().is_multiple_of(2);
    let mut spans = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let last = index + 1 == pieces.len();
        if unmatched && last {
            spans.push(Span::styled(format!("`{piece}"), base));
        } else if index % 2 == 1 {
            spans.push(Span::styled((*piece).to_owned(), theme.md_code));
        } else if !piece.is_empty() {
            spans.push(Span::styled((*piece).to_owned(), base));
        }
    }
    spans
}

#[cfg(test)]
mod tests;
