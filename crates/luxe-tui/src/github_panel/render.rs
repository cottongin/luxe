//! The GitHub panel's drawing — its chrome, the list of each tab, the detail, and a failed read.

use super::{rows, Focus, GithubPanel, SPLIT_MIN_WIDTH};
use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// The icon a section's tab wears, read from what it searches for: your review, issues, your own,
/// pull requests, or anything else a configured section asks GitHub.
pub fn section_icon(query: &str) -> Icon {
    let has = |qualifier: &str| query.split_whitespace().any(|term| term.starts_with(qualifier));
    if has("review-requested:") {
        Icon::ReviewRequested
    } else if has("is:issue") {
        Icon::Issue
    } else if has("author:@me") {
        Icon::Mine
    } else if has("is:pr") {
        Icon::PullRequest
    } else {
        Icon::Search
    }
}

/// Draws `/github` over `area`, the whole terminal, less a margin.
///
/// Args: `(frame, area, panel, styles, spinner frame)`.
pub fn render(f: &mut Frame, area: Rect, panel: &GithubPanel, styles: &ThemeStyles, frame: usize) {
    // The whole screen goes, not just the frame: a sliver of dock beside a modal this size reads
    // as a rendering fault
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new("").style(styles.background), area);
    let modal = crate::widgets::centered_rect(
        area.width.saturating_sub(2).min(200),
        area.height.saturating_sub(2),
        area,
    );
    f.render_widget(Clear, modal);
    let title = match &panel.repository {
        Some(repository) => format!("GitHub · {repository}"),
        None => "GitHub".into(),
    };
    let block = crate::widgets::titled_panel(crate::glyphs::Icon::GitHub.glyph(), &title, styles)
        .padding(Padding::horizontal(1));
    let rows = panel.rows().len();
    let count = (rows > 0).then(|| format!("{}/{rows}", panel.selected + 1));
    let body =
        crate::widgets::render_footed(f, block, modal, &panel.hint(), count.as_deref(), styles);
    let now = panel.now();
    let detail = panel.detail();
    let list_focused = panel.focus == Focus::List || detail.is_none();

    // The tabs choose what the whole panel shows, so they run across all of it
    let body = if panel.sections.is_some() && body.height > 2 {
        let tabs = rows::tab_line(panel, usize::from(body.width), styles);
        f.render_widget(Paragraph::new(tabs), Rect { height: 1, ..body });
        let rule = Span::styled("─".repeat(usize::from(body.width)), styles.border_muted);
        f.render_widget(
            Paragraph::new(Line::from(rule)),
            Rect { y: body.y + 1, height: 1, ..body },
        );
        Rect { y: body.y + 3, height: body.height - 3, ..body }
    } else {
        body
    };

    if body.width < SPLIT_MIN_WIDTH {
        match detail.filter(|_| !list_focused) {
            Some(detail) => crate::github_detail::render(f, body, detail, true, now, styles),
            None => draw_list(f, body, panel, true, now, styles, frame),
        }
    } else {
        let list_width = (body.width * 2 / 5).clamp(56, 76);
        let list = Rect { width: list_width, ..body };
        draw_list(f, list, panel, list_focused, now, styles, frame);
        let rule: Vec<Line> =
            (0..body.height).map(|_| Line::from(Span::styled("│", styles.border_muted))).collect();
        f.render_widget(Paragraph::new(rule), Rect { x: list.right() + 1, width: 1, ..body });
        let pane =
            Rect { x: list.right() + 3, width: body.width.saturating_sub(list_width + 3), ..body };
        match detail {
            Some(detail) => {
                crate::github_detail::render(f, pane, detail, !list_focused, now, styles)
            }
            None if panel.rows().is_empty() => {}
            None if failed(panel).is_some() => {
                let reason = failed(panel).unwrap_or_default();
                let line = Line::from(crate::widgets::code_spans(&reason, styles.error, styles));
                f.render_widget(
                    Paragraph::new(line).wrap(ratatui::widgets::Wrap { trim: false }),
                    pane,
                );
            }
            None => {
                let spinner = crate::statusline::spinner(frame);
                let line = Line::from(Span::styled(format!("{spinner} Reading it…"), styles.muted));
                f.render_widget(Paragraph::new(line), pane);
            }
        }
    }
    if let Some(detail) = panel.detail() {
        crate::github_detail::render_overlays(f, modal, detail, frame, styles);
    }
}

/// Why the row under the cursor has no overview, when reading it failed.
fn failed(panel: &GithubPanel) -> Option<String> {
    let item = panel.rows().get(panel.selected).copied()?;
    panel.failures.get(&item.url).cloned()
}

fn draw_list(
    f: &mut Frame,
    area: Rect,
    panel: &GithubPanel,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
    frame: usize,
) {
    let lines = rows::list_lines(
        panel,
        usize::from(area.width),
        usize::from(area.height),
        focused,
        now,
        styles,
        frame,
    );
    f.render_widget(Paragraph::new(lines), area);
    // The search being typed, over its line, with the terminal's cursor where typing goes
    if let Some(editor) = &panel.editing {
        let count = rows::query_count(panel, frame).width() as u16;
        let field = Rect {
            x: area.x + rows::QUERY_INSET,
            width: area.width.saturating_sub(rows::QUERY_INSET + count + 1),
            height: 1,
            ..area
        };
        crate::widgets::text_field::render(f, field, editor, styles);
    }
}
