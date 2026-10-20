//! Palette rendering — the modal, its search field, the list and the detail pane beside it.
//!
//! Reads [`PaletteState`] and draws it; the click targets and the scrollbar it publishes are how
//! the mouse reaches back into the state.

use super::PaletteState;
use crate::commands::MenuEntry;
use crate::glyphs::Icon;
use crate::picker::{Slot, Split, Window};

/// Panel label, shared by the rendered title and the modal-width computation.
const TITLE: &str = "Command palette";
/// Rows the list shows before it scrolls.
const MAX_ROWS: usize = 14;
/// The narrowest the detail pane may be and still read as prose rather than a column of words.
const DETAIL_MIN: u16 = 34;
/// The prose width the pane is sized for when there is room, and the list's half matches it.
const DETAIL_WIDTH: u16 = 50;

/// How wide the modal is, and whether it splits for a pane, decided once per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    modal_width: u16,
    /// `None` when there is no room for the pane and the selected summary gets a row instead
    split: Option<Split>,
}

/// How many rows the list wants, which with a pane beside it is as many as the longest story any
/// command tells.
fn list_rows(
    state: &PaletteState,
    layout: &Layout,
    theme: &crate::theme::styles::ThemeStyles,
) -> usize {
    match layout.split {
        // With a pane beside it the modal is as tall as the longest story any command tells, so
        // nothing is clipped and it does not jump as the cursor moves from three lines to twelve
        Some(Split { pane: width, .. }) => state
            .menu
            .entries
            .iter()
            .map(|entry| {
                crate::picker::wrapped_rows(
                    &crate::command_detail::lines(entry, theme),
                    width.into(),
                )
            })
            .max()
            .unwrap_or(0)
            .max(MAX_ROWS),
        None => state.filtered.len().clamp(1, MAX_ROWS) + usize::from(state.divider().is_some()),
    }
}

/// The search field and its rule, with the cursor at the end of the query.
fn render_search(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &PaletteState,
    theme: &crate::theme::styles::ThemeStyles,
) {
    use ratatui::{
        style::Modifier,
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
    };
    use unicode_width::UnicodeWidthStr;

    let search = Paragraph::new(Line::from(vec![
        Span::styled(" Search: ", theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled(state.query.clone(), theme.text),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).border_style(theme.border_muted));
    frame.render_widget(search, area);
    let cursor_x = area
        .x
        .saturating_add(9)
        .saturating_add(UnicodeWidthStr::width(state.query.as_str()) as u16)
        .min(area.right().saturating_sub(1));
    frame.set_cursor_position((cursor_x, area.y));
}

/// Renders the searchable command palette as a centered modal.
pub fn render_palette(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &PaletteState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use ratatui::{
        layout::{Constraint, Direction, Rect},
        widgets::{Clear, Paragraph},
    };

    if area.width < 28 || area.height < 8 {
        return;
    }

    let layout = layout(state, area.width);
    let rows = list_rows(state, &layout, theme);
    let summary_rows = u16::from(layout.split.is_none());
    // Borders, the search field and its rule, the list, the summary when there is no pane, the
    // footer
    let desired_height = rows as u16 + 4 + summary_rows + crate::widgets::FOOTER_ROWS;
    let modal_height = desired_height.max(9).min(area.height.saturating_sub(2));
    let modal_area = Rect::new(
        area.x + area.width.saturating_sub(layout.modal_width) / 2,
        area.y + area.height.saturating_sub(modal_height) / 2,
        layout.modal_width,
        modal_height,
    );
    frame.render_widget(Clear, modal_area);

    let block = crate::widgets::titled_panel(Icon::Palette.glyph(), TITLE, theme);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let chunks = ratatui::layout::Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(summary_rows),
            Constraint::Length(crate::widgets::FOOTER_ROWS),
        ])
        .split(inner);

    render_search(frame, chunks[0], state, theme);

    let body = chunks[1];
    let list_area = Rect { width: layout.split.map_or(body.width, |split| split.list), ..body };
    render_list(frame, list_area, state, theme, hit);

    match (layout.split, state.selected_entry()) {
        (Some(split), Some(entry)) => crate::picker::render_pane(
            frame,
            body,
            split.list,
            split.pane,
            &crate::command_detail::lines(entry, theme),
            theme,
        ),
        (Some(split), None) => {
            crate::picker::render_pane(frame, body, split.list, split.pane, &[], theme)
        }
        (None, Some(entry)) => frame.render_widget(
            Paragraph::new(crate::command_detail::summary_line(entry, theme)),
            chunks[2],
        ),
        (None, None) => {}
    }

    let count = format!(
        "{}/{}",
        state.selected.saturating_add(1).min(state.filtered.len()),
        state.filtered.len()
    );
    crate::widgets::render_footer(
        frame,
        chunks[3],
        "enter run · ↑/↓ choose · esc close",
        Some(&count),
        theme,
    );
}

