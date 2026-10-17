//! Dock plan — the PLAN section: the checklist and the grants each row carries.

use super::anchors::{list_window, scrolls, DockListAnchor, DockSection};
use super::chrome::{muted_line, section_heading_with, Section};
use crate::app::App;
use crate::widgets::truncate_end as truncate_cells;
use crate::workspace_dock::{DockList, DockPlan, DockPlanItem};
use luxe_core::TodoSummaryStatus;
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// Rows the Plan checklist may spend on todos before it starts folding them into a counted tail.
pub(super) const PLAN_ROWS_MAX: usize = 8;

/// Rows the checklist may spend, beyond one per todo, letting a long subject wrap.
///
/// A subject cut at the rail's edge hides the part that says which item it is, and a wider rail
/// only moves the cut. The rows go first to the work in hand, then to what is next, then to what is
/// done, so the budget buys the reading that matters before it buys the record.
pub(super) const PLAN_WRAP_ROWS: usize = 4;

/// Rows of upcoming work the window keeps below the active one before it starts to scroll.
pub(super) const PLAN_LOOKAHEAD: usize = 2;

/// Cells the tree connector spends at the head of a checklist row.
const CONNECTOR_WIDTH: usize = 3;

/// Cells the status glyph and its space spend after it.
const GLYPH_WIDTH: usize = 2;

/// The PLAN section: the session's todos as an actual checklist.
///
/// A single "current item + counts" line hid the shape of the work. Each todo gets its own row
/// with a status glyph, the in-progress one is emphasized, and overflow folds into a counted tail
/// so a long list can't swallow the panel.
///
/// The rows keep the order they were written in. Sorting the finished ones to the bottom made the
/// list rearrange itself under the reader every time a box was ticked — a plan is a sequence, and
/// the point of ticking one off is watching the line above the next one go green.
///
/// A checklist longer than the budget scrolls in a viewport of its own, with the window around the
/// work in hand as the position it keeps while nobody has moved it.
pub(super) fn plan_lines(plan: &DockPlan, app: &App) -> DockSection {
    let mut lines = vec![
        Line::default(),
        plan_heading_line(plan, app),
        muted_line(
            format!(
                "{} pending · {} active · {} done",
                plan.pending, plan.in_progress, plan.completed
            ),
            app,
        ),
    ];

    let total = plan.items.len();
    let (offset, width) =
        list_window(app, DockList::Plan, total, PLAN_ROWS_MAX, plan_follow(&plan.items));

    let start = lines.len();
    let window: Vec<&DockPlanItem> = plan.items.iter().skip(offset).take(PLAN_ROWS_MAX).collect();
    let subject_width = width.saturating_sub(CONNECTOR_WIDTH + GLYPH_WIDTH).max(6);
    let wrapped: Vec<Vec<String>> =
        window.iter().map(|item| subject_rows(&item.subject, subject_width)).collect();
    let grants = plan_row_grants(&window, &wrapped);
    for (index, ((item, wrapped), rows)) in window.iter().zip(wrapped).zip(grants).enumerate() {
        // The connector closes the section only where this really is the checklist's last row —
        // which a plan the snapshot trimmed does not have on the rail at all.
        let last = offset + index + 1 == total && plan.remaining.unwrap_or(0) == 0;
        lines.extend(todo_lines(item, wrapped, last, subject_width, rows, app));
    }
    let end = lines.len();
    DockSection {
        lines,
        hits: Vec::new(),
        list: scrolls(DockList::Plan, (start, end), offset, total, PLAN_ROWS_MAX)
            .map(|list| DockListAnchor { window: Some(window.len()), ..list }),
    }
}

/// Where the checklist sits while nobody has scrolled it.
///
/// Anchored at the top, because that is where the list starts and where a reader working down it is
/// looking. It slides only once the row in hand would fall off the end, and then only far enough to
/// keep that row and the next couple in view — so the window moves a line at a time rather than
/// jumping, and the work in flight is never the thing that got folded away.
pub(super) fn plan_follow(items: &[DockPlanItem]) -> usize {
    if items.len() <= PLAN_ROWS_MAX {
        return 0;
    }
    let focus = items
        .iter()
        .position(|item| item.status == TodoSummaryStatus::InProgress)
        .or_else(|| items.iter().position(|item| item.status != TodoSummaryStatus::Completed))
        .unwrap_or(items.len() - 1);
    (focus + 1 + PLAN_LOOKAHEAD).saturating_sub(PLAN_ROWS_MAX).min(items.len() - PLAN_ROWS_MAX)
}

