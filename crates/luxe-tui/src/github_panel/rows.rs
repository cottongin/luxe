//! The list side of `/github`: the tabs across the top, then the search a tab ran and each pull
//! request or issue it found on two lines.
//!
//! Line one is what it is — a state mark, the number, the title, how long ago it changed. Line two
//! is where it stands, quiet: who opened it, how big it is, and on the right its status in words.
//! Every mark sits beside a word, so nothing here depends on a glyph to be understood.

use luxe_core::github::overview::RowFacts;
use luxe_core::github::{rfc3339_seconds, Author, ListItem, ReviewDecision};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::GithubPanel;
use crate::github_detail::{ago, author_spans, diff_counts, lift_dim, state_word};
use crate::glyphs::{Icon, Mark};
use crate::theme::styles::ThemeStyles;

/// Lines each row takes: its two, and the gap under it.
pub(super) const ROW_LINES: usize = 3;

/// Where the search being typed starts on its line: after the `/ ` that says what the line is.
pub(super) const QUERY_INSET: u16 = 4;

/// Qualifiers worth knowing, offered under a search being typed, most useful first.
const QUALIFIERS: &[&str] = &[
    "is:closed",
    "is:merged",
    "is:draft",
    "label:bug",
    "author:@me",
    "assignee:@me",
    "review-requested:@me",
    "no:assignee",
    "sort:updated-desc",
    "sort:created-asc",
];

/// The tabs, on one line `width` wide: each named with its count, then without when the counts
/// will not fit, then a run of them around the open one, with `‹` and `›` saying more are aside.
pub(super) fn tab_line(panel: &GithubPanel, width: usize, styles: &ThemeStyles) -> Line<'static> {
    let Some(sections) = &panel.sections else {
        return Line::default();
    };
    let label = |index: usize, counted: bool| {
        let section = &sections[index];
        let edited = if panel.edited(index) { "*" } else { "" };
        let text = match counted {
            true => {
                let count = section.total.unwrap_or(section.items.len() as u64);
                format!("{}{edited} {}", section.name, grouped(count))
            }
            false => format!("{}{edited}", section.name),
        };
        match panel.icons.get(index) {
            Some(icon) => crate::glyphs::icon_label(icon.glyph(), &text),
            None => text,
        }
    };
    let cells = |labels: &[String]| -> usize {
        labels.iter().map(|label| label.width() + crate::widgets::SLAB_FRAME).sum()
    };
    let counted: Vec<String> = (0..sections.len()).map(|index| label(index, true)).collect();
    let labels = if cells(&counted) <= width {
        counted
    } else {
        (0..sections.len()).map(|index| label(index, false)).collect()
    };
    let open = panel.section.min(labels.len().saturating_sub(1));
    let (first, last) = window(&labels, open, width);
    let shown: Vec<&str> = labels[first..last].iter().map(String::as_str).collect();
    let mut line = crate::picker::tab_line(&shown, open - first, styles);
    if first > 0 {
        line.spans.insert(0, Span::styled("‹", styles.muted));
    }
    if last < labels.len() {
        line.spans.push(Span::styled("›", styles.muted));
    }
    line
}

/// The run of `labels` around `open` that fits `width` with room for a marker each side, as a
/// half-open range: the open one always, then a neighbor at a time, right first.
fn window(labels: &[String], open: usize, width: usize) -> (usize, usize) {
    let cells = |label: &String| label.width() + crate::widgets::SLAB_FRAME;
    let total: usize = labels.iter().map(cells).sum();
    if total <= width || labels.is_empty() {
        return (0, labels.len());
    }
    let room = width.saturating_sub(2);
    let (mut first, mut last) = (open, open + 1);
    let mut used = cells(&labels[open]);
    loop {
        let mut grew = false;
        if last < labels.len() && used + cells(&labels[last]) <= room {
            used += cells(&labels[last]);
            last += 1;
            grew = true;
        }
        if first > 0 && used + cells(&labels[first - 1]) <= room {
            used += cells(&labels[first - 1]);
            first -= 1;
            grew = true;
        }
        if !grew {
            return (first, last);
        }
    }
}

/// `count` with its thousands set apart: `3,250`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What the line over the rows says on its right: how many are here of how many there are, or
/// that the tab is being searched again.
pub(super) fn query_count(panel: &GithubPanel, frame: usize) -> String {
    let Some(section) = panel.sections.as_ref().and_then(|sections| sections.get(panel.section))
    else {
        return String::new();
    };
    if panel.researching(panel.section) {
        return format!("{} Searching…", crate::statusline::spinner(frame));
    }
    if section.error.is_some() {
        return String::new();
    }
    let loaded = section.items.len() as u64;
    if panel.editing.is_some() {
        let rows = panel.rows().len();
        return format!("{} of {} match", grouped(rows as u64), grouped(loaded));
    }
    match section.total {
        Some(total) if total > loaded => format!("{} of {}", grouped(loaded), grouped(total)),
        _ => format!("{} shown", grouped(loaded)),
    }
}

