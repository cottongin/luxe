//! Guided Review view — the review surface: the guide and rail, the diff, the footer, and every
//! dialog drawn over them.
//!
//! This surface deliberately owns the terminal while a review is open. It never asks the
//! conversation or Context Dock to infer review state from transcript text.

pub(crate) mod brief;
pub(crate) mod context;
mod diff_render;
mod feedback_window;
mod footer;
mod github_post;
mod load;
mod narration;
mod overlays;
mod pull_request_fix;
mod pull_request_opening;
mod stack_opening;
mod thread_answers;

use context::{
    render_guide_pane, render_hidden_guide_header, render_medium_context, render_narrow_context,
};
use diff_render::render_content;
use github_post::render_github_post;
use overlays::{
    render_commit_preview, render_finding_details, render_finding_draft, render_finding_fix,
    render_finish_review, render_help, render_source_picker,
};
use pull_request_fix::render_pull_request_fix;
use pull_request_opening::render_pull_request_opening;
use stack_opening::render_stack_opening;
use thread_answers::render_thread_answers;

use crate::app::App;
use crate::guided_review::{GuidedReviewLoadState, ReviewSourcePickerState};
pub(super) use footer::chord;
use footer::{footer_hint_line, footer_spans, publish_footer_regions, resolve_footer};
use load::{render_body, render_minimum_size, source_label};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Borders, Paragraph, Wrap},
    Frame,
};
#[cfg(test)]
use {
    crate::guided_review::ReviewSourcePickerSelection,
    crate::mouse::ReviewFooterAction,
    footer::{fit_footer_segments, footer_segments_width, is_key, ready_footer_segments},
    luxe_core::guided_review::ReviewTarget,
    ratatui::style::Style,
    unicode_width::UnicodeWidthStr,
};

/// Draws the rule between the rail and the diff, from one row under the top to the bottom.
///
/// It runs through the footer's row on purpose: the shortcuts belong to the diff pane and start
/// beside the rule, so carrying it down is what says which pane they are for. The row clear at the
/// top matches the content pane's own top padding.
fn render_rail_divider(frame: &mut Frame, area: Rect, guide: Rect, app: &App) {
    let rule = if area.height >= 2 {
        Rect { y: area.y.saturating_add(1), height: area.height.saturating_sub(1), ..area }
    } else {
        area
    };
    let (min, max) = crate::workspace_layout::guide_width_bounds(frame.area().width);
    let handle = crate::mouse::DividerHandle {
        pane: crate::workspace_layout::Pane::ReviewGuide,
        anchor: guide.x,
        pane_on_left: true,
        min,
        max,
    };
    super::divider::render(frame, rule, Borders::RIGHT, area, handle, app);
}

/// Renders the dedicated full-screen Guided Review source state.
pub(super) fn render(frame: &mut Frame, inner: Rect, app: &App) {
    // Borderless surface: a dock-style left rail carries the review metadata and Guide, divided
    // from the content pane by a single rule — no outer frame, matching the rest of Luxe. The
    // metadata lives in the rail (wide) or the context header (medium/narrow).
    if !app.guided_review_workspace.settle_surface(inner.width >= 40 && inner.height >= 5) {
        render_minimum_size(frame, inner, app);
        return;
    }

    // The keys sit under a rule of their own, the way every modal's do
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(crate::widgets::FOOTER_ROWS)])
        .split(inner);
    let review = app.guided_review.as_ref().filter(|review| {
        matches!(
            app.guided_review_workspace.load_state,
            GuidedReviewLoadState::Ready
                | GuidedReviewLoadState::Loading { .. }
                | GuidedReviewLoadState::Failed { .. }
        ) && review
            .files
            .iter()
            .any(|file| (file.is_text() && !file.hunks.is_empty()) || file.non_text.is_some())
    });
    // The band the keys and their rule take. The wide tier narrows it to the diff's column.
    let footer_band = match review {
        Some(review) => {
            render_ready_columns(frame, inner, sections[0], app, review).unwrap_or(sections[1])
        }
        None => {
            render_body(frame, sections[0], app);
            sections[1]
        }
    };
    let modal_open = review_modal_open(app);
    // One set of keys on screen at a time: under a window the footer's keys do nothing, and
    // two hint rows read as one list
    if !(modal_open || app.guided_review_workspace.findings_visible) && footer_band.height > 0 {
        render_review_footer(frame, footer_band, app);
    }
    if modal_open {
        crate::widgets::dim_backdrop(frame, inner, &app.theme_styles);
    }
    render_review_overlays(frame, inner, app);
}

