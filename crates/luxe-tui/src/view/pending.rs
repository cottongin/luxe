//! Pending tray — mid-turn requests parked above the composer.
//!
//! A steer, a queued follow-up or a review asked for mid-turn is not part of the conversation yet,
//! so it does not go in the
//! transcript: dropped there it would land wherever the stream happened to be, then scroll away
//! under the turn's own output while the user is still waiting on it. It sits here instead,
//! pinned to the composer and deliberately quiet, until one of three things happens to it: the turn
//! takes it in (a steer's row retires as the engine commits it — see
//! [`crate::app::App::retire_steering`]), [`crate::app::App::promote_next_queued`] promotes a
//! follow-up into a turn of its own, or the reader takes it back to edit
//! ([`crate::app::App::restore_pending`], which the strip's own hint row advertises).

use crate::app::{App, MidTurnDelivery, PendingSubmission};
use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Held submissions listed before the rest collapse into a count.
const MAX_VISIBLE: usize = 4;

/// Rows the tray needs, or 0 when nothing is held.
///
/// One row per submission plus a leading blank, so the strip separates from the activity row
/// above it instead of reading as one dense block above the prompt.
pub(super) fn height(app: &App) -> u16 {
    let held = app.pending_submissions.len();
    let review = usize::from(app.review.queued.is_some());
    if held == 0 && review == 0 {
        return 0;
    }
    let listed = held.min(MAX_VISIBLE);
    let overflow = usize::from(held > MAX_VISIBLE);
    let hint = usize::from(recall_hint(app).is_some());
    (listed + overflow + hint + review + 1) as u16
}

/// `Alt+Up to edit it` — the one thing a reader cannot discover from the tray itself.
///
/// Reads the chord out of the key map rather than naming a default, so a rebound (or unbound) key
/// never leaves the strip advertising something that does nothing. Suppressed by
/// `show_key_hints: false`, which is what that setting is for.
fn recall_hint(app: &App) -> Option<String> {
    if !app.config.show_key_hints.unwrap_or(true) {
        return None;
    }
    // Nothing to recall, nothing to say. The strip also carries a queued review, which is not a
    // message and cannot be edited — so without this the offer to edit "them" appears over a row
    // that is only reporting that a review finished capturing.
    if app.pending_submissions.is_empty() {
        return None;
    }
    let chord = app
        .keymap
        .chords_for(luxe_core::keymap::UiAction::DequeueMessages)
        .and_then(|chords| chords.first())?;
    let subject = if app.pending_submissions.len() == 1 { "it" } else { "them" };
    Some(format!("   {chord} to edit {subject} here"))
}