/// The list's lines for a pane `width` wide and `height` tall: the search, then its rows. A search
/// being typed is drawn over the first line by the caller, which owns the terminal's cursor.
///
/// Args: `(panel, width, height, focused, now, styles, spinner frame)`.
pub(super) fn list_lines(
    panel: &GithubPanel,
    width: usize,
    height: usize,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
    frame: usize,
) -> Vec<Line<'static>> {
    let Some(sections) = &panel.sections else {
        let spinner = crate::statusline::spinner(frame);
        return vec![Line::from(Span::styled(
            format!("{spinner} Fetching data from GitHub…"),
            styles.muted,
        ))];
    };
    let count = query_count(panel, frame);
    let query = match &panel.editing {
        Some(_) => String::new(),
        None => panel.shown_query(panel.section),
    };
    let query_style = if panel.edited(panel.section) { styles.text } else { styles.muted };
    let left = vec![
        Span::styled("  / ", styles.accent),
        Span::styled(if query.is_empty() { "everything".into() } else { query }, query_style),
    ];
    let mut lines = vec![Line::from(spread(left, vec![Span::styled(count, styles.muted)], width))];
    if panel.editing.is_some() {
        lines.push(Line::from(hints(width, styles)));
    }
    lines.push(Line::default());
    let head = lines.len();

    let Some(section) = sections.get(panel.section) else {
        return lines;
    };
    if let Some(error) = &section.error {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(crate::widgets::code_spans(error, styles.error, styles));
        lines.push(Line::from(spans));
        return lines;
    }
    if panel.researching(panel.section) {
        let spinner = crate::statusline::spinner(frame);
        lines
            .push(Line::from(Span::styled(format!("  {spinner} Searching GitHub…"), styles.muted)));
        return lines;
    }
    let rows = panel.rows();
    if rows.is_empty() {
        let said = match panel.editing {
            Some(_) => "No loaded row matches · enter searches GitHub",
            None => "Nothing matches this search",
        };
        lines.push(Line::from(Span::styled(format!("  {said}"), styles.muted)));
        return lines;
    }
    let more = more_line(panel, section, frame, styles);
    let room = height.saturating_sub(head + usize::from(more.is_some()));
    let fits = (room / ROW_LINES).max(1);
    let first = (panel.selected + 1).saturating_sub(fits);
    for (index, item) in rows.iter().enumerate().skip(first).take(fits) {
        let selected = index == panel.selected;
        let facts = item.facts.as_ref();
        lines.extend(row(
            item,
            facts,
            panel.elsewhere(item),
            width,
            selected,
            focused,
            now,
            styles,
        ));
        lines.push(Line::default());
    }
    if let Some(more) = more.filter(|_| first + fits >= rows.len()) {
        lines.push(more);
    }
    lines
}

/// The line under the last row when the search has more than it brought: how many, or that the
/// next page is on its way. Not while a search is being typed, when the rows are only a preview.
fn more_line(
    panel: &GithubPanel,
    section: &luxe_core::github::ListedSection,
    frame: usize,
    styles: &ThemeStyles,
) -> Option<Line<'static>> {
    if panel.editing.is_some() {
        return None;
    }
    if panel.paging(panel.section) {
        let spinner = crate::statusline::spinner(frame);
        return Some(Line::from(Span::styled(format!("  {spinner} Loading more…"), styles.muted)));
    }
    section.next.as_ref()?;
    let loaded = section.items.len() as u64;
    let left = section.total.unwrap_or(loaded).saturating_sub(loaded);
    let said = match left {
        0 => "More below".to_owned(),
        left => format!("{} more below", grouped(left)),
    };
    Some(Line::from(Span::styled(format!("  {said}"), styles.muted)))
}

/// Qualifiers for the line under a search being typed, as many as fit.
fn hints(width: usize, styles: &ThemeStyles) -> Vec<Span<'static>> {
    let lead = "    try ";
    let mut spans = vec![Span::styled(lead, styles.dim)];
    let mut used = lead.width();
    for (index, qualifier) in QUALIFIERS.iter().enumerate() {
        let gap = if index == 0 { 0 } else { 2 };
        if used + gap + qualifier.width() > width {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*qualifier, styles.muted));
        used += gap + qualifier.width();
    }
    spans
}