/// Draws a loaded review's panes for the width it has: the guide beside the diff, the diff under
/// one line of context, or under a context block. Returns the footer band when the wide tier
/// narrows it to the diff's column.
fn render_ready_columns(
    frame: &mut Frame,
    inner: Rect,
    surface: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Option<Rect> {
    // Two panes: the guide, and the evidence under it. There is no mode to switch — the story
    // and the code it describes are both on screen, which is what deleted the tab bar.
    if inner.width >= crate::workspace_layout::WIDE_LAYOUT_MIN_WIDTH
        && app.guided_review_workspace.guide_visible
    {
        // The rail and its rule take the full height, and the footer lives under the diff: the
        // shortcuts are the diff pane's, and a row spanning both panes read as split between
        // them.
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.guided_review_workspace.guide_width(inner.width)),
                Constraint::Length(crate::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH),
                Constraint::Min(1),
            ])
            .split(inner);
        let column = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(crate::widgets::FOOTER_ROWS)])
            .split(panes[2]);
        render_guide_pane(frame, panes[0], app, review);
        render_rail_divider(frame, panes[1], panes[0], app);
        render_content(frame, column[0], app, review, true);
        // Wheel over the guide moves the selection — it holds no scroll of its own, because it
        // holds no cursor of its own. Over the diff it scrolls source lines.
        let mut hit = app.hit.borrow_mut();
        hit.push_scroll(panes[0], crate::mouse::ScrollRegionId::ReviewGuide);
        hit.push_scroll(column[0], crate::mouse::ScrollRegionId::ReviewDiff);
        return Some(column[1]);
    } else if inner.width >= crate::workspace_layout::WIDE_LAYOUT_MIN_WIDTH {
        // The guide is put away. One line of context, then the whole screen for the code.
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(surface);
        render_hidden_guide_header(frame, panes[0], app, review);
        render_content(frame, panes[1], app, review, true);
        app.hit.borrow_mut().push_scroll(panes[1], crate::mouse::ScrollRegionId::ReviewDiff);
    } else if inner.width >= 90 {
        // Size the context block to what it actually says. A fixed reservation left a band of
        // dead rows above the diff on the tier with the least vertical space to spare.
        let context_height = medium_context_height(app, review, surface.width)
            .min(surface.height.saturating_sub(1))
            .min(10);
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(context_height), Constraint::Min(1)])
            .split(surface);
        render_medium_context(frame, panes[0], app, review);
        render_content(frame, panes[1], app, review, false);
        app.hit.borrow_mut().push_scroll(panes[1], crate::mouse::ScrollRegionId::ReviewDiff);
    } else {
        let header_height = surface.height.min(3);
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(header_height), Constraint::Min(1)])
            .split(surface);
        if header_height > 0 {
            render_narrow_context(frame, panes[0], app, review);
        }
        render_content(frame, panes[1], app, review, false);
        app.hit.borrow_mut().push_scroll(panes[1], crate::mouse::ScrollRegionId::ReviewDiff);
    }
    None
}

/// Whether a dialog over the review has the keyboard.
///
/// Modal overlays dim the surface behind them so they read as focused over a backgrounded
/// review, rather than appearing to clip the guide text. The feedback window takes the keyboard
/// like any of them, so it clears the footer too, though it keeps the surface bright.
fn review_modal_open(app: &App) -> bool {
    matches!(app.guided_review_workspace.source_picker, ReviewSourcePickerState::Open { .. })
        || app.guided_review_workspace.help_visible
        || app.guided_review_workspace.finding_details_visible
        || app.guided_review_workspace.finding_fix_visible
        || app.guided_review_workspace.finding_draft.is_some()
        || app.guided_review_workspace.finish_review_choice.is_some()
        || app.guided_review_workspace.send_preview.is_some()
        || app.guided_review_workspace.commit_preview.is_some()
        || app.guided_review_workspace.github_post.is_some()
        || app.guided_review_workspace.thread_answers.is_some()
        || app.guided_review_workspace.pull_request_opening.is_some()
        || app.guided_review_workspace.pull_request_fix.is_some()
        || app.guided_review_workspace.stack_opening.is_some()
}

