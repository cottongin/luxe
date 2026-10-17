//! Picker layout — the list and detail panes, the window over the rows, and the tab line.

use crate::theme::styles::{Depth, ThemeStyles};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    Frame,
};

/// The rule between a list and the pane beside it, and the gap after it.
pub(crate) const PANE_GUTTER: u16 = 2;

/// How a list and the pane beside it share a width: the rule on the middle column.
///
/// The one rule both the palette and the slash flyout follow, so on a wide terminal a list of rows
/// forty columns long does not take three quarters of the width and squeeze the prose beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Split {
    /// Columns for the list, left of the rule
    pub(crate) list: u16,
    /// Columns for the pane's text, after the rule and its gap, less a cell before the frame
    pub(crate) pane: u16,
}

impl Split {
    /// Halves `width`, the columns inside a frame, at its middle column.
    pub(crate) fn of(width: u16) -> Self {
        let list = width.saturating_sub(1) / 2;
        Self { list, pane: width.saturating_sub(list + PANE_GUTTER + 1) }
    }

    /// The width inside a frame that gives each side `half` columns, the rule between them.
    pub(crate) fn around(half: u16) -> u16 {
        half.saturating_mul(2).saturating_add(1)
    }
}

/// Draws the rule at `list_width` into `body` and `lines` in the `width` after it, wrapped.
pub(crate) fn render_pane(
    f: &mut Frame,
    body: Rect,
    list_width: u16,
    width: u16,
    lines: &[Line<'static>],
    theme: &ThemeStyles,
) {
    let rule_x = body.x.saturating_add(list_width);
    let rule: Vec<Line> =
        (0..body.height).map(|_| Line::from(Span::styled("│", theme.border_muted))).collect();
    f.render_widget(ratatui::widgets::Paragraph::new(rule), Rect { x: rule_x, width: 1, ..body });
    let pane_x = rule_x.saturating_add(PANE_GUTTER);
    let pane = Rect { x: pane_x, width: width.min(body.right().saturating_sub(pane_x)), ..body };
    f.render_widget(
        ratatui::widgets::Paragraph::new(lines.to_vec())
            .wrap(ratatui::widgets::Wrap { trim: false }),
        pane,
    );
}

/// About how many rows `lines` take wrapped to `width`: each at least one, a long one more.
///
/// An estimate — word wrap can spend a row more than this — which is fine for sizing a panel, and
/// the pane clips rather than overflowing.
pub(crate) fn wrapped_rows(lines: &[Line], width: usize) -> usize {
    lines.iter().map(|line| line.width().div_ceil(width.max(1)).max(1)).sum()
}

/// What occupies one line of a list's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The item at this filtered position
    Row(usize),
    /// The rule above the divider's position
    Rule,
}

/// The slice of a list on screen: `capacity` lines ending at the cursor, where a rule counts as a
/// line of its own and is never selected.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    divider: Option<usize>,
    start: usize,
    end: usize,
}

impl Window {
    /// Args: `(len, divider, selected, capacity)` — `divider` the filtered position the rule sits
    /// above.
    pub(crate) fn new(
        len: usize,
        divider: Option<usize>,
        selected: usize,
        capacity: usize,
    ) -> Self {
        let lines = len + usize::from(divider.is_some());
        let line_of =
            |position: usize| position + usize::from(divider.is_some_and(|d| position >= d));
        let start =
            (line_of(selected) + 1).saturating_sub(capacity).min(lines.saturating_sub(capacity));
        Self { divider, start, end: (start + capacity).min(lines) }
    }

    /// Each line on screen, top to bottom.
    pub(crate) fn slots(self) -> impl Iterator<Item = Slot> {
        (self.start..self.end).map(move |line| match self.divider {
            Some(divider) if line == divider => Slot::Rule,
            Some(divider) if line > divider => Slot::Row(line - 1),
            _ => Slot::Row(line),
        })
    }
}

/// The tab row: every source by name, the open one cut into a slab on the selection surface, in
/// the accent and bold, the rest quiet.
///
/// The surface is the one a chosen row wears, so "open" and "chosen" are one color in the picker;
/// the bold stays for a terminal that drops backgrounds. A closed tab spends the slab's two cells
/// each side in blanks, so every label keeps its column as the open tab moves — a row of tabs that
/// shifts sideways on every Tab is the jitter the fixed height exists to remove. The first label
/// starts on the column the rows' cursor does.
pub(crate) fn tab_line(tabs: &[&str], open: usize, theme: &ThemeStyles) -> Line<'static> {
    let surface = theme.selected_bg.bg.unwrap_or_else(|| theme.slab.at(Depth::Raised));
    let bold = ratatui::style::Style::new().add_modifier(ratatui::style::Modifier::BOLD);
    let mut spans = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        match index == open {
            true => spans.extend(crate::widgets::slab(
                [Span::styled((*tab).to_owned(), theme.accent)],
                surface,
                theme.slab.canvas,
                bold,
            )),
            false => spans.push(Span::styled(format!("  {tab}  "), theme.muted)),
        }
    }
    Line::from(spans)
}
