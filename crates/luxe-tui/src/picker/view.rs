//! Picker view — what a picker draws, flown out from the composer or centered over the screen.

use super::layout::{render_pane, tab_line, wrapped_rows, Slot, Split, Window};
use super::rows::picker_row;
use super::{MAX_VISIBLE_ROWS, TAB_ROWS};
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::ThemeStyles;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, List, ListItem},
    Frame,
};
use std::cell::RefCell;

/// One rendered row: a primary label and an optional muted detail column.
#[derive(Clone, Debug, Default)]
pub struct PickerRow {
    /// Primary text (e.g. a command title or a file path).
    pub label: String,
    /// Secondary muted text shown after the label (e.g. a subtitle or match context).
    pub detail: Option<String>,
    /// Characters of `label` the query matched, by char index, drawn in the accent
    pub label_matched: Vec<usize>,
    /// Characters of `detail` the query matched, by char index
    pub detail_matched: Vec<usize>,
    /// Whether `label` is a path: its folder is drawn quiet, and trimmed before the filename is
    pub path: bool,
    /// The detail's own style, for a detail whose color says something
    pub detail_style: Option<ratatui::style::Style>,
}

/// Lines for the pane beside the list, for the row at a filtered position.
pub type DetailFn<'a> = &'a dyn Fn(usize) -> Vec<Line<'static>>;

/// Everything [`render_picker`] needs that varies per flyout. `row` and `hit_target` both take a
/// *filtered position* (0-based over the filtered set).
pub struct PickerView<'a, Row, Hit>
where
    Row: Fn(usize) -> PickerRow,
    Hit: Fn(usize) -> MouseTarget,
{
    /// Nerd-font glyph identifying the surface (a `crate::widgets::glyph` constant).
    pub glyph: &'a str,
    /// Panel title, rendered beside `glyph` in bold.
    pub title: &'a str,
    /// Message shown when nothing matches (rendered muted, indented).
    pub empty_message: &'a str,
    /// Number of filtered rows.
    pub item_count: usize,
    /// Cursor position within the filtered set.
    pub selected: usize,
    /// Builds the row at a filtered position.
    pub row: Row,
    /// Maps a filtered position to its click target.
    pub hit_target: Hit,
    /// Keys along the bottom edge, when the picker has more than moving and choosing
    pub hint: Option<&'a str>,
    /// Tabs along the top, and the one open, when the picker lists more than one source
    pub tabs: Option<(&'a [&'a str], usize)>,
    /// The filtered position a muted rule is drawn above, setting a lead apart from the rest
    pub divider: Option<usize>,
    /// What to say about the row under the cursor, in a pane beside the list when there is room
    pub detail: Option<DetailFn<'a>>,
    /// Rows the list keeps whatever it holds, so moving between tabs never moves the frame, with
    /// an empty or loading message in the middle of them; `None` fits the list to its rows
    pub rows: Option<usize>,
}

/// Rows a picker with tabs keeps. Odd, so an empty tab's message has a middle row to sit on; nine,
/// the nearest to the eight a fitted list grows to.
pub const TABBED_ROWS: usize = 9;

/// Terminal width below which a picker keeps its whole width for the list.
const DETAIL_PANE_FROM: u16 = 90;

/// The tallest a picker grows to fit what its pane says, which is more than its list gets alone.
const MAX_DETAIL_ROWS: usize = 10;

/// Which edge of the composer a flyout grows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Up from this row, the composer's top: in a session, where the composer sits at the bottom
    Above(u16),
    /// Down from the top of the region it is given: on the start screen, where the composer sits
    /// high and the room is under it
    Below,
}

