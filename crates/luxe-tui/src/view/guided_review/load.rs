//! Review load states — what the surface says before there is a diff to show: too small, loading,
//! empty, or failed.

use crate::app::App;
use crate::guided_review::GuidedReviewLoadState;
use luxe_core::guided_review::ReviewTarget;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

pub(super) fn render_minimum_size(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("Review needs 40 × 5 cells", app.theme_styles.error)),
            Line::from(Span::styled("Resize, or esc back to chat", app.theme_styles.muted)),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

pub(super) fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    let (headline, detail, style) = match &app.guided_review_workspace.load_state {
        GuidedReviewLoadState::Idle => (
            "Ready to open a review".into(),
            "Choose a source to capture an immutable review revision.".into(),
            app.theme_styles.text,
        ),
        GuidedReviewLoadState::Loading { target: ReviewTarget::PullRequest { number, .. } } => (
            format!("Fetching pull request #{number}"),
            "Reading it from GitHub and pinning the exact revision. Your working tree is not touched."
                .into(),
            app.theme_styles.accent,
        ),
        GuidedReviewLoadState::Loading { target } => {
            let detail = if app.guided_review.is_some() {
                "Refreshing the source without replacing the pinned revision you are reading."
            } else {
                "Capturing a complete immutable source before presenting its diff."
            };
            (
                format!("Capturing {}", source_label(target)),
                detail.into(),
                app.theme_styles.accent,
            )
        }
        GuidedReviewLoadState::Ready if app.guided_review.as_ref().is_some_and(|review| review.files.is_empty()) => (
            "No changes to review".into(),
            "This captured source contains no reviewable changes. Choose another source or return to the conversation.".into(),
            app.theme_styles.success,
        ),
        GuidedReviewLoadState::Ready => (
            "Review ready".into(),
            "Preparing the selected immutable revision for Guided Review.".into(),
            app.theme_styles.success,
        ),
        GuidedReviewLoadState::Failed { error, .. } => (
            "Guided Review could not load".into(),
            error.message.clone(),
            app.theme_styles.error,
        ),
    };
    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Length(1), Constraint::Length(2)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(headline, style))).alignment(Alignment::Center),
        content[1],
    );
    frame.render_widget(
        Paragraph::new(detail)
            .style(app.theme_styles.muted)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        content[2],
    );
}

pub(super) fn source_label(target: &ReviewTarget) -> String {
    match target {
        ReviewTarget::WorkingTree => "working tree".into(),
        ReviewTarget::LastTurn { .. } => "last turn".into(),
        ReviewTarget::Base { reference } => format!("base {reference}"),
        ReviewTarget::Commit { oid } => format!("commit {oid}"),
        ReviewTarget::PullRequest { number, repository: None } => format!("pull request #{number}"),
        ReviewTarget::PullRequest { number, repository: Some(repository) } => {
            format!("pull request {repository}#{number}")
        }
        ReviewTarget::OwnPullRequest { number } => format!("pull request #{number}"),
    }
}
