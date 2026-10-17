//! The pull request preview — what opens on GitHub, and what happens first, before it does.

use super::overlays::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use crate::guided_review::{OpeningField, PullRequestOpening};
use luxe_core::github::Publication;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// What the draft leaves behind and how opening is going, pinned under the description.
fn tail_lines(
    app: &App,
    opening: &PullRequestOpening,
    inner_width: u16,
    spinner: &str,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut tail = Vec::new();
    if let Some(left_out) = opening.prepared.as_ref().and_then(left_out) {
        tail.push(Line::from(Span::styled(
            clip_terminal_cells_ellipsis(&left_out, usize::from(inner_width)),
            styles.warning,
        )));
    }
    if let Some(error) = opening.error.as_deref() {
        tail.push(Line::from(crate::widgets::code_spans(error, styles.error, styles)));
    }
    if opening.opening {
        tail.push(Line::from(Span::styled(
            format!("{spinner} Opening it on GitHub…"),
            styles.muted,
        )));
    }
    tail
}

/// The heading's parts: where the pull request goes, and what is unusual about it.
fn heading_parts(prepared: &crate::guided_review::PreparedOpening) -> Vec<String> {
    let pull_request = &prepared.pull_request;
    let mut parts = vec![format!("Into `{}` from `{}`", pull_request.base, pull_request.head)];
    // Ready for review goes without saying; a draft is the exception, so only it is named
    if pull_request.draft {
        parts.push("as a draft".to_owned());
    }
    parts.extend(first(prepared).map(str::to_owned));
    parts
}

/// How tall the popup wants to be.
///
/// Args: `(opening, area, (heading_height, body_rows, tail_rows))`.
fn wanted_height(
    opening: &PullRequestOpening,
    area: Rect,
    (heading_height, body_rows, tail_rows): (u16, u16, u16),
) -> u16 {
    // As tall as the draft, up to most of the screen; all of that while writing, which needs room
    let most = area.height.saturating_sub(4).max(14);
    match (&opening.prepared, opening.editing.as_ref()) {
        (None, _) => (tail_rows + 3).max(5),
        (Some(_), Some(_)) => most,
        // Heading and its gap, the title and its label, a gap, the description and its label, then
        // the frame, and anything pinned under the description after a gap of its own
        (Some(_), None) => (heading_height
            + 3
            + 1
            + 1
            + body_rows.max(3)
            + tail_rows
            + u16::from(tail_rows > 0)
            + 2)
        .min(most),
    }
    .saturating_add(crate::widgets::FOOTER_ROWS)
    .min(area.height)
}

