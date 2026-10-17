//! The offer to fix a pull request in a worktree — where it goes, what goes with it, or why not.

use super::overlays::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::PullRequestFix;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

pub(super) fn render_pull_request_fix(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    fix: &PullRequestFix,
) {
    let styles = &app.theme_styles;
    let width = area.width.saturating_sub(4).clamp(50, 84).min(area.width);
    let mut lines = Vec::new();
    match (&fix.branch, &fix.refusal) {
        (_, Some(refusal)) => lines.push(Line::from(Span::styled(refusal.clone(), styles.error))),
        (Some(branch), None) => {
            let with = match fix.comments {
                0 => String::new(),
                1 => " with your comment for the agent to address".to_string(),
                n => format!(" with your {n} comments for the agent to address"),
            };
            lines.push(Line::from(crate::widgets::code_spans(
                &format!(
                    "Checks #{} out on `{branch}` in a worktree of its own and starts a new \
                     session{with}.",
                    fix.number
                ),
                styles.text,
                styles,
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "You can come back to this session at any time.",
                styles.muted,
            )));
        }
        (None, None) => lines.push(Line::from(Span::styled(
            format!(
                "{} Asking GitHub whether a fix could reach #{}…",
                crate::statusline::spinner(app.guided_review_workspace.frame),
                fix.number
            ),
            styles.muted,
        ))),
    }
    let inner_width = usize::from(width.saturating_sub(4)).max(1);
    let rows: usize =
        lines.iter().map(|line| line.width().max(1).div_ceil(inner_width)).sum::<usize>()
            + 2
            + usize::from(crate::widgets::FOOTER_ROWS);
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);
    let Some(popup) = crate::widgets::fit_review_preview(frame, area, (width, rows, 7), app) else {
        return;
    };
    let hint = if fix.branch.is_some() && fix.refusal.is_none() {
        " enter check out · esc cancel "
    } else {
        " esc close "
    };
    let block = overlay_block(app, Icon::GitHub.glyph(), "Fix in a worktree");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(frame, block, popup, hint, None, &app.theme_styles);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
}
