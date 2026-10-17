//! Question prompt rows — the options and the typed answer, measured and laid out into lines.

use super::state::QuestionPromptState;
use super::{CUSTOM_PLACEHOLDER, MAX_DESCRIPTION_ROWS, MIN_DESCRIPTION_WIDTH, RECOMMENDED_BADGE};
use crate::widgets::truncate_end as truncate;
use ratatui::text::{Line, Span};

/// One selectable row laid out into terminal lines, so a click region can span its real height.
pub(super) struct LaidOutRow {
    pub(super) lines: Vec<Line<'static>>,
}

/// What every row is measured against.
#[derive(Clone, Copy)]
struct RowGeometry {
    indent: usize,
    text_width: usize,
    show_rationale: bool,
    custom_rows: usize,
}

/// One row's lines of spans: an option's label with its badge and rationale, or the typed answer.
fn row_content(
    state: &QuestionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    i: usize,
    geometry: &RowGeometry,
) -> Vec<Vec<Span<'static>>> {
    let RowGeometry { indent, text_width, show_rationale, custom_rows } = *geometry;
    let selected = i == state.selected();
    let cursor_style = theme.accent_on(selected);
    let label_style = theme.label_on(selected);
    let mut spans = vec![Span::styled(crate::glyphs::Mark::Cursor.gutter(selected), cursor_style)];
    if state.multi {
        let box_glyph = if state.is_checked(i) {
            format!("[{}] ", crate::glyphs::Mark::Yes)
        } else {
            "[ ] ".to_string()
        };
        spans.push(Span::styled(box_glyph, cursor_style));
    }

    let mut content: Vec<Vec<Span<'static>>> = Vec::new();
    match state.options.get(i) {
        Some(option) => {
            // The label outranks the badge: on a narrow panel the cursor already opens on
            // the recommendation, so a clipped label buys nothing.
            let label = format!("{}. {}", i + 1, option.label);
            let room = text_width.saturating_sub(indent);
            let badged =
                option.recommended && room >= label.chars().count() + RECOMMENDED_BADGE.len();
            let label_width = if badged { room - RECOMMENDED_BADGE.len() } else { room };
            spans.push(Span::styled(truncate(&label, label_width), label_style));
            if badged {
                spans.push(Span::styled(RECOMMENDED_BADGE, theme.success));
            }
            content.push(spans);

            if show_rationale {
                let rationale = option.description.as_deref().unwrap_or_default();
                for line in wrapped_rows(rationale, room, MAX_DESCRIPTION_ROWS) {
                    content
                        .push(vec![Span::raw(" ".repeat(indent)), Span::styled(line, theme.muted)]);
                }
            }
        }
        // The typed answer, always last and numbered like the rest. It wraps rather than
        // truncating: an answer cut off at the right edge is one the reader cannot check
        // before sending it.
        None => {
            let body = if state.editing() {
                format!("{}▏", state.custom())
            } else if state.custom().is_empty() {
                CUSTOM_PLACEHOLDER.to_string()
            } else {
                state.custom().to_string()
            };
            let number = format!("{}. ", i + 1);
            // The text column, wherever a longer number puts it
            let text_at = indent + number.chars().count().saturating_sub(3);
            let room = text_width.saturating_sub(text_at);
            let (mut wrapped, scrolled) = custom_view(&body, room, custom_rows, state.editing());
            // An answer of nothing but spaces still owns a row: it is the one the cursor
            // can land on, and the click regions are laid out over the rows there are.
            if wrapped.is_empty() {
                wrapped.push(String::new());
            }
            for (index, row) in wrapped.into_iter().enumerate() {
                let mut line = if index == 0 {
                    let mut first = std::mem::take(&mut spans);
                    first.push(Span::styled(number.clone(), label_style));
                    first
                } else {
                    vec![Span::raw(" ".repeat(text_at))]
                };
                // Said in the row's first cell, so the text column holds still while the
                // window scrolls
                if scrolled {
                    let lead = if index == 0 { "…" } else { " " };
                    line.push(Span::styled(lead, cursor_style));
                }
                line.push(Span::styled(row, label_style));
                content.push(line);
            }
        }
    }
    content
}