/// Draws the description — the editor while it is being written, otherwise the rendered markdown
/// — and returns whether it runs past its room.
fn render_description(
    frame: &mut Frame,
    app: &App,
    opening: &PullRequestOpening,
    (body, body_rows): (Option<Vec<Line<'static>>>, u16),
    body_area: Rect,
) -> bool {
    let styles = &app.theme_styles;
    let mut overflows = false;
    match (opening.editing.as_ref(), body) {
        (Some((OpeningField::Body, editor)), _) => {
            app.guided_review_workspace.field_width.set(body_area.width);
            crate::widgets::text_field::render(frame, body_area, editor, styles);
        }
        _ if opening
            .prepared
            .as_ref()
            .map_or("", |prepared| prepared.pull_request.body.as_str())
            .trim()
            .is_empty() =>
        {
            frame.render_widget(
                Paragraph::new(Span::styled("(no description)", styles.muted)),
                body_area,
            )
        }
        (_, Some(body)) => {
            // Clamped here, where the room is known, and remembered so scrolling back up
            // starts moving at once instead of first unwinding an overshoot
            let room = body_rows.saturating_sub(body_area.height);
            opening.body_room.set(room);
            overflows = room > 0;
            let scroll = opening.body_scroll.min(room);
            let text_area = Rect { width: body_area.width.saturating_sub(1), ..body_area };
            frame.render_widget(
                Paragraph::new(body).wrap(Wrap { trim: false }).scroll((scroll, 0)),
                text_area,
            );
            crate::widgets::steered_scrollbar(
                frame,
                &mut app.hit.borrow_mut(),
                body_area,
                crate::mouse::ScrollRegionId::PullRequestBody,
                usize::from(scroll),
                usize::from(body_rows),
                usize::from(body_area.height),
                styles,
            );
        }
        (_, None) => {}
    }
    overflows
}

/// The key hints for what the popup is doing right now.
fn opening_hint(opening: &PullRequestOpening, overflows: bool) -> String {
    match (opening.opening, opening.editing.as_ref(), opening.prepared.as_ref()) {
        (true, ..) => " opening… ".to_owned(),
        (_, Some((OpeningField::Title, _)), _) => " enter save · esc cancel ".to_owned(),
        (_, Some((OpeningField::Body, _)), _) => " ctrl+enter save · esc cancel ".to_owned(),
        (_, None, Some(_)) => {
            let scroll = if overflows { " · ↑/↓ scroll" } else { "" };
            format!(
                " enter open · t title · e description · E in your editor · d draft{scroll} · esc \
                 cancel "
            )
        }
        (_, None, None) if opening.error.is_some() => " enter try again · esc cancel ".to_owned(),
        (_, None, None) => " esc cancel ".to_owned(),
    }
}

/// The heading rows, with the backticks around names drawn as emphasis.
fn render_heading(frame: &mut Frame, app: &App, rows: Vec<String>, area: Rect, inner_width: u16) {
    let styles = &app.theme_styles;
    let heading = rows.into_iter().map(|row| {
        // the backticks are not drawn, so they get their room back
        let room = usize::from(inner_width) + row.matches('`').count();
        Line::from(crate::widgets::code_spans(
            &clip_terminal_cells_ellipsis(&row, room),
            styles.accent,
            styles,
        ))
    });
    frame.render_widget(Paragraph::new(heading.collect::<Vec<_>>()), area);
}

/// The title: the editor while it is being written, otherwise the title in bold.
fn render_title(
    frame: &mut Frame,
    app: &App,
    opening: &PullRequestOpening,
    area: Rect,
    inner_width: u16,
) {
    let styles = &app.theme_styles;
    match opening.editing.as_ref() {
        Some((OpeningField::Title, editor)) => {
            app.guided_review_workspace.field_width.set(area.width);
            crate::widgets::text_field::render(frame, area, editor, styles);
        }
        _ => {
            let title =
                opening.prepared.as_ref().map_or("", |prepared| &prepared.pull_request.title);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    clip_terminal_cells_ellipsis(title, usize::from(inner_width)),
                    styles.text.add_modifier(ratatui::style::Modifier::BOLD),
                )),
                area,
            );
        }
    }
}

/// Draws the pull request preview: small while it drafts, and most of the screen once there is a
/// draft to read, since agent-written descriptions run long and are markdown.
pub(super) fn render_pull_request_opening(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    opening: &PullRequestOpening,
) {
    let styles = &app.theme_styles;
    let width = area.width.saturating_sub(8).clamp(60, 120).min(area.width);
    let inner_width = width.saturating_sub(3).max(1);
    let spinner = crate::statusline::spinner(app.guided_review_workspace.frame);
    let editing = opening.editing.as_ref();

    let tail = tail_lines(app, opening, inner_width, spinner);
    let tail_rows = crate::widgets::text_field::wrapped_rows(&tail, inner_width);

    let heading = opening
        .prepared
        .as_ref()
        .map(|prepared| heading_rows(&heading_parts(prepared), usize::from(inner_width)));
    let heading_height = heading.as_ref().map_or(1, |rows| u16::try_from(rows.len()).unwrap_or(1));

    let body = opening
        .prepared
        .as_ref()
        .map(|prepared| crate::markdown::render_markdown(&prepared.pull_request.body, styles));
    let body_rows =
        body.as_ref().map_or(0, |body| crate::widgets::text_field::wrapped_rows(body, inner_width));
    let overflows = std::cell::Cell::new(false);

    let height = wanted_height(opening, area, (heading_height, body_rows, tail_rows));
    let popup = crate::widgets::centered_rect(width, height, area);
    let block = overlay_block(app, Icon::GitHub.glyph(), "Open pull request");
    let inner = crate::widgets::footed_body(&block, popup);
    frame.render_widget(Clear, popup);

    match opening.prepared.as_ref() {
        None => {
            let mut lines = Vec::new();
            if opening.error.is_none() {
                lines.push(Line::from(Span::styled(
                    format!("{spinner} Drafting it from the review…"),
                    styles.muted,
                )));
            }
            lines.extend(tail);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Some(_) => {
            let title_rows = match editing {
                Some((OpeningField::Title, editor)) => {
                    crate::widgets::text_field::rows(editor, inner_width).clamp(1, 3)
                }
                _ => 1,
            };
            let gap = u16::from(tail_rows > 0);
            let [head, title_label, title, _, body_label, body_area, _, tail_area] =
                ratatui::layout::Layout::vertical([
                    ratatui::layout::Constraint::Length(heading_height + 1),
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Length(title_rows),
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Min(3),
                    ratatui::layout::Constraint::Length(gap),
                    ratatui::layout::Constraint::Length(tail_rows),
                ])
                .areas(inner);
            render_heading(frame, app, heading.unwrap_or_default(), head, inner_width);
            frame.render_widget(Paragraph::new(label("Title", styles)), title_label);
            render_title(frame, app, opening, title, inner_width);
            frame.render_widget(Paragraph::new(label("Description", styles)), body_label);
            overflows.set(render_description(frame, app, opening, (body, body_rows), body_area));
            frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), tail_area);
        }
    }

    let hint = opening_hint(opening, overflows.get());
    crate::widgets::render_footed(frame, block, popup, &hint, None, &app.theme_styles);
}