/// One pull request or issue on its two lines, cut into one slab when it is under the cursor.
///
/// Args: `(item, facts, elsewhere, width, selected, focused, now, styles)` — `focused` dims the
/// slab when the keys are in the detail, so the row still says which one the detail is about.
#[allow(clippy::too_many_arguments)]
fn row(
    item: &ListItem,
    facts: Option<&RowFacts>,
    elsewhere: bool,
    width: usize,
    selected: bool,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let room = width.saturating_sub(crate::widgets::SELECTION_FRAME);
    let issue = !item.pull_request;
    let (_, state_style) = state_word(item.state, item.draft, issue, styles);
    let icon = if issue { Icon::Issue } else { Icon::PullRequest };
    let mark = icon.prefix();
    let inset = mark.width();

    let age = rfc3339_seconds(&item.updated_at)
        .map(|at| ago(at, now).trim_end_matches(" ago").to_owned())
        .unwrap_or_default();
    let title_style = styles.label_on(selected && focused);
    let first = spread(
        vec![
            Span::styled(mark, state_style),
            Span::styled(format!("#{} ", item.number), styles.muted),
            Span::styled(item.title.clone(), title_style),
        ],
        vec![Span::styled(age, styles.muted)],
        room,
    );

    let author = Author { login: item.author.clone(), bot: facts.is_some_and(|f| f.author_bot) };
    let mut left = vec![Span::raw(" ".repeat(inset))];
    left.extend(author_spans(&author, styles));
    let dot = || Span::styled(" · ", styles.dim);
    if elsewhere {
        left.extend([dot(), Span::styled(item.repository.clone(), styles.muted)]);
    }
    let status = status(item, facts, styles);
    match (issue, facts) {
        (false, Some(facts)) => {
            // Whole or not at all: cut to fit, `+96 −31` came out `+…`, which says nothing
            let counts = diff_counts(facts.additions, facts.deletions, styles);
            let width = |spans: &[Span]| spans.iter().map(Span::width).sum::<usize>();
            if width(&left) + 3 + width(&counts) + 1 + width(&status) <= room {
                left.push(dot());
                left.extend(counts);
            }
        }
        (true, Some(facts)) if !facts.labels.is_empty() => {
            let names: Vec<&str> = facts.labels.iter().map(|label| label.name.as_str()).collect();
            left.extend([dot(), Span::styled(names.join(", "), styles.muted)]);
        }
        _ => {}
    }
    let second = spread(left, status, room);

    let (mut first, mut second) = (first, second);
    if selected && focused {
        lift_dim(&mut first, styles);
        lift_dim(&mut second, styles);
    }
    let mut lines =
        crate::widgets::selection_block(vec![first, second], width, selected && focused, styles);
    // With the keys in the detail, the row it is about keeps a cursor mark in the inset the slab
    // would have used, so the detail still says which row it belongs to
    if selected && !focused {
        if let Some(inset) = lines.first_mut().and_then(|line| line.spans.first_mut()) {
            *inset = Span::styled(Mark::Cursor.gutter(true), styles.accent);
        }
    }
    lines
}

/// Where it stands, in words, for the right of line two: `✗ 1 failing · changes requested`.
fn status(item: &ListItem, facts: Option<&RowFacts>, styles: &ThemeStyles) -> Vec<Span<'static>> {
    let mut parts: Vec<Span<'static>> = Vec::new();
    let mut push = |text: String, style| {
        if !parts.is_empty() {
            parts.push(Span::styled(" · ", styles.dim));
        }
        parts.push(Span::styled(text, style));
    };
    let (word, style) = state_word(item.state, item.draft, !item.pull_request, styles);
    if item.draft || item.state != luxe_core::github::PullRequestState::Open {
        push(word.to_owned(), style);
    }
    let Some(facts) = facts else {
        return parts;
    };
    if !item.pull_request {
        match facts.linked_pull_request {
            Some(number) => push(format!("{} PR #{number}", Mark::Waiting.glyph()), styles.warning),
            None if facts.comments > 0 => {
                let noun = if facts.comments == 1 { "comment" } else { "comments" };
                push(format!("{} {noun}", facts.comments), styles.muted);
            }
            None => {}
        }
        return parts;
    }
    let checks = &facts.checks;
    if checks.failed > 0 {
        push(format!("{} {} failing", Mark::No.glyph(), checks.failed), styles.error);
    } else if checks.pending > 0 {
        push(format!("{} running", Mark::Waiting.glyph()), styles.warning);
    } else if checks.passed > 0 {
        push(format!("{} checks", Mark::Yes.glyph()), styles.success);
    }
    match facts.review_decision {
        Some(ReviewDecision::ChangesRequested) => push("changes requested".into(), styles.error),
        Some(ReviewDecision::Approved) => push("approved".into(), styles.success),
        Some(ReviewDecision::ReviewRequired) => push("needs review".into(), styles.muted),
        None => {}
    }
    parts
}

/// `left`, then `right` against the edge of `room`, the left cut short to make the room.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, room: usize) -> Vec<Span<'static>> {
    let right_cells: usize = right.iter().map(|span| span.content.width()).sum();
    let budget = room.saturating_sub(right_cells + usize::from(right_cells > 0));
    let mut spans = Vec::new();
    let mut used = 0;
    for span in left {
        let cells = span.content.width();
        if used + cells <= budget {
            used += cells;
            spans.push(span);
            continue;
        }
        let cut = crate::widgets::truncate_end(&span.content, budget.saturating_sub(used));
        used += cut.width();
        spans.push(Span::styled(cut, span.style));
        break;
    }
    spans.push(Span::raw(" ".repeat(room.saturating_sub(used + right_cells))));
    spans.extend(right);
    spans
}
