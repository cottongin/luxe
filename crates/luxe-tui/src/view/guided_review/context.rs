//! Guided Review context panes — narrow/medium headers, Guide/Files summaries, and
//! the finding/outcome summary strings.

mod foot;
mod guide;
mod metadata;
mod summary;

use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
pub(super) use guide::render_guide_pane;
pub(super) use metadata::medium_context_lines;
use metadata::{review_metadata_summary, review_title};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
pub(super) use summary::{submission_dispatch_label, with_the_agent};
use unicode_width::UnicodeWidthStr;

pub(super) fn render_narrow_context(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    let chapter_index = app
        .guided_review_workspace
        .position
        .chapter_id
        .as_ref()
        .and_then(|id| review.chapters.iter().position(|chapter| &chapter.id == id))
        .unwrap_or_default();
    let chapter_title = review
        .chapters
        .get(chapter_index)
        .map_or_else(|| "Review".into(), |chapter| chapter.title.clone());
    let file_path = app
        .guided_review_workspace
        .position
        .file_id
        .as_ref()
        .and_then(|id| review.files.iter().find(|file| &file.id == id))
        .and_then(|file| file.new_path.as_deref().or(file.old_path.as_deref()))
        .map_or_else(|| "(unknown path)".into(), |path| path.display().to_string());
    let (hidden_hunks, _) = app.guided_review_workspace.hidden_counts(&review.files);
    let hidden = if hidden_hunks > 0 {
        if app.guided_review_workspace.noise_details_visible {
            format!(" · {hidden_hunks} fmt/gen/lock/mech")
        } else {
            format!(" · {hidden_hunks} collapsed")
        }
    } else {
        Default::default()
    };
    // Budget the path last and left-truncate it, so the filename and the folded-hunk count both
    // survive. A plain format string let the terminal cut the row mid-path and drop the count.
    let prefix = format!(
        "{:02}/{:02} · {chapter_title} · ",
        chapter_index + 1,
        review.chapters.len().max(1)
    );
    let budget =
        usize::from(area.width).saturating_sub(prefix.width()).saturating_sub(hidden.width());
    let lines = vec![
        Line::from(Span::styled(
            clip_terminal_cells_ellipsis(review_title(review), usize::from(area.width)),
            app.theme_styles.accent.add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(Span::styled(
            clip_terminal_cells_ellipsis(
                &review_metadata_summary(app, review),
                usize::from(area.width),
            ),
            app.theme_styles.muted,
        )),
        Line::from(Span::styled(
            format!(
                "{prefix}{}{hidden}",
                crate::widgets::elide_path_components(&file_path, budget)
            ),
            app.theme_styles.muted,
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn render_medium_context(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    frame.render_widget(
        Paragraph::new(medium_context_lines(app, review)).wrap(Wrap { trim: true }),
        area,
    );
}

/// Formats `count` with the singular noun when it is exactly 1, else the plural noun.
fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// The git-status color for a Files-mode status letter, matching the dock's FILES palette.
pub(super) fn review_file_status_style(
    status: luxe_core::guided_review::ReviewFileStatus,
    app: &App,
) -> ratatui::style::Style {
    use luxe_core::guided_review::ReviewFileStatus as Status;
    match status {
        Status::Added | Status::Untracked => app.theme_styles.success,
        Status::Modified => app.theme_styles.warning,
        Status::Deleted => app.theme_styles.error,
        Status::Renamed | Status::Copied => app.theme_styles.info,
    }
}

// === The guide pane ===
//
// The left half of the review: what the change is, in the order a person would explain it, with
// the reviewer's place marked in it. The right half is the evidence under whatever it has
// selected.
//
// Three zones, which is the Context Dock's own shape. The head says what is being read, the foot
// says what it wants from you, and everything between is the thing itself — so the only part that
// moves is the part worth watching. The pane takes clicks and the wheel and holds no cursor of its
// own: [`crate::guided_review::ReviewPosition`] is the surface's one selection, and this follows
// it.

/// One line of context where the guide pane is put away, so the reader still knows where they are.
pub(super) fn render_hidden_guide_header(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    let styles = &app.theme_styles;
    let mut spans = vec![Span::raw(" ")];
    if let Some((index, chapter)) = review.chapters.iter().enumerate().find(|(_, chapter)| {
        app.guided_review_workspace.position.chapter_id.as_ref() == Some(&chapter.id)
    }) {
        spans.push(Span::styled(
            format!("{:02} / {:02} ", index + 1, review.chapters.len()),
            styles.dim,
        ));
        spans.push(Span::styled(chapter.title.clone(), styles.accent));
    } else {
        spans.push(Span::styled(review_title(review).to_string(), styles.accent));
    }
    if let Some((at, total)) = app.guided_review_workspace.walkthrough_step(review) {
        spans.push(Span::styled(format!("   Step {at} of {total}"), styles.muted));
    }
    spans.push(Span::styled("   alt+v shows the guide", styles.dim));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests;