/// A section's name, quiet so what is under it reads first.
fn label(text: &'static str, styles: &crate::theme::styles::ThemeStyles) -> Span<'static> {
    Span::styled(text, styles.muted)
}

/// `parts` joined with ` · ` into as few rows as fit `width`, breaking only between parts: a
/// heading cut mid-phrase, `commits and pushes fi…`, lost the part that mattered most.
pub(super) fn heading_rows(parts: &[String], width: usize) -> Vec<String> {
    // backticks mark code and are not drawn, so they take no room
    let cells = |text: &str| UnicodeWidthStr::width(text.replace('`', "").as_str());
    let mut rows: Vec<String> = Vec::new();
    for part in parts {
        match rows.last_mut() {
            Some(row) if cells(row) + 3 + cells(part) <= width => {
                row.push_str(" · ");
                row.push_str(part);
            }
            _ => rows.push(part.clone()),
        }
    }
    rows
}

/// What Enter does before the pull request exists, for the heading: a reader should not have to
/// find out from `git log` that opening it committed and pushed.
fn first(prepared: &crate::guided_review::PreparedOpening) -> Option<&'static str> {
    if !prepared.commits.is_empty() {
        Some("commits and pushes first")
    } else if matches!(prepared.publication, Publication::Pushed) {
        None
    } else {
        Some("pushes first")
    }
}

/// Changed files the pull request will not carry, which is the one thing opening does that a
/// reader could be surprised by afterward.
fn left_out(prepared: &crate::guided_review::PreparedOpening) -> Option<String> {
    (prepared.commits.is_empty() && prepared.uncommitted > 0).then(|| {
        let files = if prepared.uncommitted == 1 { "file" } else { "files" };
        format!("{} changed {files} left out · not in the review", prepared.uncommitted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::PreparedOpening;

    fn prepared(publication: Publication, uncommitted: usize, commits: &[&str]) -> PreparedOpening {
        PreparedOpening {
            pull_request: luxe_core::github::NewPullRequest {
                head: "me:7-fix".into(),
                base: "main".into(),
                title: "Fix".into(),
                body: String::new(),
                draft: false,
            },
            publication,
            uncommitted,
            commits: commits.iter().map(|subject| (*subject).to_owned()).collect(),
        }
    }

    #[test]
    fn a_heading_breaks_between_its_parts() {
        let parts =
            ["Into `main` from `todo-overdue`".to_owned(), "commits and pushes first".to_owned()];
        assert_eq!(
            heading_rows(&parts, 54),
            ["Into `main` from `todo-overdue` · commits and pushes first"]
        );
        assert_eq!(
            heading_rows(&parts, 53),
            ["Into `main` from `todo-overdue`", "commits and pushes first"]
        );
        assert!(heading_rows(&[], 10).is_empty());
    }

    #[test]
    fn the_heading_says_what_opening_does_first() {
        assert_eq!(first(&prepared(Publication::Pushed, 0, &[])), None);
        assert_eq!(
            first(&prepared(Publication::Pushed, 2, &["One", "Two"])),
            Some("commits and pushes first")
        );
        assert_eq!(first(&prepared(Publication::Unpublished, 0, &[])), Some("pushes first"));
        assert_eq!(
            first(&prepared(Publication::Ahead { commits: 1 }, 0, &[])),
            Some("pushes first")
        );
        assert_eq!(left_out(&prepared(Publication::Pushed, 1, &["One"])), None, "it commits them");
        assert_eq!(
            left_out(&prepared(Publication::Pushed, 2, &[])).as_deref(),
            Some("2 changed files left out · not in the review")
        );
        assert_eq!(
            left_out(&prepared(Publication::Pushed, 1, &[])).as_deref(),
            Some("1 changed file left out · not in the review")
        );
    }
}
