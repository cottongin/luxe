//! Brief notices — where the work came from and what needs the reader's attention.
//!
//! Facts about the review that are worth a row only when they are *bad*. Inside a labeled section
//! that ran whether or not it had anything to say, the one row that mattered would be buried in
//! filler. Here they are silent by default, and each one is a sentence a reviewer would want
//! interrupting them.

use super::{coverage, LARGE_REVIEW_JUDGEMENT_ITEMS};
use crate::app::App;
use luxe_core::guided_review::{AttentionSignal, GuidedReview};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// What this review will not tell you unless it is told to, in the order it matters.
///
/// Bounded by construction rather than by a cap: every entry is conditional and rare, and the
/// aggregate harness signals — the only unbounded source — contribute at most one row a kind.
pub(in crate::view::guided_review) fn notice_lines(
    app: &App,
    review: &GuidedReview,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(warning) = unverified_note(review) {
        lines.push(Line::from(Span::styled(warning, styles.warning)));
    }
    // A review this size is not one sitting. Name the smaller unit rather than letting the
    // reviewer discover the problem forty hunks in.
    if app.review.budget.needs_review > LARGE_REVIEW_JUDGEMENT_ITEMS {
        lines.push(Line::from(Span::styled(
            "More than one sitting · s picks a smaller source",
            styles.warning,
        )));
    }
    // Say why the budget could not help, rather than presenting a wall of work as if that were the
    // reviewer's problem. A pull request was written somewhere else, which its reader knows.
    if review.context.pull_request.is_none()
        && app.review.budget.needs_review > 0
        && no_provenance(review)
    {
        lines
            .push(Line::from(Span::styled("No record of the work behind this source", styles.dim)));
    }
    lines.extend(coverage::coverage_lines(review, styles));
    // A kind past its cap gets its total, one row each. Every flagged file wears its own badge in
    // the contents, so the call-outs' "2 more" would read as two hidden somewhere.
    for signal in app.review.attention.iter().filter(|signal| signal.aggregate) {
        let total = flagged_total(app, signal.kind);
        let said = format!("{total} {}", signal.kind.plural());
        lines.push(Line::from(Span::styled(said, attention_style(app, signal))));
    }
    // What this session did elsewhere is a fact about the session, and a pull request is not it
    if let Some(outside) = work_done_outside(app).filter(|_| review.context.pull_request.is_none())
    {
        // `info`, not a warning: this is a fact about what the review's source is, not a suspicion
        // about the change inside it.
        lines.push(Line::from(Span::styled(outside, styles.info)));
    }
    lines
}

/// What the harness knows about whether anything was run, said only when the answer is unwelcome.
///
/// Silent when a command ran after the last edit, because "the tests passed" is the expected state
/// and a row that always renders stops being read. Silent too when there is nothing to judge it
/// against — a `base` or `commit` review has no turn behind it, so it is not *unverified*, it is
/// simply not that kind of source.
fn unverified_note(review: &GuidedReview) -> Option<&'static str> {
    let provenance = review.provenance.as_ref()?;
    if provenance.written_paths.is_empty() {
        return None;
    }
    if provenance.commands.is_empty() {
        Some("No command was run in this work")
    } else if !provenance.verified_after_last_edit {
        Some("Nothing ran after the last edit")
    } else {
        None
    }
}

/// What this session changed outside the project, or `None` when it stayed inside.
///
/// A review is captured from the project's worktree, so those files are not in it and no amount of
/// reading the diff will reveal them. Saying so is the difference between a review that is
/// *scoped* and one that quietly claims to be the whole session.
fn work_done_outside(app: &App) -> Option<String> {
    let external = &app.workspace.as_ref()?.external;
    if external.is_empty() {
        return None;
    }
    let files = external.total_files;
    Some(format!(
        "{files} {} changed elsewhere this session",
        if files == 1 { "file" } else { "files" }
    ))
}

/// Whether the review has no record of the work that produced it (a base/commit review, or a
/// working tree luxe did not write).
fn no_provenance(review: &GuidedReview) -> bool {
    review.provenance.as_ref().is_none_or(|provenance| {
        provenance.commands.is_empty() && provenance.written_paths.is_empty()
    })
}

/// Color by urgency: a possible credential or a weakened gate outranks an unread edit.
fn attention_style(app: &App, signal: &AttentionSignal) -> Style {
    use luxe_core::guided_review::AttentionKind;
    match signal.kind {
        AttentionKind::SecretShaped | AttentionKind::GateWeakened => app.theme_styles.error,
        AttentionKind::BlindEdit => app.theme_styles.warning,
    }
}

/// How many lines or files of `kind` the review flags: a credential or a weakened gate is a line,
/// an unread edit a file.
fn flagged_total(app: &App, kind: luxe_core::guided_review::AttentionKind) -> usize {
    app.review.flags.iter().filter(|flag| flag.kind == kind).count()
}
