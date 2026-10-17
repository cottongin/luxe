//! Folded runs — a run of lookups drawn as one line, its rows, and the one still working.

use super::message::{clip_line, line_text};
use super::tool::{format_tool_duration, indented, safe_tool_text, BODY_INDENT};
use crate::app::{ConversationItem, ConvoBlock};
use crate::glyphs::icon_label;
use crate::theme::styles::ThemeStyles;
use crate::widgets::{tool_icon, GENERIC_TOOL};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

/// Cells the run's meter occupies while it is filling.
const GROUP_METER_CELLS: usize = 10;

/// Whether a folded run is still going, so its band stays neutral rather than green.
///
/// A run with nothing in flight has not necessarily finished: dispatch is sequential, so between
/// one call ending and the next starting there is a moment when none is running. Asking only that
/// question washed the line green and back on every call boundary — several flashes a second, on a
/// line whose whole job is to be the calm one.
pub(in crate::view) fn run_is_working(items: &[ConversationItem], arriving: bool) -> bool {
    arriving
        || items.iter().any(|item| {
            matches!(item.block(), ConvoBlock::Tool(tool)
                if tool.activity.state == luxe_core::ToolActivityState::Running)
        })
}

/// One row of a folded run, and whether a click on it opens the run.
pub(in crate::view) struct GroupRow {
    pub(crate) line: Line<'static>,
    /// The header and the footer that offers the rest are handles; the list between them is text
    /// to select, the way a card's body is
    pub(crate) handle: bool,
}

/// Calls a folded run lists by name before it counts the rest.
///
/// Five when that is all there is; four and a count past that, so the list never runs longer than
/// five rows and a run of six does not list five and then say "1 more".
const GROUP_PREVIEW_ROWS: usize = 5;

/// What a folded run draws: its header, then the first few calls by name.
///
/// The header is the run in one line, live or settled. While the run is going it is a progress
/// line: a spinner, a meter over the run its message asked for, and the call happening now. That is
/// the whole reason to fold live rather than after the fact — the reader watches one line move
/// instead of seven cards land. Once it settles the meter goes away, since a bar frozen at full is
/// a picture of something no longer happening, and what is left worth saying is how many calls it
/// was and how long they took.
///
/// The list under it is what the count stands for. A header alone — `3 read calls` — says how
/// much and never what.
///
/// `arriving` is what keeps live and settled apart in the gap between calls, where nothing is in
/// flight and the run is nowhere near done. Without it the line summarizes itself as "1 read call"
/// for a frame, every call, all the way down a run of seven.
///
/// Args: `(items, arriving, spinner frame, width, styles)`; `width` is the cells a row may take.
pub(in crate::view) fn tool_group_lines(
    items: &[ConversationItem],
    arriving: bool,
    spinner_frame: usize,
    width: u16,
    styles: &ThemeStyles,
) -> Vec<GroupRow> {
    let calls: Vec<&luxe_core::ToolActivity> = items
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::Tool(tool) => Some(&tool.activity),
            _ => None,
        })
        .collect();
    let Some(first) = calls.first() else {
        return Vec::new();
    };
    let first_name = first.name.as_str();
    let same_tool = calls.iter().all(|call| call.name == first_name);
    let header = group_header(&calls, same_tool, arriving, spinner_frame, styles);
    let mut rows = vec![GroupRow { line: header, handle: true }];

    // A run its message promised is drawn at its promised size from the first call, rows waiting
    // for the calls still to come, so nothing above it moves as they land
    let total = match arriving {
        true => calls
            .iter()
            .filter_map(|call| call.run)
            .map(|run| usize::try_from(run.total).unwrap_or(usize::MAX))
            .max()
            .unwrap_or(0)
            .max(calls.len()),
        false => calls.len(),
    };
    let listed = match total > GROUP_PREVIEW_ROWS {
        true => GROUP_PREVIEW_ROWS - 1,
        false => total,
    };
    let room = usize::from(width).saturating_sub(BODY_INDENT.len()).max(1);
    for index in 0..listed {
        let line = match calls.get(index) {
            Some(call) => clip_with_ellipsis(group_row_label(call, same_tool, styles), room),
            None => Line::from(Span::styled("…", styles.muted)),
        };
        rows.push(GroupRow { line: indented(line), handle: false });
    }
    if total > listed {
        let footer = format!("⋯ {} more · {total} total · click to expand", total - listed);
        let footer = indented(Line::from(Span::styled(footer, styles.muted)));
        rows.push(GroupRow { line: footer, handle: true });
    }
    rows
}

