//! Messages — a user's turn with its chips, reasoning and its peek, and system prose.

use super::tool::safe_tool_text;
use super::{layout_lines, BlockContext};
use crate::app::{user_message_fits_preview, USER_MESSAGE_PREVIEW_LINES};
use crate::glyphs::{icon_label, icon_prefix, Icon};
use crate::markdown::{layout_markdown_in, MarkdownBlockKind, MarkdownLine, MarkdownVoice};
use crate::theme::styles::ThemeStyles;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// A user's turn: the message, a note when it never reached the engine, what it attached, and the
/// handles of a message long enough to preview.
pub(super) fn user_lines(
    text: &str,
    attached: &[String],
    chips: &[std::ops::Range<usize>],
    style: Style,
    ctx: &BlockContext<'_>,
) -> Vec<MarkdownLine> {
    let (styles, never_sent, expanded) = (ctx.styles, ctx.never_sent, ctx.expanded);
    // No glyph — user turns are distinguished by their background band (see render).
    let previewed = !user_message_fits_preview(text);
    let mut lines = user_body_lines(text, chips, expanded, style, styles);
    if never_sent {
        lines.push(Line::from(Span::styled(
            format!(
                "   {}",
                icon_label(Icon::NeverSent.glyph(), "never sent · engine unavailable")
            ),
            styles.warning,
        )));
    }
    let image_count = attached.iter().filter(|item| Icon::Image.leads(item)).count();
    let file_count = attached.len().saturating_sub(image_count);
    if file_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "   {}",
                icon_label(Icon::Attachment.glyph(), &format!("{file_count} file(s) attached"))
            ),
            styles.muted,
        )));
    }
    if image_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "   {}",
                icon_label(Icon::Image.glyph(), &format!("{image_count} image(s) attached"))
            ),
            styles.muted,
        )));
    }
    let mut laid = layout_lines(lines, MarkdownBlockKind::Prose);
    // A message long enough to be previewed hides the rest behind its first row, and while
    // collapsed also behind the footer counting what is missing. Those are the rows to click;
    // the body between them is text.
    if previewed {
        mark_handles(&mut laid, !expanded);
    }
    laid
}

/// Renders a user message, capping a long one at [`USER_MESSAGE_PREVIEW_LINES`] with a muted
/// footer saying how much is hidden. A submitted message carries its pasted payload verbatim, so
/// without the cap a single paste can bury the rest of the conversation.
fn user_body_lines(
    text: &str,
    chips: &[std::ops::Range<usize>],
    expanded: bool,
    style: Style,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    if expanded || user_message_fits_preview(text) {
        return chipped_lines(text, chips, style, styles);
    }
    let total = text.lines().count();
    let preview = text.lines().take(USER_MESSAGE_PREVIEW_LINES).collect::<Vec<_>>().join("\n");
    let mut lines = chipped_lines(&preview, chips, style, styles);
    lines.push(Line::from(Span::styled(
        format!(
            "{} +{} more lines",
            crate::glyphs::Mark::Elision,
            total - USER_MESSAGE_PREVIEW_LINES
        ),
        styles.muted,
    )));
    lines
}

/// A message's lines, with each reference set apart from the sentence around it.
///
/// A reference takes whichever ink its sentence does not. In the composer you type in body text and
/// a chip is the accent on a tint; a sent message is already the accent, where the same tint all but
/// vanished, so here a reference is the body ink in bold. Either way `acme/todo#12` reads as a
/// thing you pointed at rather than as more of your sentence. A range that does not fall on this
/// text's character boundaries is drawn as plain text rather than trusted.
pub(super) fn chipped_lines(
    text: &str,
    chips: &[std::ops::Range<usize>],
    style: Style,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let chip_style = style.patch(styles.text).add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    let mut start = 0usize;
    for line in text.split('\n') {
        let end = start + line.len();
        let mut spans = Vec::new();
        let mut at = start;
        for chip in chips.iter().filter(|chip| chip.start >= start && chip.end <= end) {
            if chip.start < at
                || !text.is_char_boundary(chip.start)
                || !text.is_char_boundary(chip.end)
            {
                continue;
            }
            if chip.start > at {
                spans.push(Span::styled(text[at..chip.start].to_string(), style));
            }
            spans.push(Span::styled(text[chip.clone()].to_string(), chip_style));
            at = chip.end;
        }
        if at < end || spans.is_empty() {
            spans.push(Span::styled(text[at..end].to_string(), style));
        }
        lines.push(Line::from(spans));
        start = end + 1;
    }
    lines
}

/// Renders model reasoning as markdown in the muted italic reasoning voice.
///
/// No label: the muted italic voice already reads as reasoning rather than an answer, and a
/// literal "thinking" banner above every block is noise the reader has to skip past. Reasoning is
/// markdown in practice — GPT models lead each section with a `**title**` and name symbols in
/// backticks — so it is rendered as markdown rather than shown with its syntax intact. A folded
/// block is a one-line peek that says how much it stands for, and is the only part of a reasoning
/// block a click acts on.
pub(super) fn thinking_lines(
    text: &str,
    expanded: bool,
    ctx: &BlockContext<'_>,
) -> Vec<MarkdownLine> {
    let rendered = layout_markdown_in(&safe_tool_text(text), ctx.styles, MarkdownVoice::Reasoning);
    if expanded {
        // Nothing here is a handle. Reasoning shown in full is prose — read, selected, copied — and
        // a click that folded it would take the paragraph out from under the reader, who cannot see
        // that the row they clicked was a control. It folds from the keyboard, where the selection
        // says which block is being acted on.
        return rendered;
    }
    vec![MarkdownLine::prose(thinking_peek(rendered, ctx.styles, ctx.prose_width)).expandable()]
}

