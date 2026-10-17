//! Review metadata — the review's heading and the metadata rows under it, wide, medium and narrow.

use super::super::{brief, review_label};
use super::guide::TITLE_ROWS;
use super::summary::{
    chapter_progress_summary, checks_label, feedback_summary, hidden_noise_summary,
    review_activity_status, review_outcome_label, review_stats_summary, review_summary,
    stale_revision_label,
};
use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// The review title, falling back to a stable label when the source carries none.
pub(super) fn review_title(review: &luxe_core::guided_review::GuidedReview) -> &str {
    let title = review.context.title.trim();
    if title.is_empty() {
        "Guided Review"
    } else {
        review.context.title.as_str()
    }
}

/// A one-line metadata summary (source · workload · judgements · counts) for the compact
/// medium/narrow headers that have no room for the full guide pane.
pub(super) fn review_metadata_summary(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> String {
    // Ordered by what must survive clipping on a narrow tier: identity and workload, then the
    // judgements (outcome, verification, staleness), and only then the counts. The order was
    // already documented this way and already wrong — the feedback roll-up sat third, so a longer
    // one pushed the recorded outcome off the end of the row.
    let mut parts = vec![review_label(review), review_summary(app)];
    if let Some(outcome) = review_outcome_label(app.review.session()) {
        parts.push(format!("Outcome: {outcome}"));
    }
    if let Some(stale) = stale_revision_label(app, review) {
        parts.push(stale);
    }
    if let Some((_, checks)) = checks_label(review) {
        parts.push(checks);
    }
    parts
        .push(feedback_summary(app.review.session(), crate::app::github_post_is_pull_request(app)));
    if let Some(chapters) = chapter_progress_summary(app, review) {
        parts.push(chapters);
    }
    parts.push(review_stats_summary(review));
    parts.join(" · ")
}

/// The rail's own heading, cut from the same ramp as the dock's.
///
/// This rail and the Context Dock are the same column at the same screen position —
/// `LEFT_RAIL_WIDTH == CONTEXT_DOCK_WIDTH` so the divider never jumps moving chat↔review — so the
/// heading over it has to be the same object. `Hero`, because REVIEW anchors this surface the way
/// PROJECT anchors the dock. `statusline.style = "flat"` flattens it with everything else.
fn review_heading_spans(app: &App) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    if app.config.statusline.style() == luxe_core::config::StatuslineStyle::Flat {
        return vec![Span::styled(
            "REVIEW",
            styles.accent.add_modifier(ratatui::style::Modifier::BOLD),
        )];
    }
    let canvas = styles.background.bg.unwrap_or(styles.slab.canvas);
    crate::widgets::badge("REVIEW", crate::theme::styles::Depth::Hero, canvas, styles)
}

/// `REVIEW` on the left and `Step 3 of 7` on the right: where you are, in the place the peek pane
/// puts its own `2 of 5`, rather than at the foot beside a ribbon of cells, which would spend a row
/// saying a number twice.
fn review_heading_line(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    width: u16,
) -> Line<'static> {
    let mut spans = review_heading_spans(app);
    if let Some((at, total)) = app.guided_review_workspace.walkthrough_step(review) {
        let step = format!("Step {at} of {total}");
        let used: usize = spans.iter().map(|span| span.content.width()).sum();
        if used + step.width() < usize::from(width) {
            spans.push(Span::raw(" ".repeat(usize::from(width) - used - step.width())));
            spans.push(Span::styled(step, app.theme_styles.muted));
        }
    }
    Line::from(spans)
}

