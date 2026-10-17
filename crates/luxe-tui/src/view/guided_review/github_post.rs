//! The post preview — exactly what a pull request's review will say on GitHub, before it does.
//!
//! Two panes, the way the command palette reads: what will be posted on the left — the review
//! itself, then each comment that sits on a line — and the whole text of the one under the cursor
//! on the right, drawn as the markdown GitHub will render. A one-line title is never posted: what
//! GitHub shows is the comment, so that is what the preview shows. A terminal too narrow for both
//! stacks the text under the list.

mod items;
mod preview;

use super::overlays::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use items::{items, list_lines, Item};
use luxe_core::github::ReviewEvent;
use luxe_core::guided_review::PostPlanError;
use preview::pane_lines;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// The widest the dialog grows: past this a comment's lines get too long to read comfortably.
const MAX_WIDTH: u16 = 116;
/// The narrowest the list may be and still name where a comment lands.
const LIST_MIN: u16 = 24;
/// The narrowest the text pane may be and still read as prose.
const PANE_MIN: u16 = 34;
/// The most rows the dialog's body takes, however long a comment runs; the pane scrolls past it.
const MAX_BODY_ROWS: usize = 24;
pub(super) fn render_github_post(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    post: &crate::guided_review::GithubPost,
) {
    let styles = &app.theme_styles;
    let width = area.width.saturating_sub(4).clamp(50, MAX_WIDTH).min(area.width);
    let mut head = post_preview_head(app, post, width);

    let Some(prepared) = post.prepared.as_ref() else {
        head.push(Line::from(""));
        match post.error.as_deref() {
            Some(error) => {
                head.push(Line::from(Span::styled(error.to_owned(), styles.error)));
            }
            None => head.push(Line::from(Span::styled(
                format!(
                    "{} Reading the pull request…",
                    crate::statusline::spinner(app.guided_review_workspace.frame)
                ),
                styles.muted,
            ))),
        }
        return draw(frame, area, app, width, head, None, post);
    };

    let posted = posted_count(app);
    let notes = post_preview_notes(app, post, prepared, posted);
    if !notes.is_empty() {
        head.push(Line::from(""));
        head.extend(notes);
    }

    match crate::app::github_post_plan(app) {
        Some(Ok(plan)) => {
            let items = items(app, &plan);
            draw(frame, area, app, width, head, Some(&items), post);
        }
        Some(Err(error)) => {
            head.push(Line::from(""));
            head.push(Line::from(Span::styled(refusal(error, posted, prepared), styles.warning)));
            draw(frame, area, app, width, head, None, post);
        }
        None => draw(frame, area, app, width, head, None, post),
    }
}

/// The dialog's head: what it posts to, and the verdict between rules of its own.
fn post_preview_head(
    app: &App,
    post: &crate::guided_review::GithubPost,
    width: u16,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let title = app
        .guided_review
        .as_ref()
        .and_then(|review| review.context.pull_request.as_ref())
        .map_or_else(
            || "Post this review to GitHub".to_owned(),
            |pr| format!("Post this review to {}#{}", pr.repository, pr.number),
        );
    // The verdict is the one choice the dialog asks for, so it sits between rules of its own
    let inner = width.saturating_sub(3);
    let mut head = vec![Line::from(Span::styled(title, styles.accent)), rule(inner, styles)];
    head.push(verdict_row(post, styles));
    head.push(rule(inner, styles));
    head
}

/// Comments of this review already on GitHub, which posting again leaves alone.
fn posted_count(app: &App) -> usize {
    app.review.session().map_or(0, |session| {
        session.findings.iter().filter(|finding| finding.posted.is_some()).count()
    })
}

/// What the reader should know before posting: whose pull request it is, a push since they read
/// it, what is already up, an error, and the post in flight.
fn post_preview_notes(
    app: &App,
    post: &crate::guided_review::GithubPost,
    prepared: &crate::guided_review::PreparedPost,
    posted: usize,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut notes = Vec::new();
    if prepared.own_pull_request {
        notes.push(Line::from(Span::styled("Your pull request · comment only", styles.muted)));
    }
    if prepared.head_moved {
        notes.push(Line::from(Span::styled(
            format!(
                "Pushed since you read it · comments stay on {}, marked outdated",
                pinned_head(app)
            ),
            styles.warning,
        )));
    }
    if posted > 0 {
        notes.push(Line::from(Span::styled(
            match posted {
                1 => "1 comment already on GitHub · not posted again".to_owned(),
                count => format!("{count} comments already on GitHub · not posted again"),
            },
            styles.dim,
        )));
    }
    if let Some(error) = post.error.as_deref() {
        notes.push(Line::from(Span::styled(error.to_owned(), styles.error)));
    }
    if post.posting {
        notes.push(Line::from(Span::styled(
            format!(
                "{} Posting to GitHub…",
                crate::statusline::spinner(app.guided_review_workspace.frame)
            ),
            styles.muted,
        )));
    }
    notes
}

