//! Input surfaces — composer and transcript-search rendering.

use crate::glyphs::Icon;
use crate::slash_args::Mark;
use crate::workspace_layout::gutter_inset;
use crate::{app::App, editor::TextLayout, search::SearchState, theme::styles::ThemeStyles};
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Cell width the composer wraps its draft at, given the prompt box's own area.
///
/// The box is framed, so its text area is two columns narrower than the area it fills. Measuring
/// at any wider value grows the box a keystroke late — the draft wraps onto a second row while
/// the frame is still one row tall.
pub(super) fn text_width(composer_width: u16) -> u16 {
    composer_width.saturating_sub(2).max(1)
}

pub(super) fn height(layout: &TextLayout, terminal_height: u16) -> u16 {
    const MAX_VISIBLE_LINES: usize = 8;

    let rows = layout.row_count().clamp(1, MAX_VISIBLE_LINES);
    (rows as u16 + 2).min(terminal_height.saturating_sub(4).max(3))
}

/// The composer's title: the session's mode, in its own color, and a button under the pointer.
///
/// Drawn in both modes rather than only when planning, because a button that appears only once you
/// are in plan mode cannot be what gets you there.
fn mode_title(app: &App) -> Line<'static> {
    let mode = app.status.mode.as_str();
    let hue = crate::modes::accent(mode, &app.theme_styles);
    let hovered = app.pointer.hover.as_ref() == Some(&crate::mouse::MouseTarget::ComposerMode);
    let style = match hovered {
        // The slab the bar's badges are cut from: ink on the color, so it reads as pressable
        true => ratatui::style::Style::new()
            .fg(app.theme_styles.slab.canvas)
            .bg(hue.fg.unwrap_or(ratatui::style::Color::Reset)),
        false => hue,
    };
    let label = style.add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(" ", style),
        Span::styled(crate::modes::icon(mode).prefix(), style),
        Span::styled(mode.to_string(), label),
        Span::styled(" ", style),
    ])
}

/// Renders the prompt composer.
pub(super) fn composer(f: &mut Frame, area: Rect, app: &mut App, layout: &TextLayout) {
    let title = mode_title(app);
    let block = crate::widgets::panel_block()
        .border_style(crate::modes::accent(&app.status.mode, &app.theme_styles))
        .title(title.clone());
    let inner = block.inner(area);
    let width = inner.width.max(1);
    app.editor.set_viewport_width(width);

    let lines = layout
        .rows()
        .iter()
        .map(|segments| {
            Line::from(
                segments
                    .iter()
                    .map(|segment| {
                        let base = match (segment.is_chip, segment.mark) {
                            (true, _) => app.theme_styles.chip,
                            (false, Some(Mark::Command)) => {
                                app.theme_styles.accent.add_modifier(Modifier::BOLD)
                            }
                            (false, Some(Mark::Keyword)) => app.theme_styles.accent_secondary,
                            (false, None) => app.theme_styles.text,
                        };
                        let style = if segment.is_selected {
                            base.patch(app.theme_styles.selected_bg)
                        } else {
                            base
                        };
                        Span::styled(segment.text.clone(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let cursor = app.editor.cursor_in(layout);
    let scroll_y = cursor.row.saturating_sub(usize::from(inner.height.saturating_sub(1)));
    f.render_widget(Paragraph::new(lines).block(block).scroll((scroll_y as u16, 0)), area);
    // The title is the mode switch: a click moves to the next mode, as shift+tab does
    let title_width = u16::try_from(title.width()).unwrap_or(u16::MAX);
    app.hit.borrow_mut().push(
        Rect::new(
            area.x.saturating_add(1),
            area.y,
            title_width.min(area.width.saturating_sub(2)),
            1,
        ),
        crate::mouse::MouseTarget::ComposerMode,
    );

    // A full-window overlay is drawn over the composer, so parking the caret here would leave a
    // blinking block sitting on top of someone else's surface.
    if app.overlays.image_viewer.is_some()
        || app.overlays.github.is_some()
        || app.overlays.pull_request_card.is_some()
    {
        return;
    }
    let cursor_x =
        inner.x.saturating_add(cursor.column as u16).min(inner.right().saturating_sub(1));
    let cursor_y = inner
        .y
        .saturating_add(cursor.row.saturating_sub(scroll_y) as u16)
        .min(inner.bottom().saturating_sub(1));
    f.set_cursor_position((cursor_x, cursor_y));

    // Each drawn row is a selectable text row, which is also how a click places the caret: the
    // press anchors a selection at the cell it hit, and anchoring moves the cursor there.
    let mut regions = app.hit.borrow_mut();
    for offset in 0..inner.height {
        regions.push_text(
            Rect::new(inner.x, inner.y + offset, inner.width, 1),
            crate::mouse::TextRow {
                surface: crate::mouse::TextSurface::Composer,
                line: usize::from(offset) + scroll_y,
                x: inner.x,
                first_column: 0,
                pin: None,
            },
        );
    }
}

/// The composer's own area inside the main column, inset the way the conversation and the dock
/// are, so the prompt box lines up with them.
pub(super) fn composer_area(column: Rect, frame: Rect) -> Rect {
    gutter_inset(column, frame)
}

pub(super) fn search(f: &mut Frame, area: Rect, search: &SearchState, styles: &ThemeStyles) {
    let match_info = if search.matches.is_empty() {
        "No matches".to_string()
    } else {
        let current = search.current_match.map(|index| index + 1).unwrap_or(0);
        format!("{current}/{}", search.matches.len())
    };
    // Same field chrome as the palette and the modal pickers: a bold accent `Search:` label so the
    // input is findable at a glance, the live query in body text, then a muted hit count.
    let block = crate::widgets::titled_panel(Icon::Search.glyph(), "Search conversation", styles);
    let inner = block.inner(area);
    let line = Line::from(vec![
        Span::styled(" Search: ", styles.accent.add_modifier(Modifier::BOLD)),
        Span::styled(search.query.clone(), styles.text),
        Span::styled(format!("  {match_info}"), styles.muted),
    ]);
    f.render_widget(Paragraph::new(line).block(block), area);

    // Park the terminal cursor after the query — the search bar replaces the composer while it is
    // up, so this is the only live input on screen.
    let cursor_x = inner
        .x
        .saturating_add(9)
        .saturating_add(UnicodeWidthStr::width(search.query.as_str()) as u16)
        .min(inner.right().saturating_sub(1));
    f.set_cursor_position((cursor_x, inner.y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measured_wrap_column_is_the_one_the_composer_draws_at() {
        // Height measurement and rendering must agree cell-for-cell: any drift and the box grows a
        // keystroke after the draft has already wrapped. The gutter is in the middle of that, so
        // it is measured on both sides of a dock as well as on its own.
        for column_width in [8u16, 24, 40, 80, 120, 200] {
            let frame = Rect::new(0, 0, column_width + 43, 3);
            for column in [
                Rect { width: column_width, ..frame },
                Rect { x: 43, width: column_width, ..frame },
            ] {
                let area = composer_area(column, frame);
                let drawn = crate::widgets::panel_block().inner(area).width;
                assert_eq!(text_width(area.width), drawn.max(1), "column width {column_width}");
            }
        }
    }
}