/// The guide pane's head: what is being read, and nothing about what it asks for.
///
/// Identity and scale only. The counts a reviewer glances down for — workload, chapters walked,
/// comments outstanding — are in the foot, so the two questions have one place each instead of a
/// column of numbers at the top that has to be re-read to find the one that changed.
pub(super) fn review_metadata_lines(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::default(), // 1-cell top pad
        review_heading_line(app, review, width),
        // Two lines and then an ellipsis. A review's title is usually the session's first message,
        // which is a paragraph as often as it is a name — left to wrap it took four lines off the
        // top of the guide and still ended mid-word, which reads as a rendering fault rather than
        // as a long title.
        Line::from(Span::styled(
            clip_terminal_cells_ellipsis(
                review_title(review),
                usize::from(width).saturating_mul(TITLE_ROWS),
            ),
            app.theme_styles.text,
        )),
        Line::from(Span::styled(
            format!("{} · {}", review_label(review), review_stats_summary(review)),
            app.theme_styles.muted,
        )),
    ];
    if let Some(pr) = review.context.pull_request.as_ref() {
        lines.extend(brief::pull_request_facts(app, pr, usize::from(width)));
    }
    lines.extend(guide_state_lines(app));
    if let Some(status) = review_activity_status(app) {
        // Spun, with the live tool beside it: after a submission a silent review gives no sign the
        // agent is doing anything, which is when a reviewer starts pressing keys.
        let mut spans = vec![
            Span::styled(
                format!("{} ", crate::statusline::spinner(app.guided_review_workspace.frame)),
                app.theme_styles.accent,
            ),
            Span::styled(status, app.theme_styles.accent),
        ];
        if let Some(tool) = app.status.active_tool.as_deref() {
            spans.push(Span::styled(format!("  {tool}"), app.theme_styles.muted));
        }
        // Through the status model rather than off `turn_started`, so a turn blocked on a prompt
        // reports the same figure here as it does above the composer.
        if let Some(elapsed) = app.status.elapsed(app.frame_now()) {
            spans.push(Span::styled(
                format!("  {}", crate::statusline::format_elapsed_compact(elapsed)),
                app.theme_styles.dim,
            ));
        }
        lines.push(Line::from(spans));
    }
    if let Some(stale) = stale_revision_label(app, review) {
        lines.push(Line::from(Span::styled(stale, app.theme_styles.warning)));
    }
    // What the review will not say unless it is told to. Silent whenever there is nothing wrong,
    // which is most of the time.
    lines.extend(brief::notice_lines(app, review));
    lines.push(Line::default());
    lines
}

/// How the visible chapters were produced, when that is worth saying. A review opens on
/// deterministic sections and upgrades in place, so silence here means the model wrote them.
///
/// There is no waiting case. The guide pass takes tens of seconds on a local model and every
/// heading, sentence and per-file note on this surface arrives with it, so opening on a structural
/// placeholder would rewrite itself under the reader. The surface opens only once the guide has
/// landed, and the waiting is announced in the tray above the composer, which is where the reader
/// is while it happens and which does not need a whole screen to say one line.
fn guide_state_lines(app: &App) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let Some(provenance) = app.review.provenance.as_ref() else {
        return Vec::new();
    };
    provenance
        .label()
        .map(|label| vec![Line::from(Span::styled(label, styles.muted))])
        .unwrap_or_default()
}

/// The medium tier's stacked context block, shared with the layout so its reserved height matches
/// what it renders.
pub(in crate::view::guided_review) fn medium_context_lines(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Vec<Line<'static>> {
    let chapter_index = app
        .guided_review_workspace
        .position
        .chapter_id
        .as_ref()
        .and_then(|id| review.chapters.iter().position(|chapter| &chapter.id == id))
        .unwrap_or_default();
    let chapter = review.chapters.get(chapter_index);
    let file = app
        .guided_review_workspace
        .position
        .file_id
        .as_ref()
        .and_then(|id| review.files.iter().find(|file| &file.id == id));
    let mut lines = vec![
        Line::default(), // top pad
        Line::from(Span::styled(
            review_title(review).to_string(),
            app.theme_styles.accent.add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(Span::styled(review_metadata_summary(app, review), app.theme_styles.muted)),
        Line::from(Span::styled(
            format!("{:02} / {:02}", chapter_index + 1, review.chapters.len().max(1)),
            app.theme_styles.muted,
        )),
    ];
    if let Some(chapter) = chapter {
        lines.push(Line::from(Span::styled(chapter.title.clone(), app.theme_styles.text)));
        lines.push(Line::from(Span::styled(chapter.rationale.clone(), app.theme_styles.muted)));
    }
    if let Some(file) = file {
        let path = file
            .new_path
            .as_deref()
            .or(file.old_path.as_deref())
            .map_or_else(|| "(unknown path)".into(), |path| path.display().to_string());
        lines.push(Line::from(Span::styled(
            format!(
                "› {path}  +{} −{}",
                file.additions.unwrap_or_default(),
                file.deletions.unwrap_or_default()
            ),
            app.theme_styles.muted,
        )));
    }
    if let Some(summary) = hidden_noise_summary(app, review) {
        let detail = if app.guided_review_workspace.noise_details_visible {
            let (hunks, lines) = app.guided_review_workspace.hidden_counts(&review.files);
            format!(
                "{hunks}/{lines} folded: format/generated/lockfile/mechanical; semantic/comments kept · d close"
            )
        } else {
            summary
        };
        lines.push(Line::from(Span::styled(detail, app.theme_styles.muted)));
    }
    lines
}