/// How many rows each todo in the window may take: one each, then the wrap budget handed out by
/// what the reader needs most — the work in hand (up to three rows), what is next (two), what is
/// done (two).
pub(super) fn plan_row_grants(window: &[&DockPlanItem], wrapped: &[Vec<String>]) -> Vec<usize> {
    let wants: Vec<usize> = window
        .iter()
        .zip(wrapped)
        .map(|(item, rows)| {
            let cap = match item.status {
                TodoSummaryStatus::InProgress => 3,
                _ => 2,
            };
            rows.len().clamp(1, cap)
        })
        .collect();
    let mut grants = vec![1; window.len()];
    let mut spare = (PLAN_ROWS_MAX + PLAN_WRAP_ROWS).saturating_sub(window.len());
    let rank = |status: TodoSummaryStatus| match status {
        TodoSummaryStatus::InProgress => 0,
        TodoSummaryStatus::Pending | TodoSummaryStatus::Blocked => 1,
        TodoSummaryStatus::Completed => 2,
    };
    let mut order: Vec<usize> = (0..window.len()).collect();
    order.sort_by_key(|&index| rank(window[index].status));
    for index in order {
        let extra = (wants[index] - 1).min(spare);
        grants[index] += extra;
        spare -= extra;
    }
    grants
}

/// A subject word-wrapped to `width`, one string per row.
pub(super) fn subject_rows(subject: &str, width: usize) -> Vec<String> {
    if subject.width() <= width {
        return vec![subject.to_string()];
    }
    crate::widgets::wrap_line(&Line::from(subject.to_string()), width)
        .iter()
        .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
        .map(|row| row.trim_end().to_string())
        .collect()
}

/// `PLAN 2/5` — the heading carries the completion ratio so a collapsed glance still informs.
fn plan_heading_line(plan: &DockPlan, app: &App) -> Line<'static> {
    section_heading_with(Section::Plan, format!("{}/{}", plan.completed, plan.total), app)
}

/// One checklist row: tree connector, status glyph, subject.
///
/// A blocked row says so with its mark and nothing else: its blockers' ids appear nowhere on the
/// rail, and the row is too narrow to spare the cells.
///
/// A finished row is struck through as well as green, because green alone has to carry two jobs on
/// this rail — "done" here and "added" in Changes — and now that the ticks stay where they were
/// written, the strike is what a reader scans for. Decoration, not meaning: the ✓ and the words
/// beside it still say it, so a terminal that ignores the attribute loses shine.
///
/// A subject longer than its row wraps onto up to `rows` rows, hanging under its own first word with
/// the connector carried down beside it, and is cut on the last of them.
pub(super) fn todo_lines(
    item: &DockPlanItem,
    mut wrapped: Vec<String>,
    last: bool,
    subject_width: usize,
    rows: usize,
    app: &App,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let (glyph, style) = match item.status {
        TodoSummaryStatus::Completed => (crate::glyphs::Mark::Yes.glyph(), styles.success),
        TodoSummaryStatus::InProgress => {
            (crate::glyphs::Mark::Underway.glyph(), styles.text.add_modifier(Modifier::BOLD))
        }
        TodoSummaryStatus::Blocked => (crate::glyphs::Mark::Halted.glyph(), styles.warning),
        TodoSummaryStatus::Pending => ("○", styles.muted),
    };
    let subject_style = match item.status {
        TodoSummaryStatus::Completed => style.add_modifier(Modifier::CROSSED_OUT),
        _ => style,
    };
    let connector = if last { "└─" } else { "├─" };
    let rows = rows.max(1);
    if wrapped.len() > rows {
        let rest = wrapped.split_off(rows - 1).join(" ");
        wrapped.push(truncate_cells(&rest, subject_width));
    }
    let carried = if last { "   " } else { "│  " };
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let (lead, mark) = match index {
                0 => (format!("{connector} "), format!("{glyph} ")),
                _ => (carried.to_string(), " ".repeat(GLYPH_WIDTH)),
            };
            Line::from(vec![
                Span::styled(lead, styles.border.patch(styles.tool_bg)),
                Span::styled(mark, style.patch(styles.tool_bg)),
                Span::styled(text, subject_style.patch(styles.tool_bg)),
            ])
        })
        .collect()
}
