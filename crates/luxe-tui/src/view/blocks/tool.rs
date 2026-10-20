//! Tool blocks — a tool card's header: its icon, subject, summary and duration.

use super::message::mark_handles;
use super::{image_lines, layout_lines, tool_card, BlockContext};
use crate::app::ToolItem;
use crate::glyphs::{icon_label, Icon};
use crate::markdown::{MarkdownBlockKind, MarkdownLine};
use crate::theme::styles::ThemeStyles;
use crate::widgets::tool_glyph;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// A tool call's card: its header, its body (indented unless it draws flush), its handles, and
/// any picture it returned.
pub(super) fn tool_lines(
    item: &ToolItem,
    style: Style,
    ctx: &BlockContext<'_>,
) -> Vec<MarkdownLine> {
    let (styles, spinner_frame) = (ctx.styles, ctx.spinner_frame);
    let activity = &item.activity;
    // The band's success/error wash already carries the outcome, so a finished call shows
    // its *tool's* icon instead of another tick. Cancelled and interrupted calls keep a
    // state glyph — their band stays neutral, so nothing else says they never finished.
    let running = crate::statusline::spinner(spinner_frame).to_string();
    let icon = card_icon(activity, ctx.waiting_on_user, &running);
    // "Completed" is what a green card means; only a summary that adds information
    // earns a slot — a `write` trades it for what the write actually did.
    let outcome = tool_card::header_summary(activity);
    let summary = outcome.as_deref().or_else(|| {
        activity.summary.as_deref().filter(|summary| !summary.eq_ignore_ascii_case("completed"))
    });
    // Where this call sits in the run its message asked for — while it is *in* that run.
    // A card that never folds is one the reader is meant to look at, and the one thing the
    // card cannot otherwise tell them is how many more are queued behind it. Gone the
    // moment it lands: a finished call has no queue left to be part of.
    let queued = (activity.state == luxe_core::ToolActivityState::Running)
        .then_some(activity.batch)
        .flatten();
    let header = tool_header_line(
        ToolHeader {
            icon,
            tool: &activity.name,
            subject: header_subject(activity),
            elapsed_ms: header_duration(item, ctx.running_ms),
            summary,
            queued,
        },
        style,
        styles,
    );
    let mut lines = Vec::new();
    // The change, the file, or the command — always visible for the tools that have one,
    // never gated on expand, because seeing it is the whole point of the card. Everything
    // else stays a one-line card until the reader opens it. No `result:` row either way:
    // the header carries the summary, and "Completed" is what a green card means.
    let card = tool_card::body(item, &ctx.card());
    let truncated = card.truncated;
    if card.flush {
        lines.extend(card.lines);
    } else {
        lines.extend(card.lines.into_iter().map(indented));
    }
    let mut laid = vec![header_layout(header, icon, &activity.name)];
    laid.extend(layout_lines(lines, MarkdownBlockKind::Wide));
    // The header is the card's handle, and a footer that offers the expand says "click" in
    // so many words. The body between them is text to select.
    mark_handles(&mut laid, truncated);
    // Never gated on expand. For a `screenshot` the picture *is* the result, and a payload
    // hidden behind a keystroke is the mistake the card bodies already stopped making.
    let call = (activity.session_id.clone(), activity.call_id.clone());
    for key in ctx.tool_images.get(&call).into_iter().flatten() {
        let rows = image_lines(key, ctx, style);
        if !rows.is_empty() {
            laid.push(MarkdownLine::wide(Line::default()));
            laid.extend(rows);
        }
    }
    laid
}

/// The glyph a card's header leads with: a spinner while it works, a state mark when it stopped
/// short, and otherwise its tool's own icon.
fn card_icon<'a>(
    activity: &luxe_core::ToolActivity,
    waiting_on_user: bool,
    running: &'a str,
) -> &'a str {
    match activity.state {
        // The tool running and the model writing the call are both work in progress, and
        // the card says so the same way for each — a spinner over a card that is visibly
        // filling in, and a queued mark only over one that genuinely is not moving. A
        // model writes a message's calls in order and the engine runs them in order, and a
        // call written while another runs arrives queued, so exactly one card spins at a time.
        // Except the call that is waiting on the reader. A spinner says the agent is busy, and
        // over a question it is waiting on you to answer it only adds pressure; the call's
        // own glyph, in the same running color, says it is open without hurrying anyone
        luxe_core::ToolActivityState::Running if waiting_on_user => tool_glyph(&activity.name),
        luxe_core::ToolActivityState::Running | luxe_core::ToolActivityState::Composing => running,
        luxe_core::ToolActivityState::Queued => Icon::Queued.glyph(),
        luxe_core::ToolActivityState::Cancelled => crate::glyphs::Mark::Halted.glyph(),
        luxe_core::ToolActivityState::Interrupted => "!",
        _ => tool_glyph(&activity.name),
    }
}

/// A tool card's header, laid out to wrap under its own subject rather than back at the margin.
fn header_layout(line: Line<'static>, header_icon: &str, tool: &str) -> MarkdownLine {
    let mut layout = MarkdownLine::wide(line);
    layout.hang = Some(icon_label(header_icon, tool).width() + SUBJECT_GAP.len());
    layout
}

/// What a tool card's one line is made of.
struct ToolHeader<'a> {
    /// The tier's glyph for this tool, or empty where the tier draws none.
    icon: &'a str,
    /// Registered tool name.
    tool: &'a str,
    /// The command, the path, the pattern — what the call was about.
    subject: &'a str,
    /// How long the call took, once it has taken it.
    elapsed_ms: Option<u64>,
    /// An outcome worth a slot, for the tools whose result says more than a green wash.
    summary: Option<&'a str>,
    /// Where the call sits in the run its message asked for, while it is still in that run.
    queued: Option<luxe_core::ToolBatch>,
}

