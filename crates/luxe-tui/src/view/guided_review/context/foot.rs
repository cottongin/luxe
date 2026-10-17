//! The guide's foot — the feedback written so far, and what to do next.

use super::super::overlays::finding_state_glyph;
use super::super::{chord, feedback_window};
use super::summary::{
    checks_label, collapsed_banner, feedback_tally, folded_reasons, review_outcome_label,
    review_summary, ChecksTone,
};
use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Most comments the foot lists before pointing at the feedback window for the rest.
const FOOT_COMMENTS: usize = 4;

/// The guide's foot: the feedback, and what the review wants from you next.
///
/// Pinned to the bottom of the pane so "what have I said, and what is waiting on me" is always in
/// the same place. With no feedback yet there is no heading for it, and a foot with nothing to say
/// is not drawn at all: a `FEEDBACK none yet` row told the reader only that the rail had a slot. The comments are listed rather than counted, the way the post preview lists
/// them, so the foot is a way back to each one rather than a number to remember. Where you are is
/// the head's job now (`Step 3 of 7`), and the folded churn says what it is on its own row in the
/// diff, so neither costs the foot a line.
pub(super) fn guide_foot_lines(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    width: u16,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let pull_request = crate::app::github_post_is_pull_request(app);
    let mut lines = vec![Line::default()];
    lines.extend(feedback_foot(app, usize::from(width), pull_request));
    if walkthrough_is_empty(app, review) {
        // Nothing to walk is worth saying; it is the one thing a missing step counter cannot
        lines.push(Line::from(Span::styled(review_summary(app), styles.success)));
    }
    // Posted is the answer to "what comes next", so it takes the prompt's place: a toast saying
    // it went is gone in seconds, and a foot still offering to post read as if it had not
    let posted = pull_request.then(|| posted_verdict(app.review.session())).flatten();
    if let Some(verdict) = posted {
        lines.extend(posted_next_action(app, verdict));
    }
    // Read to the end is when the reader wants to know what comes next, and nothing else says it
    let (walked, total) = app.guided_review_workspace.chapters_walked(review);
    if total > 0 && walked == total && posted.is_none() {
        lines.push(walkthrough_next_action(app, pull_request));
    }
    if let Some(outcome) = review_outcome_label(app.review.session()).filter(|_| posted.is_none()) {
        lines.push(Line::from(Span::styled(format!("Outcome: {outcome}"), styles.muted)));
    }
    if let Some((tone, label)) = checks_label(review) {
        let style = match tone {
            ChecksTone::Passed => styles.success,
            ChecksTone::Failed => styles.error,
            ChecksTone::Unsettled => styles.warning,
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    lines.extend(folded_reason_rows(app, review));
    if lines.len() == 1 {
        lines.clear();
    }
    lines
}

/// The feedback heading and the first few comments, with a pointer to the window for the rest;
/// nothing until there is feedback.
fn feedback_foot(app: &App, width: usize, pull_request: bool) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines = Vec::new();
    if let Some(session) = app.review.session().filter(|s| !s.findings.is_empty()) {
        lines.push(feedback_heading(app, pull_request));
        for finding in session.findings.iter().take(FOOT_COMMENTS) {
            lines.push(feedback_row(app, finding, width));
        }
        let more = session.findings.len().saturating_sub(FOOT_COMMENTS);
        if more > 0 {
            let window = chord(app, luxe_core::keymap::UiAction::ReviewFindings, "alt+c");
            lines.push(Line::from(vec![
                Span::styled(format!(" +{more} more \u{b7} "), styles.dim),
                Span::styled(window, styles.accent),
            ]));
        }
    }
    lines
}

/// That the review went to GitHub and with what, then the key that posts what was written since,
/// or the one that opens it.
fn posted_next_action(app: &App, verdict: &str) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let posted =
        Line::from(Span::styled(format!("Posted to GitHub \u{b7} {verdict}"), styles.success));
    let unposted = unsent_comment_count(app);
    let line = if unposted > 0 {
        let finish = chord(app, luxe_core::keymap::UiAction::ReviewFinish, "alt+enter");
        let what = match unposted {
            1 => " to post your new comment".to_owned(),
            count => format!(" to post your {count} new comments"),
        };
        vec![Span::styled(finish, styles.accent), Span::styled(what, styles.text)]
    } else {
        let open = chord(app, luxe_core::keymap::UiAction::ReviewOpenInBrowser, "o");
        vec![Span::styled(open, styles.accent), Span::styled(" opens it", styles.text)]
    };
    vec![posted, Line::from(line)]
}

/// `All read`, and the key that finishes, named for what the finish dialog will offer.
fn walkthrough_next_action(app: &App, pull_request: bool) -> Line<'static> {
    let styles = &app.theme_styles;
    let finish = app
        .keymap
        .chords_for(luxe_core::keymap::UiAction::ReviewFinish)
        .and_then(|chords| chords.first())
        .map_or_else(|| "alt+enter".to_owned(), ToString::to_string);
    // Named for what the dialog will offer, which is the one thing the key cannot say itself
    let then = if pull_request {
        " to post your review"
    } else if crate::app::pull_request_opening_offered(app) {
        " to open a pull request"
    } else if crate::app::is_own_pull_request(app) {
        " to answer on GitHub"
    } else {
        " to finish"
    };
    Line::from(vec![
        Span::styled("All read · ", styles.success),
        Span::styled(finish, styles.accent),
        Span::styled(then, styles.text),
    ])
}