/// Draws the held submissions, newest last, in the strip directly above the composer.
pub(super) fn render(f: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 || (app.pending_submissions.is_empty() && app.review.queued.is_none()) {
        return;
    }
    let styles = &app.theme_styles;
    let width = usize::from(area.width);
    let mut lines = vec![Line::default()];
    // Queue positions are the order they will run in, counted as we go: derived from the list
    // rather than stored, so promoting one can't leave a stale number behind.
    let mut position = 0usize;
    for held in app.pending_submissions.iter() {
        if held.delivery == MidTurnDelivery::Queue {
            position += 1;
        }
        if lines.len() > MAX_VISIBLE {
            continue;
        }
        lines.push(submission_line(held, position, width, styles));
    }
    let hidden = app.pending_submissions.len().saturating_sub(MAX_VISIBLE);
    if hidden > 0 {
        lines.push(Line::from(Span::styled(format!("   +{hidden} more waiting"), styles.muted)));
    }
    if let Some(hint) = recall_hint(app) {
        lines.push(Line::from(Span::styled(hint, styles.dim)));
    }
    if let Some(queued) = app.review.queued.as_ref() {
        lines.push(queued_review_line(app, queued, styles));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The queued review's row: what it is waiting for, then what to press when it stops waiting.
///
/// Same shape as a steer's row because it is the same kind of promise — something asked for that
/// has not happened yet — and the same reason for living here rather than in the transcript: it
/// would scroll away under the output of the very turn it is waiting on.
fn queued_review_line(
    app: &App,
    queued: &crate::app::QueuedReview,
    styles: &ThemeStyles,
) -> Line<'static> {
    use crate::app::QueuedReviewState;
    let (glyph, label, detail, detail_style) = match queued.state {
        QueuedReviewState::Waiting => (
            Icon::Queued.glyph().to_string(),
            "review queued",
            "starts when this turn ends".to_owned(),
            styles.muted,
        ),
        // A spinner, not the queued glyph: work is happening in both of these, and a pause-shaped
        // mark over it says the opposite of what is true. Each says what the work is, because
        // "capturing" for a minute while a model writes reads as a hang
        QueuedReviewState::Capturing => (
            crate::statusline::spinner(app.status.spinner_frame).to_string(),
            "review",
            capture_label(&queued.target),
            styles.muted,
        ),
        // The long part, so it names who is doing it and counts: a number that keeps moving is
        // what tells a reader the wait is progress rather than a stall
        QueuedReviewState::Writing { since } => (
            crate::statusline::spinner(app.status.spinner_frame).to_string(),
            "review",
            format!(
                "{} is writing the guide \u{b7} {}",
                match app.status.model.trim() {
                    "" => "the model",
                    model => model,
                },
                crate::statusline::format_elapsed_compact(
                    app.frame_now().saturating_duration_since(since)
                )
            ),
            styles.muted,
        ),
        QueuedReviewState::Ready => (
            Icon::Findings.glyph().to_string(),
            "review ready",
            match app
                .keymap
                .chords_for(luxe_core::keymap::UiAction::ReviewOpen)
                .and_then(|chords| chords.first())
            {
                // The chord alone. The row already says the review is ready, and the only thing
                // anyone does with a ready review is open it.
                Some(chord) => chord.to_string(),
                None => "open it from the palette".to_owned(),
            },
            styles.success,
        ),
    };
    let marker_style =
        if queued.state == QueuedReviewState::Ready { styles.success } else { styles.warning };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(crate::glyphs::icon_prefix(&glyph), marker_style),
        Span::styled(label, styles.muted),
        Span::styled(" \u{b7} ".to_string(), styles.muted),
        Span::styled(detail, detail_style),
    ])
}

/// What a capture is doing, named by its source.
fn capture_label(target: &luxe_core::guided_review::ReviewTarget) -> String {
    use luxe_core::guided_review::ReviewTarget;
    match target {
        ReviewTarget::WorkingTree => "capturing the working tree".into(),
        ReviewTarget::LastTurn { .. } => "capturing the last turn".into(),
        ReviewTarget::Base { reference } => format!("capturing the change since {reference}"),
        ReviewTarget::Commit { oid } => {
            format!("capturing commit {}", oid.chars().take(12).collect::<String>())
        }
        ReviewTarget::PullRequest { number, .. } => format!("fetching pull request #{number}"),
        ReviewTarget::OwnPullRequest { number } => {
            format!("reading pull request #{number} and its threads")
        }
    }
}

/// `→ steering · <message>` on one row: the state first so the column scans, the message after it
/// so the row is still recognizable as the thing the user typed.
fn submission_line(
    held: &PendingSubmission,
    position: usize,
    width: usize,
    styles: &ThemeStyles,
) -> Line<'static> {
    let (glyph, label) = match held.delivery {
        MidTurnDelivery::Steer => (Icon::Steering.glyph(), "steering".to_string()),
        MidTurnDelivery::Queue => (Icon::Queued.glyph(), format!("queued #{position}")),
    };
    // Muted throughout, including the marker: nothing here has happened yet, and the transcript's
    // accent-on-band user messages are the ones that have.
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(crate::glyphs::icon_prefix(glyph), styles.warning),
        Span::styled(label.clone(), styles.muted),
    ];
    let used = 1 + crate::glyphs::icon_cells(glyph) + crate::selection::cell_width(&label);
    let room = width.saturating_sub(used + 3);
    let preview = preview_text(&held.display, room);
    if !preview.is_empty() {
        spans.push(Span::styled(" \u{b7} ".to_string(), styles.muted));
        spans.push(Span::styled(preview, styles.muted));
    }
    Line::from(spans)
}

/// The message flattened to one row: newlines become spaces, and an over-long draft is cut with
/// an ellipsis so the tray height never depends on what was typed.
fn preview_text(display: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
    let flattened = display.split_whitespace().collect::<Vec<_>>().join(" ");
    if crate::selection::cell_width(&flattened) <= room {
        return flattened;
    }
    let mut out = String::new();
    let mut used = 0usize;
    for character in flattened.chars() {
        let cell = crate::selection::cell_width(&character.to_string());
        if used + cell > room.saturating_sub(1) {
            break;
        }
        out.push(character);
        used += cell;
    }
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests;