/// The one line every tool card leads with: `{icon} {tool}  {subject}  {n/N}  {duration} · {summary}`.
///
/// Three voices, so a glance lands on the subject rather than on the clock. The icon and tool name
/// take the card's state color in bold (yellow while it runs, green/red once it lands); the subject
/// — the command, the path, the pattern — takes plain body text, because that is the part worth
/// reading; the duration is muted and held off by a double space, since how long a call took is the
/// least of it. A summary that adds something rides at the end in the state color.
fn tool_header_line(header: ToolHeader<'_>, style: Style, styles: &ThemeStyles) -> Line<'static> {
    let name_style = style.add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(icon_label(header.icon, header.tool), name_style)];
    let subject = safe_tool_text(header.subject);
    if !subject.trim().is_empty() {
        spans.push(Span::styled(format!("{SUBJECT_GAP}{subject}"), styles.text));
    }
    if let Some(batch) = header.queued {
        spans.push(Span::styled(format!("  {}/{}", batch.index, batch.total), styles.muted));
    }
    if let Some(elapsed) = header.elapsed_ms {
        spans.push(Span::styled(format!("  {}", format_tool_duration(elapsed)), styles.muted));
    }
    let summary = header.summary.map(safe_tool_text).filter(|text| !text.trim().is_empty());
    if let Some(summary) = summary {
        spans.push(Span::styled(format!(" · {summary}"), style));
    }
    Line::from(spans)
}

/// What the header says the call was about, or nothing when the body below it already says it.
///
/// A header subject is a *preview*, for a card that shows nothing else. Where the body is the same
/// payload the subject was cut out of — a `bash` command, the queries a `web_search` ran — the
/// header's ellipsized copy of its first line sits directly above the real thing, syntax
/// highlighted and in full. Two renderings of one string, the worse one on top.
///
/// Not every card with a body restates it: a `write`'s subject is the path and its body is the
/// file, an `edit`'s is the path and its body is the diff. Those say different things, and so does
/// a described `bash` call: its subject is the description, its body the command, folded.
fn header_subject(activity: &luxe_core::ToolActivity) -> &str {
    let restated = activity.card_body.as_ref().is_some_and(|body| body.collapsed_lines.is_none())
        && matches!(activity.name.as_str(), "bash" | "web_search" | "websearch");
    if restated {
        return "";
    }
    &activity.label
}

/// How long the call took, counting only the part the *tool* spent.
///
/// A call gated by a permission prompt is timed from dispatch to result, most of which was the
/// reader deciding — a `bash` that ran in a second reports thirty if they took thirty to approve
/// it, which is a card telling them about themselves.
///
/// A `question` is that case taken to its limit: the whole call is a person thinking, so there is
/// no tool time to report and the figure is dropped. It was also actively in the way — a `question`
/// lays its answer out as a *peer* of the header, so a long prompt wrapped and dropped the duration
/// between the question and the answer to it.
fn header_duration(item: &crate::app::ToolItem, running_ms: Option<u64>) -> Option<u64> {
    if item.activity.name == "question" {
        return None;
    }
    // A call in flight is timed by the front-end: the projector only measures one once it has
    // finished, and a card would report nothing for the whole time the answer matters most
    let running = matches!(item.activity.state, luxe_core::ToolActivityState::Running);
    let measured = match running {
        true => running_ms,
        false => item.activity.elapsed_ms,
    }?;
    let elapsed = measured.saturating_sub(item.waited_on_user_ms);
    // Below a second a live card has nothing to say but milliseconds, and those are three digits
    // rewritten on every frame. A finished call keeps them: that figure holds still, and `842ms`
    // is the only honest reading of a call that fast.
    (!running || elapsed >= 1_000).then_some(elapsed)
}

/// The clearance between a card header's tool name and what the call was about.
const SUBJECT_GAP: &str = "  ";

/// The column a card header's subject starts in, measured at the tier that is drawing it.
///
/// For a body row that belongs beside the header rather than under it — a question's answer is the
/// other half of an exchange, not content the card is previewing — so that it lands on the same
/// column however wide the terminal's icons turn out to be, or whether it draws them at all.
pub(in crate::view) fn header_subject_column(tool: &str) -> usize {
    icon_label(tool_glyph(tool), tool).width() + SUBJECT_GAP.len()
}

/// Insets a card body row under its header, so the body reads as belonging to the card.
pub(super) fn indented(line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(BODY_INDENT));
    spans.extend(line.spans);
    Line::from(spans)
}

/// Cells a card body sits in from the card's left edge.
pub(in crate::view) const BODY_INDENT: &str = "   ";

/// Formats a tool call's duration at a scale a reader can act on.
///
/// A bare millisecond count reads badly at both ends: `0ms` looks like the call never ran (it was
/// simply faster than the clock's resolution — a small `read`, a rejected `edit`), and `47608ms`
/// makes the reader do arithmetic to notice it waited 47 seconds.
///
/// **Whole seconds once past one**, and that is about motion rather than precision. A card in
/// flight is re-timed on every frame, so a tenths digit is a figure changing ten times a second in
/// the corner of the eye — which is what a spinner is for, and this is not the spinner. Nobody
/// acts on the tenth of a second a call took; they act on whether it was one second or forty.
pub(super) fn format_tool_duration(elapsed_ms: u64) -> String {
    match elapsed_ms {
        0 => "<1ms".to_string(),
        ms if ms < 1_000 => format!("{ms}ms"),
        ms => crate::statusline::format_elapsed_compact(std::time::Duration::from_millis(ms)),
    }
}

pub(super) fn safe_tool_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .collect()
}
