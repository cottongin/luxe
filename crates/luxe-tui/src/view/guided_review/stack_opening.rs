//! The stack preview — every pull request a review's chapters open as, before they do.

use super::overlays::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use crate::guided_review::StackOpening;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

pub(super) fn render_stack_opening(frame: &mut Frame, area: Rect, app: &App, stack: &StackOpening) {
    let styles = &app.theme_styles;
    let width = area.width.saturating_sub(4).clamp(50, 96).min(area.width);
    let row_width = usize::from(width.saturating_sub(4));
    let spinner = crate::statusline::spinner(app.guided_review_workspace.frame);
    let mut lines = Vec::new();
    match &stack.layers {
        None if stack.error.is_none() => lines.push(Line::from(Span::styled(
            format!("{spinner} Drafting a pull request per chapter…"),
            styles.muted,
        ))),
        None => {}
        Some(layers) => {
            let into = layers.first().map_or("", |layer| layer.base.as_str());
            let mut parts = vec![format!(
                "{} pull requests into `{into}`, each based on the one below",
                layers.len()
            )];
            if stack.draft {
                parts.push("as drafts".to_owned());
            }
            parts.push("commits and pushes first".to_owned());
            for row in super::pull_request_opening::heading_rows(&parts, row_width) {
                lines.push(Line::from(crate::widgets::code_spans(&row, styles.accent, styles)));
            }
            lines.push(Line::from(""));
            for (index, layer) in layers.iter().enumerate() {
                let row = format!("{}. `{}` — {}", index + 1, layer.head, layer.title);
                lines.push(Line::from(crate::widgets::code_spans(
                    &clip_terminal_cells_ellipsis(&row, row_width),
                    styles.text,
                    styles,
                )));
            }
        }
    }
    if let Some(error) = stack.error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(error.to_owned(), styles.error)));
    }
    if stack.opening {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("{spinner} Opening the stack…"), styles.muted)));
    }
    let inner_width = usize::from(width.saturating_sub(4)).max(1);
    let rows: usize =
        lines.iter().map(|line| line.width().max(1).div_ceil(inner_width)).sum::<usize>()
            + 2
            + usize::from(crate::widgets::FOOTER_ROWS);
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);
    let Some(popup) = crate::widgets::fit_review_preview(frame, area, (width, rows, 8), app) else {
        return;
    };
    let hint = match (stack.opening, stack.layers.is_some()) {
        (true, _) => " opening… ",
        (false, true) => " enter open · d draft · esc cancel ",
        (false, false) => " esc close ",
    };
    let block = overlay_block(app, Icon::GitHub.glyph(), "Open as a stack");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(frame, block, popup, hint, None, &app.theme_styles);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
}