/// Lay out the options and the free-text row.
///
/// `with_descriptions` keeps each option's rationale, and `custom_rows` is what the typed answer
/// may spend before its window starts scrolling — the two knobs [`super::plan`] turns down to fit.
pub(super) fn lay_out_rows(
    state: &QuestionPromptState,
    theme: &crate::theme::styles::ThemeStyles,
    inner_width: usize,
    with_descriptions: bool,
    custom_rows: usize,
) -> Vec<LaidOutRow> {
    // `❯ `, the optional checkbox, and `1. ` — what a rationale has to clear to line up under
    // its label.
    let indent = 2 + if state.multi { 4 } else { 0 } + 3;
    // Whether a rationale is worth setting at all: what is left of the panel after the slab's
    // frame and the label's own indent has to be wide enough to read. A narrower panel shows the
    // labels alone rather than a column of two-word fragments.
    let roomy = inner_width.saturating_sub(crate::widgets::SELECTION_FRAME + indent)
        >= MIN_DESCRIPTION_WIDTH;
    let show_rationale = with_descriptions && roomy;
    let custom_rows = custom_rows.min(MAX_DESCRIPTION_ROWS + 1);
    // What a row's text may occupy: the panel, less the slab's two cap cells.
    let text_width = inner_width.saturating_sub(crate::widgets::SELECTION_FRAME);

    let geometry = RowGeometry { indent, text_width, show_rationale, custom_rows };
    let rows: Vec<(bool, Vec<Vec<Span<'static>>>)> = (0..state.row_count())
        .map(|i| (i == state.selected(), row_content(state, theme, i, &geometry)))
        .collect();

    debug_assert!(
        rows.iter().all(|(_, lines)| lines.len() <= MAX_DESCRIPTION_ROWS + 1),
        "a row is taller than the rationale budget"
    );
    rows.into_iter()
        // One slab for the whole option, not one per line: the label and the rationale under it are
        // a single choice.
        .map(|(selected, content)| LaidOutRow {
            lines: crate::widgets::selection_block(content, inner_width, selected, theme),
        })
        .collect()
}

/// Word-wrap `text` to `width` columns.
fn wrap_rows(text: &str, width: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() || width == 0 {
        return Vec::new();
    }
    let line = Line::from(Span::raw(text.to_string()));
    crate::widgets::wrap_line(&line, width)
        .into_iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect()
}

/// Word-wrap `text` to `width` columns, clipped to `max_rows` with an ellipsis on the last row.
pub(super) fn wrapped_rows(text: &str, width: usize, max_rows: usize) -> Vec<String> {
    if max_rows == 0 {
        return Vec::new();
    }
    let mut rows = wrap_rows(text, width);
    if rows.len() > max_rows {
        rows.truncate(max_rows);
        if let Some(last) = rows.last_mut() {
            *last = truncate(&format!("{last}…"), width);
        }
    }
    rows
}

/// The rows of a typed answer that fit, and whether any were left above them.
///
/// While the row is being edited the caret is what has to stay on screen, so the window follows
/// the tail of the answer; at rest it reads from the top like any other text, with the usual
/// clipped-tail mark.
fn custom_view(text: &str, width: usize, max_rows: usize, editing: bool) -> (Vec<String>, bool) {
    if !editing {
        return (wrapped_rows(text, width, max_rows), false);
    }
    if max_rows == 0 {
        return (Vec::new(), false);
    }
    let rows = wrap_rows(text, width);
    if rows.len() <= max_rows {
        return (rows, false);
    }
    // a cell short, for the mark that says there is more above
    let mut rows = wrap_rows(text, width.saturating_sub(1));
    (rows.split_off(rows.len() - max_rows), true)
}
