//! `/context` — the window, resolved into what is in it.
//!
//! Nothing here is charted. Every row already carries its tokens *and* its share, so a bar would be
//! a third drawing of a number printed twice — ink, and no information. The rows are set as columns
//! instead, ranked largest first, because the question a full window raises is *what is biggest*.
//!
//! The Context Dock reaches the same answer from the other side. It drew occupancy as a ten-cell
//! meter for a while, on the theory that a level against a limit has no room beside it for a figure
//! — except the row printed the figure anyway. What survives there is the severity hue, which is
//! the part that was never available from the number.
//!
//! Findings come last and name a lever apiece. A report with nothing to act on says so in one line
//! rather than printing an empty heading.

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use luxe_core::context_report::{
    format_tokens, Calibration, ContextComposition, ContextSlice, FindingLevel,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Width of the label column — the longest part label plus a separating space.
const LABEL_WIDTH: usize = 14;

/// Width of the token column, right-aligned so the figures line up under each other.
const TOKENS_WIDTH: usize = 7;

/// Share of its own part a named item must reach to earn a line.
const DETAIL_SHARE_PCT: u64 = 15;

/// Tokens a named item must reach regardless of share, so a tiny session names nothing.
const DETAIL_FLOOR_TOKENS: u64 = 200;

/// Width a detail label gets before it is elided.
///
/// Wider than the part label above it because a detail is the row worth acting on, and `read` of
/// *which* file is the whole content of the line.
const DETAIL_WIDTH: usize = 34;

/// Occupancy past which the headline takes the warning hue, matching the dock's own thresholds.
const TIGHT_PCT: f32 = 75.0;
/// Occupancy past which it takes the error hue.
const FULL_PCT: f32 = 90.0;

/// Renders the report as styled lines, ready for the transcript.
pub(crate) fn lines(report: &ContextComposition, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut out = vec![headline(report, styles)];

    if report.slices.is_empty() {
        out.push(muted("Nothing in the window yet.", styles));
        return out;
    }

    out.push(Line::default());
    for slice in &report.slices {
        out.push(slice_row(report, slice, styles));
        for detail in &slice.detail {
            // Only the outliers earn a line of their own: a detail list that reprints every tool
            // is an inventory, and an inventory is not a decision.
            if worth_naming(slice.tokens, detail.tokens) {
                out.push(detail_row(&detail.label, detail.tokens, styles));
            }
        }
    }
    if let Some(free) = report.free_tokens() {
        out.push(free_row(free, styles));
    }

    if let Some(mcp) = &report.mcp {
        out.push(Line::default());
        out.push(muted(
            format!(
                "{} MCP server{} behind one {} gateway tool.",
                mcp.servers,
                if mcp.servers == 1 { "" } else { "s" },
                format_tokens(mcp.gateway_tokens)
            ),
            styles,
        ));
    }

    out.push(Line::default());
    out.push(muted(method(report), styles));

    if !report.findings.is_empty() {
        out.push(Line::default());
        for finding in &report.findings {
            out.extend(finding_lines(&finding.text, level_style(finding.level, styles), styles));
        }
    }
    out
}

/// `47.2k of 200k · 24% used`, in the hue the occupancy has earned.
fn headline(report: &ContextComposition, styles: &ThemeStyles) -> Line<'static> {
    let style = occupancy_style(report, styles).add_modifier(Modifier::BOLD);
    let text = match (report.used_pct(), report.context_window) {
        (Some(pct), window) => format!(
            "{} of {} · {pct:.0}% used",
            format_tokens(report.used_tokens),
            format_tokens(u64::from(window))
        ),
        // A locally served model whose catalog entry publishes no window: report what is in it and
        // claim no percentage, rather than inventing a denominator.
        _ => format!("{} in the window · no published size", format_tokens(report.used_tokens)),
    };
    Line::from(vec![
        Span::styled(Icon::Model.prefix(), styles.muted),
        Span::styled(report.model.clone(), styles.text.add_modifier(Modifier::BOLD)),
        Span::styled("   ", styles.text),
        Span::styled(text, style),
    ])
}

/// `conversation    31.4k     66%`
fn slice_row(
    report: &ContextComposition,
    slice: &ContextSlice,
    styles: &ThemeStyles,
) -> Line<'static> {
    // One hue for every row: the figure says how big and the label says what. Coloring rows by
    // category would make position mean something it does not — the mistake the dashboard menu
    // already un-learned.
    Line::from(vec![
        Span::styled(format!("  {:<LABEL_WIDTH$}", slice.part.label()), styles.text),
        Span::styled(
            format!("{:>TOKENS_WIDTH$}", format_tokens(slice.tokens)),
            styles.text.add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{:>5.0}%", report.share_pct(slice)), styles.muted),
    ])
}