/// Marks a block's first row — and its last, when `with_footer` — as the rows a click opens it from.
pub(super) fn mark_handles(lines: &mut [MarkdownLine], with_footer: bool) {
    if let Some(first) = lines.first_mut() {
        first.expands = true;
    }
    if with_footer {
        if let Some(last) = lines.last_mut() {
            last.expands = true;
        }
    }
}

/// Cells the reasoning itself keeps. A count that would leave a peek narrower than this is dropped
/// instead — on a canvas that cannot carry both, the thought is the part worth reading.
const MIN_PEEK_WIDTH: usize = 24;

/// A one-line preview of reasoning for a collapsed block: the first non-blank rendered line, cut to
/// the prose measure, and a muted count of what is behind it.
///
/// The count is the part that earns its keep. Without it a fold is indistinguishable from text that
/// went missing — the peek stops mid-sentence and the rest of the block is simply gone — which is
/// how a deliberate one-line summary came to read as a rendering bug.
///
/// Built from the *rendered* lines rather than the source, so the peek shows a formatted title
/// instead of the `**` that produced it.
fn thinking_peek(rendered: Vec<MarkdownLine>, styles: &ThemeStyles, width: u16) -> Line<'static> {
    // A rendered line keeps its source breaks, so a line is not a row: the peek counts what it is
    // hiding in the breaks too, or a block written as one paragraph per break claims to hide none.
    let spoken: Vec<Line<'static>> = rendered
        .into_iter()
        .map(|layout| layout.line)
        .filter(|line| line.spans.iter().any(|span| !span.content.trim().is_empty()))
        .collect();
    let Some(first) = spoken.first().cloned() else {
        let style = styles.thinking_text.add_modifier(Modifier::ITALIC);
        return Line::from(Span::styled("(no content)", style));
    };
    let hidden = spoken
        .iter()
        .map(|line| line_text(line).lines().filter(|part| !part.trim().is_empty()).count())
        .sum::<usize>()
        .saturating_sub(1);
    let mut more = (hidden > 0)
        .then(|| format!(" · +{hidden} more {}", if hidden == 1 { "line" } else { "lines" }));
    let mut room = usize::from(width);
    if let Some(count) = more.as_deref() {
        let left = room.saturating_sub(crate::selection::cell_width(count));
        match left >= MIN_PEEK_WIDTH {
            true => room = left,
            false => more = None,
        }
    }

    let head = truncate_at_newline(first);
    let clipped = crate::selection::cell_width(&line_text(&head)) > room;
    let mut peek = clip_line(head, if clipped { room.saturating_sub(1) } else { room });
    if clipped {
        let style = peek.spans.last().map(|span| span.style).unwrap_or_default();
        peek.spans.push(Span::styled("…", style));
    }
    if let Some(more) = more {
        peek.spans.push(Span::styled(more, styles.muted));
    }
    peek
}

/// Everything up to the line's first embedded newline.
fn truncate_at_newline(line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        match span.content.split_once('\n') {
            Some((head, _)) => {
                if !head.is_empty() {
                    spans.push(Span::styled(head.to_string(), span.style));
                }
                break;
            }
            None => spans.push(span),
        }
    }
    Line::from(spans)
}

/// Clips a styled line to `width` terminal cells, keeping each span's own style.
pub(super) fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len());
    let mut used = 0usize;
    for span in line.spans {
        let span_width = crate::selection::cell_width(&span.content);
        if used + span_width <= width {
            used += span_width;
            spans.push(span);
            continue;
        }
        let text = crate::selection::slice_columns(&span.content, 0..width - used);
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
        break;
    }
    Line::from(spans)
}

/// A styled line's plain text.
pub(super) fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// A system notice's lines, like [`prefixed_text_lines`], with each `` `quoted` `` stretch drawn as
/// inline code: a command a notice names is something to type, and should look it.
pub(super) fn system_lines(
    icon: &str,
    text: &str,
    style: Style,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    text.split('\n')
        .enumerate()
        .map(|(index, text)| {
            let mut spans = Vec::new();
            if index == 0 && !icon.is_empty() {
                spans.push(Span::styled(icon_prefix(icon), style));
            }
            spans.extend(crate::widgets::code_spans(text, style, styles));
            Line::from(spans)
        })
        .collect()
}

pub(super) fn prefixed_text_lines(icon: &str, text: &str, style: Style) -> Vec<Line<'static>> {
    text.split('\n')
        .enumerate()
        .map(|(index, text)| {
            if index == 0 && !icon.is_empty() {
                Line::from(vec![
                    Span::styled(icon_prefix(icon), style),
                    Span::styled(text.to_string(), style),
                ])
            } else {
                Line::from(Span::styled(text.to_string(), style))
            }
        })
        .collect()
}