/// The run's one-line header: progress while it works, a summary once it settles.
fn group_header(
    calls: &[&luxe_core::ToolActivity],
    same_tool: bool,
    arriving: bool,
    spinner_frame: usize,
    styles: &ThemeStyles,
) -> Line<'static> {
    let first_name = calls[0].name.as_str();
    // A run only forms from calls that are running or done, so the icon says which tool is
    // working (or that the run is mixed) rather than repeating an outcome the wash already carries.
    let icon = if same_tool { tool_icon(first_name).glyph() } else { GENERIC_TOOL.glyph() };

    // The call in flight, or — in the gap before the next one starts — the most recent, which is
    // the one whose place in the run the meter is still honestly reporting.
    let running = calls
        .iter()
        .find(|call| call.state == luxe_core::ToolActivityState::Running)
        .or_else(|| calls.last().filter(|_| arriving));
    if let Some(running) = running {
        return running_group_line(running, spinner_frame, styles);
    }

    // Only lookups and one server's calls fold, so a mixed run has a real word for what it was —
    // better than "tool calls", which would describe every card in the transcript equally well.
    let mut description = match same_tool {
        true => format!("{} {first_name} calls", calls.len()),
        false => format!("{} lookups", calls.len()),
    };
    if let Some(server) = calls.iter().find_map(|call| crate::app::mcp_server(&call.label)) {
        if first_name == "mcp" {
            description.push_str(&format!(" · {server}"));
        }
    }
    let mut spans = vec![Span::styled(
        format!("{} {}", crate::glyphs::Mark::FoldClosed, icon_label(icon, &description)),
        styles.success,
    )];
    let elapsed: u64 = calls.iter().filter_map(|call| call.elapsed_ms).sum();
    if elapsed > 0 {
        spans.push(Span::styled(format!("  {}", format_tool_duration(elapsed)), styles.muted));
    }
    Line::from(spans)
}

/// One call in a folded run's list: what it was about, under the tool's name when the run mixes
/// tools. An MCP call drops the server its header already names.
fn group_row_label(
    call: &luxe_core::ToolActivity,
    same_tool: bool,
    styles: &ThemeStyles,
) -> Line<'static> {
    let label = safe_tool_text(&call.label);
    let subject = match (call.name.as_str(), crate::app::mcp_server(&label)) {
        ("mcp", Some(server)) => label[server.len() + 1..].to_string(),
        _ => label.clone(),
    };
    let subject = match subject.trim().is_empty() {
        true => call.name.clone(),
        false => subject,
    };
    let mut spans = Vec::new();
    if !same_tool {
        spans.push(Span::styled(format!("{}  ", call.name), styles.muted));
    }
    spans.push(Span::styled(subject, styles.text));
    Line::from(spans)
}

/// `line` cut to `width` cells, with an ellipsis where it was cut.
fn clip_with_ellipsis(line: Line<'static>, width: usize) -> Line<'static> {
    if crate::selection::cell_width(&line_text(&line)) <= width {
        return line;
    }
    let mut clipped = clip_line(line, width.saturating_sub(1));
    let style = clipped.spans.last().map(|span| span.style).unwrap_or_default();
    clipped.spans.push(Span::styled("…", style));
    clipped
}

/// The folded run while it is still moving: spinner, meter, position, and the call in flight.
///
/// Named for the call *in flight*, even where the run is mixed. A live line's job is to say what is
/// happening now, and a run summarized as "lookups" while it works spends its one line on a word
/// the reader could have guessed. The summary belongs on the settled line, which is a summary.
fn running_group_line(
    running: &luxe_core::ToolActivity,
    spinner_frame: usize,
    styles: &ThemeStyles,
) -> Line<'static> {
    let icon = tool_icon(&running.name).glyph();
    let mut spans = vec![Span::styled(
        format!(
            "{} {}",
            crate::statusline::spinner(spinner_frame),
            icon_label(icon, &running.name)
        ),
        styles.text.add_modifier(Modifier::BOLD),
    )];
    // The denominator is the run the *message* asked for, which is known before any of it has been
    // dispatched — so the line can say "3 of 7" while the seventh call is still hypothetical.
    if let Some(batch) = running.batch {
        let percent = 100.0 * f32::from(u16::try_from(batch.index).unwrap_or(u16::MAX))
            / f32::from(u16::try_from(batch.total).unwrap_or(u16::MAX)).max(1.0);
        let shades = styles.gauge_shades(styles.accent, styles.tool_call_bg);
        spans.push(Span::raw("  "));
        spans.extend(crate::widgets::fill_gauge(
            percent,
            GROUP_METER_CELLS,
            &shades.fill,
            &shades.rail,
        ));
        // The count is the part that has to survive a bare monospace font; the meter is the shine.
        spans.push(Span::styled(format!("  {}/{}", batch.index, batch.total), styles.muted));
    }
    let label = safe_tool_text(&running.label);
    if !label.trim().is_empty() {
        spans.push(Span::styled(format!("  {label}"), styles.text));
    }
    Line::from(spans)
}