/// Why each folded hunk was folded, counted by class, once the reader asks with `d`.
fn folded_reason_rows(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines = Vec::new();
    // Asked for with `d`, never volunteered: each folded row already names what it is
    let (hidden_hunks, hidden_lines) = app.guided_review_workspace.hidden_counts(&review.files);
    if hidden_hunks > 0 && app.guided_review_workspace.noise_details_visible {
        lines.push(Line::from(Span::styled(
            collapsed_banner(hidden_hunks, hidden_lines),
            styles.dim,
        )));
        // Per-class counts, not a generic blurb: "why was *this* folded" is the question
        for (label, count) in folded_reasons(app, review) {
            lines.push(Line::from(Span::styled(format!("  {count} {label}"), styles.dim)));
        }
        lines.push(Line::from(Span::styled("Semantic and comment-only changes stay", styles.dim)));
        lines.push(Line::from(Span::styled("d close", styles.dim)));
    }
    lines
}

/// The verdict a pull request's review went to GitHub with, as GitHub's page says it, once it has.
///
/// A pull request's outcome is recorded by the post landing and by nothing else, so having one is
/// what having posted means.
fn posted_verdict(
    session: Option<&luxe_core::guided_review::ReviewSession>,
) -> Option<&'static str> {
    use luxe_core::guided_review::ReviewOutcome;
    Some(match session?.outcome? {
        ReviewOutcome::Accepted => "approved",
        ReviewOutcome::ChangesRequested => "changes requested",
        ReviewOutcome::Commented => "commented",
    })
}

/// Whether there is no walkthrough to step through, which the foot says in so many words.
fn walkthrough_is_empty(app: &App, review: &luxe_core::guided_review::GuidedReview) -> bool {
    app.guided_review_workspace.walkthrough_step(review).is_none()
}

/// `FEEDBACK  3 · 2 unposted`, a section heading like the dock's, with the tally riding beside it.
fn feedback_heading(app: &App, pull_request: bool) -> Line<'static> {
    let styles = &app.theme_styles;
    let mut spans = if app.config.statusline.style() == luxe_core::config::StatuslineStyle::Flat {
        vec![Span::styled("FEEDBACK", styles.accent.add_modifier(ratatui::style::Modifier::BOLD))]
    } else {
        let canvas = styles.tool_bg.bg.unwrap_or(styles.slab.canvas);
        crate::widgets::badge("FEEDBACK", crate::theme::styles::Depth::Raised, canvas, styles)
    };
    let (tally, waiting) = match feedback_tally(app.review.session(), pull_request) {
        Some(tally) => (tally, unsent_comment_count(app) > 0),
        None => (String::new(), false),
    };
    spans.push(Span::styled(
        format!(" {tally}"),
        if waiting { styles.warning } else { styles.muted },
    ));
    Line::from(spans)
}

/// One comment in the foot: its state, where it is, and how it starts.
///
/// No `R1` here. The label is how the agent names a comment back to you, and a list whose every row
/// is its own place needs no second name.
fn feedback_row(
    app: &App,
    finding: &luxe_core::guided_review::ReviewFinding,
    width: usize,
) -> Line<'static> {
    use luxe_core::guided_review::FindingState;
    let styles = &app.theme_styles;
    let glyph_style = match finding.state {
        _ if finding.posted.is_some() => styles.dim,
        FindingState::Draft => styles.warning,
        FindingState::Open | FindingState::NeedsVerification => styles.info,
        FindingState::Resolved => styles.success,
        FindingState::Stale => styles.dim,
    };
    let glyph = format!(" {} ", finding_state_glyph(finding));
    let place = format!("{}  ", feedback_place(app, &finding.anchor));
    // The title the feedback window lists it by, or its first line when it was never given one
    let first = finding.body.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let said = if finding.title.trim().is_empty() { first } else { finding.title.trim() };
    let said = feedback_window::plain(said);
    let budget = width.saturating_sub(glyph.width() + place.width());
    Line::from(vec![
        Span::styled(glyph, glyph_style),
        Span::styled(place, styles.muted),
        Span::styled(clip_terminal_cells_ellipsis(&said, budget), styles.text),
    ])
}

/// Where a comment sits, short enough for the rail: `notification.rs:13`, `on the review`.
fn feedback_place(app: &App, anchor: &luxe_core::guided_review::FindingAnchor) -> String {
    use luxe_core::guided_review::FindingAnchor;
    let name = |file_id: &luxe_core::guided_review::FileId| {
        app.guided_review
            .as_ref()
            .and_then(|review| review.files.iter().find(|file| &file.id == file_id))
            .and_then(|file| file.new_path.as_deref().or(file.old_path.as_deref()))
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    };
    match anchor {
        FindingAnchor::Line { file_id, line, .. } => {
            name(file_id).map_or_else(|| format!("line {line}"), |name| format!("{name}:{line}"))
        }
        FindingAnchor::Hunk { file_id, .. } => name(file_id).unwrap_or_else(|| "a hunk".into()),
        FindingAnchor::Chapter { .. } => "on the chapter".into(),
        FindingAnchor::Review => "on the review".into(),
    }
}

/// Comments written but not yet sent, which is the only finding count with a key attached to it.
///
/// A comment already on GitHub is sent, though it stays a draft locally, and a question on a pull
/// request stays with the agent: counting either kept the heading's warning lit over a review that
/// had gone out whole.
fn unsent_comment_count(app: &App) -> usize {
    let pull_request = crate::app::github_post_is_pull_request(app);
    app.review.session().map_or(0, |session| {
        session
            .findings
            .iter()
            .filter(|finding| {
                finding.state == luxe_core::guided_review::FindingState::Draft
                    && finding.posted.is_none()
                    && !(pull_request
                        && finding.severity == luxe_core::guided_review::FindingSeverity::Question)
            })
            .count()
    })
}