/// A named item under its part, indented so it reads as belonging to the row above.
fn detail_row(label: &str, tokens: u64, styles: &ThemeStyles) -> Line<'static> {
    let label = elide_middle(label, DETAIL_WIDTH);
    let pad = DETAIL_WIDTH.saturating_sub(label.width()) + DETAIL_GAP;
    Line::from(vec![
        Span::styled("      ".to_string(), styles.muted),
        Span::styled(label, styles.muted),
        Span::styled(" ".repeat(pad), styles.muted),
        Span::styled(format_tokens(tokens), styles.muted),
    ])
}

/// What is left, set apart from the parts because it is the one row that is not occupied.
fn free_row(free: u64, styles: &ThemeStyles) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<LABEL_WIDTH$}", "free"), styles.muted),
        Span::styled(format!("{:>TOKENS_WIDTH$}", format_tokens(free)), styles.muted),
    ])
}

/// The gap between a detail label and its figure, so a label that fills its column still reads as
/// two fields.
const DETAIL_GAP: usize = 2;

/// How the figures were arrived at, said once so a reader never has to guess how exact they are.
fn method(report: &ContextComposition) -> String {
    match report.calibration {
        Calibration::Measured { chars_per_token } => format!(
            "Total measured from the provider; parts measured in characters and divided at \
             {chars_per_token:.1} per token, so they sum to it."
        ),
        Calibration::Estimated => {
            "Estimated: nothing has been billed this session yet, so there is no measured total to \
             divide. The first reply replaces this with the provider's own figure."
                .to_string()
        }
    }
}

/// A finding, wrapped by hand at the width prose is comfortable at and hung under its marker.
fn finding_lines(text: &str, style: Style, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (index, chunk) in wrap(text, 76).into_iter().enumerate() {
        let marker = if index == 0 { "  · " } else { "    " };
        out.push(Line::from(vec![
            Span::styled(marker.to_string(), if index == 0 { style } else { styles.muted }),
            Span::styled(chunk, if index == 0 { style } else { styles.muted }),
        ]));
    }
    out
}

fn level_style(level: FindingLevel, styles: &ThemeStyles) -> Style {
    match level {
        FindingLevel::Alert => styles.error,
        FindingLevel::Warn => styles.warning,
        FindingLevel::Note => styles.text,
    }
}

/// The hue the occupancy has earned — the dock's thresholds, so the two agree on what is tight.
fn occupancy_style(report: &ContextComposition, styles: &ThemeStyles) -> Style {
    match report.used_pct() {
        Some(pct) if pct >= FULL_PCT => styles.error,
        Some(pct) if pct >= TIGHT_PCT => styles.warning,
        _ => styles.text,
    }
}

/// Whether a detail row is an outlier rather than an entry in an inventory.
///
/// Measured against **its own part**, not against the window. A share of the window does not
/// survive a change of model: the same 12k read is a twentieth of a 200k window and a hundredth of
/// a 1.1M one, so a window-relative rule silently stops naming anything on a large-context model —
/// which is exactly where a runaway result is easiest to miss. The floor keeps a small session from
/// naming trivia just because it is all there is.
fn worth_naming(part_tokens: u64, tokens: u64) -> bool {
    tokens >= DETAIL_FLOOR_TOKENS && tokens * 100 >= part_tokens * DETAIL_SHARE_PCT
}

fn muted(text: impl Into<String>, styles: &ThemeStyles) -> Line<'static> {
    Line::from(Span::styled(format!("  {}", text.into()), styles.muted))
}

/// Shortens from the middle, keeping both ends.
///
/// A detail label is `‘tool’ path`, and both ends identify it: cutting the tail loses the file
/// name, which is the only part that says *which* call to narrow, and cutting the head loses the
/// tool. So the middle goes, with the tail favored — that is where a path gets specific.
fn elide_middle(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let budget = max - 1;
    let head = budget / 3;
    let tail = budget - head;
    let characters: Vec<char> = text.chars().collect();
    let lead = take_width(&characters, head, false);
    let trail = take_width(&characters, tail, true);
    format!("{lead}…{trail}")
}

/// `width` cells' worth of characters from one end, never splitting a wide one.
fn take_width(characters: &[char], width: usize, from_end: bool) -> String {
    let mut used = 0;
    let mut out: Vec<char> = Vec::new();
    let iter: Box<dyn Iterator<Item = &char>> =
        if from_end { Box::new(characters.iter().rev()) } else { Box::new(characters.iter()) };
    for character in iter {
        let next = unicode_width::UnicodeWidthChar::width(*character).unwrap_or(0);
        if used + next > width {
            break;
        }
        out.push(*character);
        used += next;
    }
    if from_end {
        out.reverse();
    }
    out.into_iter().collect()
}

/// Greedy word wrap. The transcript re-wraps prose for the canvas, but a finding is laid out beside
/// a marker column, so it is wrapped here where the indent is known.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests;