/// Render a composer flyout against the input, growing away from it and scrolling a fixed window.
///
/// Shared by the `/` and `@` pickers: identical chrome (the standard
/// [`crate::widgets::titled_panel`]
/// frame, accent cursor, bold label on the focused row, muted detail) and identical mouse-region
/// publishing — only [`PickerView`] varies.
pub fn render_picker<Row, Hit>(
    f: &mut Frame,
    area: Rect,
    edge: Edge,
    theme: &ThemeStyles,
    hit: &RefCell<HitRegions>,
    view: PickerView<'_, Row, Hit>,
) where
    Row: Fn(usize) -> PickerRow,
    Hit: Fn(usize) -> MouseTarget,
{
    let available_height = match edge {
        Edge::Above(input_top) => input_top.saturating_sub(area.y),
        Edge::Below => area.height,
    };
    if area.width == 0 || available_height == 0 {
        return;
    }
    let FlyoutMeasure { split, pane_width, tab_rows, divider, footer_rows, desired_height } =
        measure_flyout(area, &view);
    let flyout_height = desired_height.min(available_height);
    let top = match edge {
        Edge::Above(input_top) => input_top.saturating_sub(flyout_height),
        Edge::Below => area.y,
    };
    let flyout_area = Rect::new(area.x, top, area.width, flyout_height);
    f.render_widget(Clear, flyout_area);

    let list_width = match split {
        Some(split) => split.list,
        None => flyout_area.width.saturating_sub(2),
    };
    let capacity = flyout_height.saturating_sub(2 + tab_rows + footer_rows) as usize;
    let window = Window::new(view.item_count, divider, view.selected, capacity);
    let items = build_picker_window(
        &view,
        &window,
        hit,
        (flyout_area, list_width, pane_width, tab_rows),
        theme,
    );
    let body = render_picker_header(f, flyout_area, &view, theme);
    let list_area = Rect { width: list_width.min(body.width), ..body };
    match (view.item_count, view.rows) {
        (0, Some(_)) => render_centered(f, list_area, view.empty_message, theme),
        _ => f.render_widget(List::new(items), list_area),
    }
    if let (Some(detail), Some(width)) = (view.detail, pane_width) {
        render_pane(f, body, list_width, width, &detail(view.selected), theme);
    }
}

/// How tall a picker wants to be and how it divides its width, measured before anything draws.
struct FlyoutMeasure {
    split: Option<Split>,
    pane_width: Option<u16>,
    tab_rows: u16,
    divider: Option<usize>,
    footer_rows: u16,
    desired_height: u16,
}

