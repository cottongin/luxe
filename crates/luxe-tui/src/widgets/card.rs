//! card — something someone said, as one object among other rows.
//!
//! A rounded border in the card's tone, its header set into the top edge the way a dialog carries
//! its title, and a faint wash of the same tone behind it. Guided Review draws comments on a diff
//! this way and `/github` draws a conversation this way, so a comment looks like a comment wherever
//! it turns up.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// The text width inside a card drawn in `width` cells from `indent`: a border and a space each
/// side.
pub fn card_inner_width(width: usize, indent: usize) -> usize {
    width.saturating_sub(indent + 4).max(10)
}

/// A card `width` cells wide, `indent` cells in, with `title` in its top edge and `body` inside.
///
/// A framed card rather than a bar down the left side: among rows already tinted green and red, a
/// bar reads as one more gutter mark rather than as someone having said something.
///
/// Args: `(tone, fill, title, body, width, indent)` — `tone` colors the frame, `fill` washes the
/// inside.
pub fn card(
    tone: Style,
    fill: Style,
    title: Vec<Span<'static>>,
    body: Vec<Line<'static>>,
    width: usize,
    indent: usize,
) -> Vec<Line<'static>> {
    let inner = card_inner_width(width, indent);
    // The frame's top and bottom rows stay on the canvas: a terminal draws a border through the
    // middle of its cell, so a tinted border row spills half a row of wash outside the frame.
    let frame = |text: String| Span::styled(text, tone);
    let edge = |text: String| Span::styled(text, tone.patch(fill));
    let margin = || Span::raw(" ".repeat(indent));
    let mut lines = Vec::with_capacity(body.len() + 2);

    // `╭─ R2   unsent ─────╮`: the title is clipped before the corner rather than pushing it off
    let title = crate::widgets::wrap_line(&Line::from(title), inner.saturating_sub(1).max(1))
        .into_iter()
        .next()
        .map_or_else(Vec::new, |line| line.spans);
    let title_width: usize = title.iter().map(|span| span.content.width()).sum();
    let mut top = vec![margin(), frame("\u{256d}\u{2500} ".into())];
    top.extend(title);
    top.push(frame(format!(
        " {}\u{256e}",
        "\u{2500}".repeat(inner.saturating_sub(title_width + 1))
    )));
    lines.push(Line::from(top));

    for row in body {
        let used: usize = row.spans.iter().map(|span| span.content.width()).sum();
        let mut spans = vec![margin(), edge("\u{2502} ".into())];
        spans.extend(row.spans.into_iter().map(|span| {
            let style = if span.style.bg.is_none() { span.style.patch(fill) } else { span.style };
            Span::styled(span.content, style)
        }));
        spans.push(Span::styled(" ".repeat(inner.saturating_sub(used)), fill));
        spans.push(edge(" \u{2502}".into()));
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(vec![
        margin(),
        frame(format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(inner + 2))),
    ]));
    lines
}
