//! Context files layout — the rows as lines under their group headings.

use super::{ContextFilesState, ContextKind};
use crate::theme::styles::ThemeStyles;
use ratatui::text::{Line, Span};

/// Every row as a line under its group's heading, with where each landed: `(line, anchor)`, the
/// line the row is on and the line a window has to reach to show it in context.
pub(super) fn row_layout(
    state: &ContextFilesState,
    theme: &ThemeStyles,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let mut lines = Vec::new();
    // Where each row landed among the lines, and the line the window has to reach to show it in
    // context — its group heading, for the row that opens a group. The headings between rows mean
    // a row's index is not its offset, and a cursor arriving on a group's first row with the
    // heading scrolled off is a row that has lost what kind of thing it is.
    let mut row_lines: Vec<(usize, usize)> = Vec::with_capacity(state.rows.len());
    let mut group: Option<ContextKind> = None;
    for (index, row) in state.rows.iter().enumerate() {
        let mut anchor = lines.len();
        if group != Some(row.kind) {
            if group.is_some() {
                lines.push(Line::from(""));
                anchor = lines.len();
            }
            lines.push(Line::from(Span::styled(row.kind.heading(), theme.accent_secondary)));
            group = Some(row.kind);
        }
        let chosen = index == state.selected;
        row_lines.push((lines.len(), anchor));
        lines.push(Line::from(vec![
            Span::styled(crate::glyphs::Mark::Cursor.gutter(chosen), theme.accent),
            Span::styled(row.kind.glyph(), if chosen { theme.accent } else { theme.muted }),
            Span::raw(crate::glyphs::icon_gap()),
            Span::styled(row.label.clone(), if chosen { theme.accent } else { theme.text }),
            Span::styled(format!("  {}", row.scope), theme.muted),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("Nothing loaded yet.", theme.text)));
    }
    (lines, row_lines)
}