/// The list: a window of rows around the cursor, with the rule under the recent lead when one is
/// drawn, each row a click target for its filtered position.
fn render_list(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &PaletteState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use ratatui::{
        layout::Rect,
        text::{Line, Span},
        widgets::{List, ListItem, Paragraph},
    };

    if state.filtered.is_empty() {
        frame.render_widget(Paragraph::new(" No matching commands").style(theme.muted), area);
        return;
    }
    let divider = state.divider();
    let window = Window::new(state.filtered.len(), divider, state.selected, area.height as usize);
    let lines = Window::lines(state.filtered.len(), divider);
    let width = crate::widgets::list_rows_width(area.width, lines, area.height);
    let row_width = usize::from(width);
    let mut items = Vec::new();
    for (offset, slot) in window.slots().enumerate() {
        let row_y = area.y.saturating_add(offset as u16);
        match slot {
            Slot::Rule => items.push(ListItem::new(Line::from(Span::styled(
                "─".repeat(row_width),
                theme.border_muted,
            )))),
            Slot::Row(position) => {
                hit.borrow_mut().push(
                    Rect::new(area.x, row_y, width, 1),
                    crate::mouse::MouseTarget::PaletteRow(position),
                );
                let entry = state.entry_at(position).expect("the window stays inside the list");
                let selected = position == state.selected;
                items.push(ListItem::new(row(
                    entry,
                    &state.matched_at(position),
                    selected,
                    row_width,
                    theme,
                )));
            }
        }
    }
    frame.render_widget(List::new(items), Rect { width, ..area });
    let list = crate::mouse::OverlayList::Palette;
    let shown = (window.top(), lines);
    crate::widgets::list_scrollbar(frame, &mut hit.borrow_mut(), area, list, shown, theme);
}

/// One command row: the cursor, the name, its slash trigger, and its key pushed to the right edge.
///
/// Matched characters take the accent in the name and the trigger alike, since both are searched —
/// a row found through its `/auth` must not draw with nothing lit at all.
pub(super) fn row(
    entry: &MenuEntry,
    matched: &std::collections::HashSet<usize>,
    selected: bool,
    row_width: usize,
    theme: &crate::theme::styles::ThemeStyles,
) -> ratatui::text::Line<'static> {
    use ratatui::{style::Modifier, text::Span};
    use unicode_width::UnicodeWidthStr;

    let lit = theme.accent.add_modifier(Modifier::BOLD);
    let paint = |text: &str, offset: usize, base| -> Vec<Span<'static>> {
        text.chars()
            .enumerate()
            .map(|(index, character)| {
                let style = if matched.contains(&(offset + index)) { lit } else { base };
                Span::styled(character.to_string(), style)
            })
            .collect()
    };
    let name = entry.command.name.as_ref();
    let name_chars = name.chars().count();
    let mut spans =
        vec![Span::styled(crate::glyphs::Mark::Cursor.gutter(selected), theme.accent_on(selected))];
    spans.extend(paint(name, 0, theme.label_on(selected)));
    let mut used = UnicodeWidthStr::width(name);
    if let Some(slash) = entry.command.slash_name() {
        spans.push(Span::raw("  "));
        // The haystack carries the trigger without its slash, so the slash itself is never lit
        // and the trigger's first letter is the character after the name's trailing space
        spans.push(Span::styled("/", theme.muted));
        spans.extend(paint(slash.trim_start_matches('/'), name_chars + 1, theme.muted));
        used += 2 + UnicodeWidthStr::width(slash);
    }
    let room = row_width.saturating_sub(crate::widgets::SELECTION_FRAME + 2);
    if let Some(chord) = entry.chord.as_deref() {
        let chord_width = UnicodeWidthStr::width(chord);
        if used + 2 + chord_width <= room {
            spans.push(Span::raw(" ".repeat(room - used - chord_width)));
            spans.push(Span::styled(chord.to_owned(), theme.muted));
        }
    }
    crate::widgets::selection_row(spans, row_width, selected, theme)
}

/// The natural width of a row: its frame, the cursor column, the name, the trigger and the key.
fn row_width(entry: &MenuEntry) -> usize {
    use unicode_width::UnicodeWidthStr;
    crate::widgets::SELECTION_FRAME
        + 2
        + UnicodeWidthStr::width(entry.command.name.as_ref())
        + entry.command.slash_name().map_or(0, |slash| 2 + UnicodeWidthStr::width(slash))
        + entry.chord.as_deref().map_or(0, |chord| 2 + UnicodeWidthStr::width(chord))
}

/// How wide the modal is, and where it splits, given `available` terminal columns.
///
/// Measured over every entry, not the filtered ones, so the modal holds still as the query
/// narrows. With room for a pane the modal is two equal halves — each as wide as the longest row
/// or a comfortable line of prose, whichever is more — so the rule sits on its middle column, as
/// it does in the slash flyout. The pane appears only when both halves fit.
fn layout(state: &PaletteState, available: u16) -> Layout {
    use unicode_width::UnicodeWidthStr;
    let widest_row = state.menu.entries.iter().map(row_width).max().unwrap_or(0) as u16;
    let total = state.menu.entries.len();
    let footer = UnicodeWidthStr::width(
        format!(" {total}/{total} · enter run · ↑/↓ choose · esc close ").as_str(),
    ) as u16;
    let title = crate::widgets::panel_title_width(Icon::Palette.glyph(), TITLE) as u16;
    let max_width = available.saturating_sub(4).max(28);
    // The pane's half carries its gap, its text and a cell before the frame
    let half = widest_row.max(title).max(DETAIL_WIDTH + PANE_SPACING);
    let fits = |half: u16| Split::around(half).saturating_add(2) <= max_width;
    let half = if fits(half) { half } else { max_width.saturating_sub(3) / 2 };
    let inner = Split::around(half);
    let split = Split::of(inner);
    if split.list >= widest_row && split.pane >= DETAIL_MIN {
        let modal_width = inner.saturating_add(2).max(footer + 2);
        return Layout { modal_width, split: Some(split) };
    }
    let natural = widest_row.max(footer).max(title);
    // Borders, and one cell of breathing room on the right
    let modal_width = natural.saturating_add(3).clamp(28, max_width);
    Layout { modal_width, split: None }
}

/// What the pane's half spends besides its text: the gap after the rule, and a cell before the
/// frame.
const PANE_SPACING: u16 = crate::picker::PANE_GUTTER;
