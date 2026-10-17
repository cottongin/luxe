//! Parts — the rows, marks and prose both tabs are built from.

use luxe_core::github::overview::{CheckAction, CheckMark};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::glyphs::Mark;
use crate::theme::styles::ThemeStyles;

/// Cells the label column of a fact row takes: `Pull requests` and a gap.
pub(super) const LABEL_CELLS: usize = 15;

/// Cap on untrusted text drawn here: more than any real comment, less than a pasted log.
pub(super) const BODY_MAX_CHARS: usize = 20_000;

/// Spans cut to `width` cells, the cut marked.
pub(super) fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut kept = Vec::new();
    let mut used = 0;
    for span in spans {
        let cells = span.content.width();
        if used + cells <= width {
            used += cells;
            kept.push(span);
            continue;
        }
        let room = width.saturating_sub(used);
        if room > 0 {
            kept.push(Span::styled(crate::widgets::truncate_end(&span.content, room), span.style));
        }
        break;
    }
    Line::from(kept)
}

/// A row cut into the selection slab when under the cursor: `left`, then `right` against the
/// far edge, both on one line.
pub(super) fn row(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
    selected: bool,
    styles: &ThemeStyles,
) -> Line<'static> {
    let room = width.saturating_sub(crate::widgets::SELECTION_FRAME);
    let right_cells: usize = right.iter().map(|span| span.content.width()).sum();
    let left = fit(left, room.saturating_sub(right_cells + 1));
    let left_cells = left.width();
    let mut content = left.spans;
    if !right.is_empty() {
        content.push(Span::raw(" ".repeat(room.saturating_sub(left_cells + right_cells))));
        content.extend(right);
    }
    if selected {
        lift_dim(&mut content, styles);
    }
    crate::widgets::selection_row(content, width, selected, styles)
}

/// Steps `dim` spans up to `muted`, for a row about to sit on the selection surface.
///
/// Most themes paint `selectedBg` the color of `dim`, so a dim separator on a chosen row is
/// invisible — the escape [`ThemeStyles::secondary_on`] makes, applied to spans already built.
pub(crate) fn lift_dim(spans: &mut [Span<'static>], styles: &ThemeStyles) {
    for span in spans.iter_mut().filter(|span| span.style.fg == styles.dim.fg) {
        span.style = span.style.patch(styles.muted);
    }
}

/// A heading in the accent, with a quiet note against the far edge.
pub(super) fn heading(
    text: &str,
    note: Option<String>,
    width: usize,
    styles: &ThemeStyles,
) -> Line<'static> {
    let right = note.map(|note| vec![Span::styled(note, styles.muted)]).unwrap_or_default();
    row(vec![Span::styled(text.to_owned(), styles.md_heading)], right, width, false, styles)
}

/// A mark in its color, with the space after it.
pub(super) fn mark(check: CheckMark, styles: &ThemeStyles) -> Span<'static> {
    let (glyph, style) = match check {
        CheckMark::Pass => (Mark::Yes.glyph(), styles.success),
        CheckMark::Fail => (Mark::No.glyph(), styles.error),
        CheckMark::Wait => (Mark::Waiting.glyph(), styles.warning),
        CheckMark::Note => ("\u{2013}", styles.muted),
    };
    Span::styled(format!("{glyph} "), style)
}

/// The key a status row answers to, and what the hint calls it — GitHub's own words where it has
/// them (`update branch`, `resolve conflicts`, `mark ready`).
///
/// Merging has no letter: it is the one thing here that cannot be taken back, so it is Enter on its
/// row and a confirmation after that.
pub fn action_hint(action: CheckAction) -> (String, String) {
    let (key, verb) = match action {
        CheckAction::FixChecks => ("f", "fix with agent".to_string()),
        CheckAction::ResolveThreads => ("t", "resolve threads".to_string()),
        CheckAction::UpdateBranch => ("u", "update branch".to_string()),
        CheckAction::ResolveConflicts => ("x", "resolve conflicts".to_string()),
        CheckAction::MarkReady => ("R", "mark ready".to_string()),
        CheckAction::OpenBelow(number) => ("enter", format!("open #{number}")),
        CheckAction::Merge => ("enter", "merge…".to_string()),
        CheckAction::EnableAutoMerge => ("enter", "enable auto-merge…".to_string()),
        CheckAction::DisableAutoMerge => ("enter", "disable auto-merge".to_string()),
    };
    (key.to_string(), verb)
}

pub(super) fn key_spans(key: &str, verb: &str, styles: &ThemeStyles) -> Vec<Span<'static>> {
    vec![Span::styled(key.to_owned(), styles.accent), Span::styled(format!(" {verb}"), styles.text)]
}

pub(super) fn label_cell(label: &str, styles: &ThemeStyles) -> Span<'static> {
    let pad = LABEL_CELLS.saturating_sub(label.width());
    Span::styled(format!("{label}{}", " ".repeat(pad)), styles.text)
}

/// Untrusted markdown, sanitized, rendered and wrapped to sit at the rows' text column.
pub(super) fn prose(body: &str, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let inset = crate::widgets::SELECTION_INSET;
    markdown(body, width.saturating_sub(inset * 2), styles).into_iter().map(indented).collect()
}

/// Untrusted markdown, sanitized, rendered and wrapped to `width`, with no margin of its own.
///
/// NOTE: laid out on every frame; the real surface caches it per entry and width, the way the
/// transcript caches its rows.
pub(super) fn markdown(body: &str, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let clean = super::html::readable(&luxe_core::github::untrusted(body, BODY_MAX_CHARS));
    if clean.trim().is_empty() {
        return Vec::new();
    }
    let rendered = crate::markdown::render_markdown(&clean, styles);
    crate::widgets::wrap_lines(&rendered, width)
}

pub(super) fn indented(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(crate::widgets::SELECTION_INSET))];
    spans.extend(line.spans);
    Line::from(spans)
}
