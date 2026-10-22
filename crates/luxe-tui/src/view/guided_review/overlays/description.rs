//! Pull request description — the author's whole description, scrollable over the review.
//!
//! The guide shows only its opening, because a long description pushes the chapters out of view.
//! The rest used to mean leaving for GitHub, and descriptions written by agents are often long
//! enough that the opening is a small part of them, so this reads it here instead.

use super::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::view::guided_review::brief::prose_lines;
use luxe_core::guided_review::PullRequestContext;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

/// The widest the reading column gets. Prose much wider than this is hard to follow line to line.
const MAX_READING_WIDTH: u16 = 100;

/// Draws the description over `area`, scrolled down `scroll` rows, and records how far it can
/// scroll so the keys stop at its end.
pub(in crate::view::guided_review) fn render_pull_request_description(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    scroll: u16,
) {
    let Some(pr) =
        app.guided_review.as_ref().and_then(|review| review.context.pull_request.as_ref())
    else {
        return;
    };
    let popup = popup_area(area);
    let block = overlay_block(app, Icon::PullRequest.glyph(), "What the pull request says");
    let body = crate::widgets::footed_body(&block, popup);
    let lines = description_lines(app, pr, usize::from(body.width));
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let room = total.saturating_sub(body.height);
    app.guided_review_workspace.description_room.set(room);
    let scroll = scroll.min(room);
    let count = position(scroll, body.height, total);
    frame.render_widget(Clear, popup);
    crate::widgets::render_footed(frame, block, popup, HINT, count.as_deref(), &app.theme_styles);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
}

/// The footer's keys.
const HINT: &str = "\u{2191}\u{2193} scroll \u{b7} esc close";

/// Where the window is in the description (`21–40 of 64`), or `None` when it all fits.
fn position(first: u16, rows: u16, total: u16) -> Option<String> {
    (total > rows).then(|| format!("{}\u{2013}{} of {total}", first + 1, (first + rows).min(total)))
}

/// A centered window most of the review's height, at a width prose reads well at.
fn popup_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(MAX_READING_WIDTH);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// The title, the branches, and the description as written, every line of it.
fn description_lines(app: &App, pr: &PullRequestContext, width: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines = prose_lines(&pr.title, app, width, "");
    lines.push(Line::from(Span::styled(
        format!("{} \u{2192} {}", pr.head_ref, pr.base_ref),
        styles.dim,
    )));
    lines.push(Line::default());
    lines.extend(prose_lines(&pr.body, app, width, ""));
    lines
}
