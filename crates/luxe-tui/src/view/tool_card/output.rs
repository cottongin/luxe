//! Card output — errors, test results and raw output.

use super::rows;
use super::window::{expanded_budget, finish, Unit, Window};
use super::{
    code_width, note_lines, CardBody, CardContext, RowLines, CARD_PREVIEW_LINES,
    OUTPUT_PREVIEW_LINES,
};
use ratatui::text::{Line, Span};

/// Failing tests a test card names before it counts the rest.
const TEST_FAILURES_SHOWN: usize = 3;

/// A test run as a card: the counts, then each failing test with where it is and the first line of
/// why.
///
/// Not the runner's whole output: forty red lines read as something having gone badly wrong even
/// when it is the failing test a test-first fix starts from. What a reader needs is how many,
/// which, and why — three lines for one failure.
pub(super) fn test_body(
    summary: &luxe_core::compression::TestSummary,
    ctx: &CardContext<'_>,
) -> CardBody {
    let styles = ctx.styles;
    let width = code_width(usize::from(ctx.width), 0);
    let mut counts = vec![Span::styled(format!("{} passed", summary.passed), styles.success)];
    if summary.failed > 0 {
        counts.push(Span::styled(" · ", styles.muted));
        counts.push(Span::styled(format!("{} failed", summary.failed), styles.error));
    }
    if summary.ignored > 0 {
        counts.push(Span::styled(format!(" · {} skipped", summary.ignored), styles.muted));
    }
    let mut lines = crate::widgets::wrap_line(&Line::from(counts), width);
    for failure in summary.failures.iter().take(TEST_FAILURES_SHOWN) {
        let mut row = vec![
            Span::styled(crate::glyphs::icon_prefix(crate::glyphs::Mark::No.glyph()), styles.error),
            Span::styled(rows::sanitize(&failure.name), styles.text),
        ];
        if let Some(location) = &failure.location {
            row.push(Span::styled(format!("  {}", rows::sanitize(location)), styles.muted));
        }
        lines.extend(crate::widgets::wrap_line(&Line::from(row), width));
        let why = failure
            .message
            .as_deref()
            .and_then(|message| message.lines().map(str::trim).find(|line| !line.is_empty()));
        if let Some(why) = why {
            let why = Line::from(Span::styled(format!("  {}", rows::sanitize(why)), styles.muted));
            lines.extend(crate::widgets::wrap_line(&why, width));
        }
    }
    if summary.failures.len() > TEST_FAILURES_SHOWN {
        let more = summary.failures.len() - TEST_FAILURES_SHOWN;
        lines.push(Line::from(Span::styled(format!("+{more} more failing"), styles.muted)));
    }
    CardBody { lines, truncated: false, flush: false }
}

/// What a card with no payload of its own shows when the reader opens it: the raw call, then a
/// bounded window onto what came back.
///
/// Bounded even when expanded, unlike a body. A body is the point of its card, so opening one shows
/// all of it; a `read`'s output is reference material the reader never asked for line by line — and
/// the expand toggle opens every card at once, so an unbounded window here turns one keystroke into
/// a wall of file contents.
pub(super) fn opened_output(
    activity: &luxe_core::ToolActivity,
    output: &str,
    ctx: &CardContext<'_>,
) -> CardBody {
    // The raw call, which is what you want when a tool did something you did not expect.
    let arguments = activity.input_preview.as_deref().unwrap_or(&activity.label);
    let mut lines = note_lines(&format!("arguments: {}", rows::sanitize(arguments)), ctx);
    // Labeled on its first row only, so the reader can tell the call from what came back without
    // spending a whole row of the card on the word.
    let rendered: Vec<RowLines> = output
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let text = rows::sanitize(line);
            note_lines(&if index == 0 { format!("output: {text}") } else { text }, ctx)
        })
        .collect();
    let total = rendered.len();
    let window = finish(rendered, total, total, Unit::Line, Window::top(OUTPUT_PREVIEW_LINES), ctx);
    lines.extend(window.lines);
    CardBody { lines, truncated: window.truncated, flush: false }
}

/// A failed call's own message under its card, capped by the same policy as everything else.
pub(super) fn error_body(output: &str, summary: Option<&str>, ctx: &CardContext<'_>) -> CardBody {
    let mut lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<std::collections::VecDeque<_>>();
    // The card's summary is already the first line of this text; a guard whose summary was reworded
    // keeps its full instruction.
    if lines.front().is_some_and(|first| repeats_summary(first, summary)) {
        lines.pop_front();
    }
    let rendered: Vec<RowLines> = lines
        .into_iter()
        .map(|line| {
            crate::widgets::wrap_cells(
                vec![Span::styled(rows::sanitize(line), ctx.styles.error)],
                code_width(usize::from(ctx.width), 0),
            )
            .into_iter()
            .map(Line::from)
            .collect()
        })
        .collect();
    let total = rendered.len();
    let budget = expanded_budget(ctx, CARD_PREVIEW_LINES);
    finish(rendered, total, total, Unit::Line, Window::top(budget), ctx)
}

/// Drops the tool's opening sentence when the header already said it.
///
/// `edit` leads with `Applied 1 edit(s) to '<path>'` under a header that reads `± edit <path>`. Saying
/// it twice costs a row of the card and tells the reader nothing.
pub(super) fn strip_restated_summary(mut parsed: Vec<rows::Row>) -> Vec<rows::Row> {
    let restates =
        matches!(parsed.first(), Some(rows::Row::Note(note)) if note.starts_with("Applied "));
    if restates {
        parsed.remove(0);
    }
    // A leading blank the summary left behind is not a row of the diff.
    while matches!(parsed.first(), Some(rows::Row::Note(note)) if note.trim().is_empty()) {
        parsed.remove(0);
    }
    parsed
}

/// Whether an error line says what the card's summary already said.
fn repeats_summary(line: &str, summary: Option<&str>) -> bool {
    let Some(summary) = summary.map(|summary| summary.trim_end_matches('…').trim()) else {
        return false;
    };
    let line = line.trim().strip_prefix("Error:").unwrap_or(line).trim();
    !summary.is_empty() && line.starts_with(summary)
}