/// Draws the shortcut row under its rule at the bottom of `footer_band`, and publishes each
/// shortcut's click.
fn render_review_footer(frame: &mut Frame, footer_band: Rect, app: &App) {
    let keys_row = Rect { y: footer_band.bottom() - 1, height: 1, ..footer_band };
    // Left-aligned under the text above it, one cell in like the diff's own padding
    let footer_area = Rect {
        x: keys_row.x.saturating_add(1),
        width: keys_row.width.saturating_sub(1),
        ..keys_row
    };
    // From the same cell, like the rule under the narration: flush with the pane, it all but
    // met the divider beside it
    if footer_band.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(usize::from(footer_area.width)),
                app.theme_styles.border_muted,
            ))),
            Rect { y: keys_row.y - 1, ..footer_area },
        );
    }
    let (footer_text, footer_segments) = resolve_footer(app, footer_area.width);
    let footer_line = match footer_segments.as_ref() {
        Some(segments) => footer_spans(app, segments, footer_area.width),
        None => footer_hint_line(&footer_text, &app.theme_styles),
    };
    frame.render_widget(Paragraph::new(footer_line).style(app.theme_styles.muted), footer_area);
    // A click region per shortcut; only the interactive "ready" footer is clickable
    if let Some(segments) = footer_segments.as_ref() {
        publish_footer_regions(app, footer_area, segments);
    }
}

/// Draws whatever dialog is open over the review, in the order they stack.
fn render_review_overlays(frame: &mut Frame, inner: Rect, app: &App) {
    if let ReviewSourcePickerState::Open { selection } = app.guided_review_workspace.source_picker {
        render_source_picker(frame, inner, app, selection);
    }
    if app.guided_review_workspace.help_visible {
        render_help(frame, inner, app);
    }
    // The drawer is a non-modal companion, but the detail modals are *about* the row it selects,
    // so it stands down instead of showing through half-covered behind them.
    if app.guided_review_workspace.findings_visible
        && !app.guided_review_workspace.finding_details_visible
        && !app.guided_review_workspace.finding_fix_visible
        && app.guided_review_workspace.send_preview.is_none()
    {
        feedback_window::render_feedback_window(
            frame,
            inner,
            app,
            feedback_window::FeedbackMode::Manage,
        );
    }
    if app.guided_review_workspace.finding_details_visible {
        render_finding_details(frame, inner, app);
    }
    if app.guided_review_workspace.finding_fix_visible {
        render_finding_fix(frame, inner, app);
    }
    // Under the composer: a summary is written from the preview, over it
    if let Some(post) = app.guided_review_workspace.github_post.as_ref() {
        render_github_post(frame, inner, app, post);
    }
    if let Some(answers) = app.guided_review_workspace.thread_answers.as_ref() {
        render_thread_answers(frame, inner, app, answers);
    }
    if let Some(opening) = app.guided_review_workspace.pull_request_opening.as_ref() {
        render_pull_request_opening(frame, inner, app, opening);
    }
    if let Some(fix) = app.guided_review_workspace.pull_request_fix.as_ref() {
        render_pull_request_fix(frame, inner, app, fix);
    }
    if let Some(stack) = app.guided_review_workspace.stack_opening.as_ref() {
        render_stack_opening(frame, inner, app, stack);
    }
    if let Some(draft) = app.guided_review_workspace.finding_draft.as_ref() {
        render_finding_draft(frame, inner, app, draft);
    }
    if let Some(choice) = app.guided_review_workspace.finish_review_choice {
        render_finish_review(frame, inner, app, choice);
    }
    if let Some(preview) = app.guided_review_workspace.send_preview.as_ref() {
        feedback_window::render_feedback_window(
            frame,
            inner,
            app,
            feedback_window::FeedbackMode::Send(preview),
        );
    }
    if let Some(preview) = app.guided_review_workspace.commit_preview.as_ref() {
        render_commit_preview(frame, inner, app, preview);
    }
}

/// Wrapped height of the medium-tier context block, so the diff gets every row it does not need.
fn medium_context_height(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    width: u16,
) -> u16 {
    context::medium_context_lines(app, review)
        .into_iter()
        .map(|line| Paragraph::new(line).wrap(Wrap { trim: true }).line_count(width) as u16)
        .sum()
}

/// What a loaded review is of: a pull request named with its repository, else [`source_label`].
pub(crate) fn review_label(review: &luxe_core::guided_review::GuidedReview) -> String {
    luxe_core::guided_review::pull_request_label(review)
        .unwrap_or_else(|| source_label(&review.target))
}

#[cfg(test)]
mod tests;