/// A rule across the dialog's inside, the one divider it uses between its parts.
/// A rule across the dialog's head, a cell clear of the frame on both sides: the panel's left pad
/// gives it one, and this the other. The footer's rule is the frame's and runs edge to edge.
fn rule(inner: u16, styles: &ThemeStyles) -> Line<'static> {
    Line::from(Span::styled("─".repeat(usize::from(inner.saturating_sub(1))), styles.border_muted))
}

/// The verdicts on offer as tabs, each with GitHub's icon for it: the chosen one cut the way every
/// open tab is, the ones GitHub will not take dimmed.
fn verdict_row(post: &crate::guided_review::GithubPost, styles: &ThemeStyles) -> Line<'static> {
    use crate::glyphs::{icon_label, Icon};
    let on_offer = post.verdicts();
    let surface = styles
        .selected_bg
        .bg
        .unwrap_or_else(|| styles.slab.at(crate::theme::styles::Depth::Raised));
    let bold = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
    let mut spans = Vec::new();
    for verdict in [ReviewEvent::Comment, ReviewEvent::Approve, ReviewEvent::RequestChanges] {
        let label = match verdict {
            ReviewEvent::Comment => icon_label(Icon::ReviewComment.glyph(), "Comment"),
            ReviewEvent::Approve => icon_label(Icon::ReviewApprove.glyph(), "Approve"),
            ReviewEvent::RequestChanges => {
                icon_label(Icon::ReviewRequestChanges.glyph(), "Request changes")
            }
        };
        if verdict == post.verdict {
            spans.extend(crate::widgets::slab(
                [Span::styled(label, styles.accent)],
                surface,
                styles.slab.canvas,
                bold,
            ));
        } else {
            let style = if on_offer.contains(&verdict) { styles.muted } else { styles.dim };
            spans.push(Span::styled(format!("  {label}  "), style));
        }
    }
    Line::from(spans)
}

/// Why nothing can be posted yet, in the words that say what to do about it.
fn refusal(
    error: PostPlanError,
    posted: usize,
    prepared: &crate::guided_review::PreparedPost,
) -> String {
    match error {
        PostPlanError::Nothing if posted > 0 => {
            "Already on GitHub · nothing new to post".to_owned()
        }
        PostPlanError::Nothing if prepared.own_pull_request => {
            "Nothing to post · no comments yet".to_owned()
        }
        PostPlanError::NeedsBody => {
            format!("{} c writes it.", crate::app::github_post_refusal(PostPlanError::NeedsBody))
        }
        error => crate::app::github_post_refusal(error),
    }
}

/// How the body splits once the head is laid out: in half, the palette's rule. Args:
/// `(list_width, pane_width)`, or `None` for one column.
fn split(inner: u16) -> Option<(u16, u16)> {
    let split = crate::picker::Split::of(inner);
    (split.list >= LIST_MIN && split.pane >= PANE_MIN).then_some((split.list, split.pane))
}

fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    width: u16,
    head: Vec<Line<'static>>,
    items: Option<&[Item]>,
    post: &crate::guided_review::GithubPost,
) {
    let styles = &app.theme_styles;
    // Borders and the block's left pad
    let inner = width.saturating_sub(3);
    let head: Vec<Line<'static>> =
        head.iter().flat_map(|line| crate::widgets::wrap_line(line, inner.into())).collect();
    let selected = items.map_or(0, |items| post.selected.min(items.len().saturating_sub(1)));
    let layout = items.map(|items| (items, split(inner)));

    // Sized for the longest text any item has, so the dialog holds still as the cursor moves
    let body_rows = match layout {
        None => 0,
        Some((items, Some((_, pane)))) => items
            .iter()
            .map(|item| pane_lines(app, item, pane.into()).len())
            .max()
            .unwrap_or(0)
            .max(list_lines(app, items, 0, pane.into()).len())
            .min(MAX_BODY_ROWS),
        Some((items, None)) => {
            let text = items
                .iter()
                .map(|item| pane_lines(app, item, inner.into()).len())
                .max()
                .unwrap_or(0);
            (list_lines(app, items, 0, inner.into()).len() + 1 + text).min(MAX_BODY_ROWS)
        }
    };
    let gap = usize::from(body_rows > 0);
    let rows = head.len() + gap + body_rows + 2 + usize::from(crate::widgets::FOOTER_ROWS);
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);
    let Some(popup) = crate::widgets::fit_review_preview(frame, area, (width, rows, 8), app) else {
        return;
    };
    let block = overlay_block(app, Icon::GitHub.glyph(), "Post to GitHub");
    frame.render_widget(Clear, popup);
    let summary = crate::app::review_draft_summary(app).is_some();
    let room = usize::from(width.saturating_sub(4));
    let keys = hint(post, items.map_or(0, <[Item]>::len), summary, room);
    let content = crate::widgets::render_footed(frame, block, popup, &keys, None, styles);

    let head_rows = u16::try_from(head.len()).unwrap_or(u16::MAX).min(content.height);
    frame.render_widget(Paragraph::new(head), Rect { height: head_rows, ..content });
    let top = content.y + head_rows + u16::try_from(gap).unwrap_or(0);
    let body = Rect { y: top, height: content.bottom().saturating_sub(top), ..content };
    let Some((items, split)) = layout else { return };
    if body.height == 0 {
        return;
    }
    let item = &items[selected];
    match split {
        Some((list, pane)) => {
            let rows = list_lines(app, items, selected, list.into());
            let at = rows.iter().position(|(item, _)| *item == Some(selected)).unwrap_or(0);
            let skip = at.saturating_sub(usize::from(body.height).saturating_sub(1));
            let rows: Vec<Line<'static>> =
                rows.into_iter().skip(skip).map(|(_, row)| row).collect();
            frame.render_widget(Paragraph::new(rows), Rect { width: list, ..body });
            let text = scrolled(pane_lines(app, item, pane.into()), post.scroll, body.height);
            crate::picker::render_pane(frame, body, list, pane, &text, styles);
        }
        None => {
            let rows: Vec<Line<'static>> = list_lines(app, items, selected, body.width.into())
                .into_iter()
                .map(|(_, row)| row)
                .collect();
            let list_rows = u16::try_from(rows.len()).unwrap_or(u16::MAX).min(body.height);
            frame.render_widget(Paragraph::new(rows), Rect { height: list_rows, ..body });
            let text_top = body.y + list_rows + 1;
            let text_area =
                Rect { y: text_top, height: body.bottom().saturating_sub(text_top), ..body };
            let text =
                scrolled(pane_lines(app, item, body.width.into()), post.scroll, text_area.height);
            frame.render_widget(Paragraph::new(text), text_area);
        }
    }
}

/// `lines` from `scroll` on, clamped so the last row can reach the bottom but not leave it.
fn scrolled(lines: Vec<Line<'static>>, scroll: u16, height: u16) -> Vec<Line<'static>> {
    let most = lines.len().saturating_sub(usize::from(height));
    lines.into_iter().skip(usize::from(scroll).min(most)).collect()
}

/// The keys, naming only what this state of the dialog will do, in the `room` the frame has.
///
/// Posting and leaving always fit; the rest drop out least-needed first, so a narrow terminal
/// loses the worktree key before it loses the one that moves between comments.
fn hint(
    post: &crate::guided_review::GithubPost,
    items: usize,
    summary: bool,
    room: usize,
) -> String {
    if post.posting {
        return " posting… ".to_owned();
    }
    if post.prepared.is_none() {
        let keys =
            if post.error.is_some() { " enter retry · esc cancel " } else { " esc cancel " };
        return keys.to_owned();
    }
    // In reading order, each with the rank it is dropped at: highest goes first
    let drafting = post.drafting.is_some();
    let keys = [
        ("enter post", 0),
        ("↑/↓ comment", if items > 1 { 2 } else { u8::MAX }),
        ("tab verdict", if post.verdicts().len() > 1 { 1 } else { u8::MAX }),
        ("drafting the summary…", if drafting { 1 } else { u8::MAX }),
        ("d draft summary", if drafting { u8::MAX } else { 2 }),
        (if summary { "e edit summary" } else { "c summary" }, 3),
        ("w fix in a worktree", 4),
        ("esc cancel", 0),
    ];
    let render = |limit: u8| {
        let shown: Vec<&str> =
            keys.iter().filter(|(_, rank)| *rank <= limit).map(|(key, _)| *key).collect();
        format!(" {} ", shown.join(" · "))
    };
    (0..=4).rev().map(render).find(|text| text.width() <= room).unwrap_or_else(|| render(0))
}

fn pinned_head(app: &App) -> String {
    match app.guided_review.as_ref().map(|review| &review.source.base) {
        Some(luxe_core::guided_review::ReviewBase::PullRequest { head_oid, .. }) => {
            head_oid.chars().take(7).collect()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pane's last row can reach the bottom, and a scroll past it stops there.
    #[test]
    fn scrolling_stops_at_the_last_row() {
        let lines: Vec<Line<'static>> = (0..10).map(|n| Line::from(n.to_string())).collect();
        assert_eq!(scrolled(lines.clone(), 0, 4)[0].to_string(), "0");
        assert_eq!(scrolled(lines.clone(), 3, 4)[0].to_string(), "3");
        assert_eq!(scrolled(lines.clone(), 99, 4)[0].to_string(), "6", "clamped to the end");
        assert_eq!(scrolled(lines, 5, 20)[0].to_string(), "0", "nothing to scroll when it fits");
    }
}
