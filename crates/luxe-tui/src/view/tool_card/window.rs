//! Card window — how much of a body a card shows, open or closed, and the footer offering the rest.

use super::{
    CardBody, CardContext, DiffRow, RowLines, CARD_PREVIEW_LINES, CARD_QUERY_PREVIEW_LINES,
    CHANGE_LEAD_LINES,
};
use ratatui::text::{Line, Span};

/// How much of a body is drawn: the visual lines it may spend, and where its window opens.
#[derive(Debug, Clone, Copy)]
pub(super) struct Window {
    /// Visual lines the body may spend.
    pub(super) budget: usize,
    /// Rows passed over before the window opens. They are hidden, and expanding reveals them.
    pub(super) opens_at: usize,
}

impl Window {
    /// A window on the top of the body, which is where every body but a diff opens.
    pub(super) const fn top(budget: usize) -> Self {
        Self { budget, opens_at: 0 }
    }
}

/// Splits a wrapped row into one row per visual line.
///
/// The budget stops *before* a row that would breach it and always keeps the first, so a single
/// answer long enough to fill the card would otherwise be exempt from the cap every other body
/// obeys — and the reader would have no expand to undo it with.
pub(super) fn per_visual_line(lines: RowLines) -> impl Iterator<Item = RowLines> {
    lines.into_iter().map(|line| vec![line])
}

/// The budget for a body the reader has explicitly opened: all of it.
pub(super) fn expanded_budget(ctx: &CardContext<'_>, collapsed: usize) -> usize {
    if ctx.expanded {
        usize::MAX
    } else {
        collapsed
    }
}

/// How many visual lines this tool's body may spend while the card is closed.
pub(super) const fn collapsed_budget(tool: &str) -> usize {
    match tool.as_bytes() {
        b"web_search" | b"websearch" => CARD_QUERY_PREVIEW_LINES,
        _ => CARD_PREVIEW_LINES,
    }
}

/// Where a collapsed diff's window opens, as an index into its laid-out rows.
///
/// `0` — the top — unless the change would fall outside the budget, which a run of context ahead of
/// it is enough to do: a wrapped row spends three visual lines on one line of file, and ten of those
/// is a card of scenery with the edit below the fold. The window then opens a couple of lines above
/// the change, or on the change itself where even that will not fit.
pub(super) fn diff_window(rendered: &[DiffRow], budget: usize) -> usize {
    let Some(first) = rendered.iter().position(|row| row.changed) else {
        return 0;
    };
    let through: usize = rendered[..=first].iter().map(|row| row.lines.len()).sum();
    if through <= budget {
        return 0;
    }
    // Whatever the change's own row leaves, up to the lead — so a change that fills the card on its
    // own still opens on itself rather than on the line before it.
    let room = budget.saturating_sub(rendered[first].lines.len()).min(CHANGE_LEAD_LINES);
    let mut lead = 0usize;
    let mut opens_at = first;
    while opens_at > 0 {
        let height = rendered[opens_at - 1].lines.len();
        if lead + height > room {
            break;
        }
        lead += height;
        opens_at -= 1;
    }
    opens_at
}

/// Applies a visual-line budget and appends the footer that accounts for whatever it left out.
///
/// `total_lines` is what the *payload* has, which can exceed the rows rendered here — a written file
/// longer than the front-end carries. Two different shortfalls, so two different footers: one the
/// reader can undo with a keystroke, one they cannot.
///
/// Rows the window opens past are hidden exactly like the ones it stops short of: the count covers
/// both, and either is reason enough to offer the expand.
///
/// `unit` names what a row of this body is, because "2 more lines" under a list of search queries
/// counts the right number of the wrong thing.
pub(super) fn finish(
    rendered: Vec<RowLines>,
    total_lines: usize,
    held: usize,
    unit: Unit,
    window: Window,
    ctx: &CardContext<'_>,
) -> CardBody {
    let mut lines = Vec::new();
    let mut shown = 0usize;
    let mut visual = 0usize;
    for row in rendered.iter().skip(window.opens_at) {
        // Stop *before* a row that would breach the budget rather than after, so the number means
        // what it says however wide the row turned out to be. The first row goes in regardless: a
        // card that is only a footer says nothing at all.
        if shown > 0 && visual + row.len() > window.budget {
            break;
        }
        visual += row.len();
        shown += 1;
        lines.extend(row.iter().cloned());
    }
    let hidden = total_lines.saturating_sub(shown);
    if hidden == 0 {
        return CardBody { lines, truncated: false, flush: false };
    }
    // Expanding only helps when there are rows here it would reveal, and only when the card is not
    // already open. Otherwise the shortfall is the payload's, not the budget's. Counted against
    // what the front-end *holds* rather than what was laid out, because a bounded layout stops at
    // the budget and would otherwise report every long body as complete.
    let expandable = shown < held && !ctx.expanded;
    let mut footer = format!(
        "{} {hidden} more {} · {total_lines} total",
        crate::glyphs::Mark::Elision,
        unit.counting(hidden)
    );
    match (expandable, ctx.expand_chord.as_deref()) {
        // The card names both ways in, because a reader who reaches for the mouse and a reader who
        // reaches for a chord are looking at the same line.
        (true, Some(chord)) => footer.push_str(&format!(" · click or {chord} to expand")),
        (true, None) => footer.push_str(" · click to expand"),
        // Expanding cannot reveal what the front-end was never given; the tool's own note says
        // where the rest lives.
        (false, _) => footer.push_str(" · open the file for the rest"),
    }
    lines.push(Line::from(Span::styled(footer, ctx.styles.muted)));
    // Only a footer that offers the expand is worth clicking; "open the file for the rest" is a
    // note, and a click on it would do nothing.
    CardBody { lines, truncated: expandable, flush: false }
}

/// What one row of a card body is, for the footer that counts the rows it hid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unit {
    /// A line of a file, a command, or a diff.
    Line,
    /// One search a `web_search` call ran.
    Query,
}

impl Unit {
    /// The noun for `count` of them.
    const fn counting(self, count: usize) -> &'static str {
        match (self, count) {
            (Self::Line, 1) => "line",
            (Self::Line, _) => "lines",
            (Self::Query, 1) => "query",
            (Self::Query, _) => "queries",
        }
    }
}
