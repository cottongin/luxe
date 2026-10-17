//! Reveal — draws a prefix of an item's rendered rows, its newest characters fading in.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// Rendered characters an item's laid-out lines hold, which is what the reveal paces against.
pub(in crate::view) fn rendered_chars(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.chars().count()).sum::<usize>())
        .sum()
}

/// Draws the first `shown` characters of an item, with the newest of them still fading in.
///
/// Truncating *rendered* lines rather than the source text is the whole trick: markdown and wrapping
/// ran over the complete text, so line breaks are already final, nothing re-flows as the cursor
/// advances, and a half-typed `**` is hidden rather than shown as asterisks. `band` is what the text
/// sits on, so the fade can land on it exactly and the newest character starts invisible.
///
/// `settle` is how resolved the trail is — 1 when the stream has stalled long enough that the tail
/// must be legible, which costs nothing here beyond skipping the gradient entirely.
pub(in crate::view) fn revealed_lines(
    lines: Vec<Line<'static>>,
    shown: usize,
    styles: &crate::theme::styles::ThemeStyles,
    band: Style,
    settle: f32,
) -> Vec<Line<'static>> {
    let fade = if settle >= 1.0 { 0 } else { crate::theme::styles::STREAM_TRAIL_LEN };
    let mut out = Vec::with_capacity(lines.len());
    let mut seen = 0usize;
    for mut line in lines {
        let width = rendered_chars(std::slice::from_ref(&line));
        if seen >= shown {
            break;
        }
        // Settled: far enough behind the cursor that no character on it is still fading.
        if seen + width + fade <= shown {
            seen += width;
            out.push(line);
            continue;
        }
        let mut at = seen;
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
        for span in std::mem::take(&mut line.spans) {
            let len = span.content.chars().count();
            if at >= shown {
                break;
            }
            if at + len + fade <= shown {
                at += len;
                spans.push(span);
                continue;
            }
            // A fenced block brings its own background; prose and reasoning sit on the band.
            let base = match span.style.bg.or(line.style.bg) {
                Some(bg) => Style::default().bg(bg),
                None => band,
            };
            let ramp = styles.stream_shades(span.style, base, settle);
            spans.extend(faded_runs(&span, at, shown, &ramp));
            at += len;
        }
        line.spans = spans;
        out.push(line);
        seen += width;
    }
    out
}

/// Splits the visible part of `span` into runs of equal shade, newest character faintest.
///
/// One span per shade rather than one per character: a run of characters at the same point on the
/// ramp is one span, so the gradient costs a handful of spans on one line rather than sixteen.
fn faded_runs(
    span: &Span<'static>,
    start: usize,
    shown: usize,
    ramp: &[Style],
) -> Vec<Span<'static>> {
    let mut runs: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut current: Option<usize> = None;
    for (offset, character) in span.content.chars().enumerate() {
        let at = start + offset;
        if at >= shown {
            break;
        }
        // Distance behind the cursor: 0 is the character that just arrived, and it lands on the far
        // end of the ramp, which is the background itself.
        let behind = shown - at - 1;
        let shade = ramp.len().saturating_sub(1) - behind.min(ramp.len().saturating_sub(1));
        if current.is_some_and(|previous| previous != shade) {
            let style = ramp.get(current.unwrap_or(0)).copied().unwrap_or(span.style);
            runs.push(Span::styled(std::mem::take(&mut text), style));
        }
        current = Some(shade);
        text.push(character);
    }
    if let Some(shade) = current {
        let style = ramp.get(shade).copied().unwrap_or(span.style);
        runs.push(Span::styled(text, style));
    }
    runs
}
