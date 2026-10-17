//! Text field — an [`Editor`] drawn into a rectangle, with the terminal's own cursor in it.
//!
//! The composer's rendering, for the fields that live in modals: a reply, a pull request's title
//! and description. A multi-line field whose cursor you cannot see is one you cannot edit, so this
//! wraps at the field's width, scrolls to keep
//! the cursor on screen, shows the selection, and parks the real cursor where typing lands.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::editor::Editor;
use crate::theme::styles::ThemeStyles;

/// Draws `editor` into `area` and puts the terminal cursor at its caret.
pub(crate) fn render(frame: &mut Frame, area: Rect, editor: &Editor, styles: &ThemeStyles) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (lines, cursor) = lines(editor, area.width, area.height, styles);
    frame.render_widget(Paragraph::new(lines), area);
    frame.set_cursor_position((
        area.x.saturating_add(cursor.0).min(area.right().saturating_sub(1)),
        area.y.saturating_add(cursor.1).min(area.bottom().saturating_sub(1)),
    ));
}

/// Rows `editor` wraps to at `width`, at least one.
pub(crate) fn rows(editor: &Editor, width: u16) -> u16 {
    u16::try_from(editor.layout(width.max(1)).row_count()).unwrap_or(u16::MAX)
}

/// Rows `lines` take wrapped at `width`, as a `Paragraph` wrapping them draws them.
pub(crate) fn wrapped_rows(lines: &[Line<'_>], width: u16) -> u16 {
    let rows: usize = lines
        .iter()
        .map(|line| {
            Paragraph::new(line.clone())
                .wrap(ratatui::widgets::Wrap { trim: false })
                .line_count(width.max(1))
        })
        .sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}

/// The rows that fit in `height`, scrolled to keep the caret on screen, and the caret's `(x, y)`
/// within them.
fn lines(
    editor: &Editor,
    width: u16,
    height: u16,
    styles: &ThemeStyles,
) -> (Vec<Line<'static>>, (u16, u16)) {
    let layout = editor.layout(width.max(1));
    let cursor = editor.cursor_in(&layout);
    let scroll = cursor.row.saturating_sub(usize::from(height.saturating_sub(1)));
    let lines = layout
        .rows()
        .iter()
        .skip(scroll)
        .take(usize::from(height))
        .map(|segments| {
            Line::from(
                segments
                    .iter()
                    .map(|segment| {
                        let base = if segment.is_chip { styles.chip } else { styles.text };
                        let style =
                            if segment.is_selected { base.patch(styles.selected_bg) } else { base };
                        Span::styled(segment.text.clone(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let x = u16::try_from(cursor.column).unwrap_or(u16::MAX);
    let y = u16::try_from(cursor.row - scroll).unwrap_or(u16::MAX);
    (lines, (x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles() -> ThemeStyles {
        let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
        ThemeStyles::from_resolved(&resolved, "nord")
    }

    fn editor(text: &str, cursor: usize) -> Editor {
        Editor::with_text(text, cursor)
    }

    /// The caret sits where typing lands, on a wrapped row as much as a written one, and the field
    /// scrolls to keep it on screen rather than letting it fall off the bottom.
    #[test]
    fn the_caret_is_where_typing_lands_and_stays_on_screen() {
        let styles = styles();
        let text = "one two\nthree four five";
        let (drawn, caret) = lines(&editor(text, 2), 20, 5, &styles);
        assert_eq!(caret, (2, 0));
        assert_eq!(drawn.len(), 2);
        assert_eq!(lines(&editor(text, text.len()), 20, 5, &styles).1, (15, 1));
        // Wrapped at eight cells: "three " / "four " / "five" under "one two"
        let (drawn, caret) = lines(&editor(text, text.len()), 8, 2, &styles);
        assert_eq!(caret.1, 1, "the last row is the one on screen");
        assert_eq!(drawn.len(), 2);
        assert_eq!(drawn[1].to_string(), "five");
        assert_eq!(rows(&editor(text, 0), 8), 4);
        assert_eq!(rows(&editor("", 0), 8), 1, "an empty field still has a row to type in");
    }

    /// The selection is drawn, since a field that selects invisibly deletes by surprise.
    #[test]
    fn the_selection_is_drawn() {
        let styles = styles();
        let mut field = editor("select me", 0);
        field.select_all();
        let (drawn, _) = lines(&field, 20, 1, &styles);
        assert!(drawn[0]
            .spans
            .iter()
            .all(|span| span.style == styles.text.patch(styles.selected_bg)));
    }

    /// A field with no room draws nothing and claims no cursor, whichever side it has none on.
    #[test]
    fn a_field_with_no_room_claims_no_cursor() {
        use ratatui::{backend::Backend, backend::TestBackend, Terminal};
        let styles = styles();
        for area in [Rect::new(2, 2, 0, 3), Rect::new(2, 2, 10, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(20, 6)).expect("terminal");
            terminal.draw(|frame| render(frame, area, &editor("text", 4), &styles)).expect("draw");
            // A frame that sets no cursor hides it, and leaves it where it was
            assert_eq!(terminal.backend_mut().get_cursor_position().unwrap(), (0, 0).into());
        }
    }
}