/// Measures the picker: its list and detail split, the tallest detail, the tab and footer rows, and
/// the height that holds them all.
fn measure_flyout<Row, Hit>(area: Rect, view: &PickerView<'_, Row, Hit>) -> FlyoutMeasure
where
    Row: Fn(usize) -> PickerRow,
    Hit: Fn(usize) -> MouseTarget,
{
    // Inside the frame, halved: the same split the palette makes
    let split = view
        .detail
        .filter(|_| area.width >= DETAIL_PANE_FROM && view.item_count > 0)
        .map(|_| Split::of(area.width.saturating_sub(2)));
    let pane_width = split.map(|split| split.pane);
    // The tallest thing the pane will say across every row, so the picker holds its height as the
    // cursor moves rather than jumping with each command's story
    let pane_rows = match (view.detail, pane_width) {
        (Some(detail), Some(width)) => (0..view.item_count)
            .map(|position| wrapped_rows(&detail(position), usize::from(width)))
            .max()
            .unwrap_or(0)
            .min(MAX_DETAIL_ROWS),
        _ => 0,
    };
    // The tabs take a header of their own above the list
    let tab_rows = if view.tabs.is_some() { TAB_ROWS } else { 0 };
    let divider = view.divider.filter(|&at| at > 0 && at < view.item_count);
    let list_rows = match view.rows {
        Some(rows) => rows.max(1),
        None => view.item_count.clamp(1, MAX_VISIBLE_ROWS) + usize::from(divider.is_some()),
    };
    let footer_rows = if view.hint.is_some() { crate::widgets::FOOTER_ROWS } else { 0 };
    let desired_height = list_rows.max(pane_rows) as u16 + 2 + tab_rows + footer_rows;
    FlyoutMeasure { split, pane_width, tab_rows, divider, footer_rows, desired_height }
}

/// The rows the window shows, each publishing its click as it is built so the drawn row and its
/// hit cannot drift; a picker with nothing to list says why instead.
///
/// Args: `(view, window, hit, (flyout_area, list_width, pane_width, tab_rows), theme)`.
fn build_picker_window<Row, Hit>(
    view: &PickerView<'_, Row, Hit>,
    window: &Window,
    hit: &RefCell<HitRegions>,
    (flyout_area, list_width, pane_width, tab_rows): (Rect, u16, Option<u16>, u16),
    theme: &ThemeStyles,
) -> Vec<ListItem<'static>>
where
    Row: Fn(usize) -> PickerRow,
    Hit: Fn(usize) -> MouseTarget,
{
    // Inside the border, which is where the row — and its closing cap — has to fit.
    let row_width = usize::from(list_width);
    if view.item_count == 0 {
        // Through the same row builder as a real row, so the message lands in the label column
        // rather than on a count of spaces that has to be re-derived every time the frame changes.
        // A reason can name a command to run, which reads as code like it does everywhere else
        let mut message = vec![Span::raw("  ")];
        message.extend(crate::widgets::code_spans(view.empty_message, theme.muted, theme));
        vec![ListItem::new(crate::widgets::selection_row(message, row_width, false, theme))]
    } else {
        let mut regions = hit.borrow_mut();
        window
            .slots()
            .enumerate()
            .map(|(offset, slot)| match slot {
                Slot::Rule => ListItem::new(Line::from(Span::styled(
                    "─".repeat(row_width),
                    theme.border_muted,
                ))),
                Slot::Row(pos) => {
                    let row_y =
                        flyout_area.y.saturating_add(1 + tab_rows).saturating_add(offset as u16);
                    // The whole row clicks, up to the pane when there is one
                    let width = match pane_width {
                        Some(_) => list_width.saturating_add(1),
                        None => flyout_area.width,
                    };
                    regions.push(Rect::new(flyout_area.x, row_y, width, 1), (view.hit_target)(pos));
                    ListItem::new(picker_row(
                        (view.row)(pos),
                        pos == view.selected,
                        row_width,
                        theme,
                    ))
                }
            })
            .collect()
    }
}

/// Draws the picker's frame, and its tab line when it has tabs, and returns the body under them.
fn render_picker_header<Row, Hit>(
    f: &mut Frame,
    flyout_area: Rect,
    view: &PickerView<'_, Row, Hit>,
    theme: &ThemeStyles,
) -> Rect
where
    Row: Fn(usize) -> PickerRow,
    Hit: Fn(usize) -> MouseTarget,
{
    let block = crate::widgets::titled_panel(view.glyph, view.title, theme);
    let inner = match view.hint {
        Some(hint) => crate::widgets::render_footed(f, block, flyout_area, hint, None, theme),
        None => {
            let inner = block.inner(flyout_area);
            f.render_widget(block, flyout_area);
            inner
        }
    };
    match view.tabs {
        Some((tabs, open)) => {
            f.render_widget(
                ratatui::widgets::Paragraph::new(tab_line(tabs, open, theme)).block(
                    ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::BOTTOM)
                        .border_style(theme.border_muted),
                ),
                Rect { height: inner.height.min(TAB_ROWS), ..inner },
            );
            Rect { y: inner.y + TAB_ROWS, height: inner.height.saturating_sub(TAB_ROWS), ..inner }
        }
        None => inner,
    }
}

/// `message` in the middle of `area`, both ways, wrapping when it is wider than the area.
///
/// A fixed-height picker with nothing to list would otherwise say so on its top row and leave the
/// rest blank, which reads as a list that failed to draw. A reason can name a command, which reads
/// as code like it does everywhere else.
fn render_centered(f: &mut Frame, area: Rect, message: &str, theme: &ThemeStyles) {
    let line = Line::from(crate::widgets::code_spans(message, theme.muted, theme));
    // A column of margin each side, so a wrapped message does not run into the frame
    let width = area.width.saturating_sub(2);
    let rows =
        (wrapped_rows(std::slice::from_ref(&line), usize::from(width)) as u16).min(area.height);
    let top = area.y + area.height.saturating_sub(rows) / 2;
    f.render_widget(
        ratatui::widgets::Paragraph::new(line)
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true }),
        Rect { x: area.x + 1, y: top, width, height: rows },
    );
}
