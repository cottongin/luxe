//! The answers preview — what goes back to your own pull request's threads, before it does.

use super::overlays::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use crate::widgets::fit_review_preview;
use luxe_core::guided_review::ReviewFinding;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// One thread's rows: its cursor, what answering does to it, the reply typed so far and any error.
///
/// Args: `(app, answers, (index, row, finding), row_width)`.
fn thread_row(
    app: &App,
    answers: &crate::guided_review::ThreadAnswers,
    (index, row, finding): (usize, &crate::guided_review::AnswerRow, &ReviewFinding),
    row_width: usize,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines = Vec::new();
    let thread = finding.github_thread.as_ref();
    let cursor = if index == answers.selected { "›" } else { " " };
    let conversation = thread.is_some_and(|thread| thread.conversation);
    let can_resolve = thread.is_some_and(|thread| thread.can_resolve);
    let (mark, action) = match (row.resolve, can_resolve, conversation) {
        (true, _, _) => (crate::glyphs::Mark::Yes.glyph(), "resolve"),
        // Said in the conversation, where a reply is a new comment and nothing resolves
        (false, _, true) => ("○", "conversation"),
        (false, true, false) => ("○", "leave open"),
        (false, false, false) => ("○", "cannot resolve"),
    };
    let state = match finding.state {
        luxe_core::guided_review::FindingState::Resolved => "verified",
        luxe_core::guided_review::FindingState::NeedsVerification => "to verify",
        _ => "open",
    };
    // No `R2`: that is the agent's name for a thread, and the title already says who opened it
    let head = format!("{cursor} {mark} {action:<15}");
    let detail = format!("{state} · {}", finding.title);
    lines.push(Line::from(vec![
        Span::styled(head.clone(), styles.accent),
        Span::styled(
            clip_terminal_cells_ellipsis(&detail, row_width.saturating_sub(head.width())),
            styles.text,
        ),
    ]));
    let editing = index == answers.selected && answers.replying.is_some();
    if !editing && !row.reply.is_empty() {
        let reply = format!("      reply: {}", row.reply.replace('\n', " "));
        lines.push(Line::from(Span::styled(
            clip_terminal_cells_ellipsis(&reply, row_width),
            styles.muted,
        )));
    }
    if let Some(error) = row.error.as_deref() {
        lines.push(Line::from(Span::styled(format!("      {error}"), styles.error)));
    }
    lines
}

/// The list above the reply field: what this posts, whether the fix is on GitHub, one row per
/// thread, and what went wrong or is still happening.
fn answer_lines(
    app: &App,
    answers: &crate::guided_review::ThreadAnswers,
    row_width: usize,
) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let title = app
        .guided_review
        .as_ref()
        .and_then(|review| review.context.pull_request.as_ref())
        .map_or_else(
            || "Answer these threads on GitHub".to_owned(),
            |pr| format!("Answer these threads on {}#{}", pr.repository, pr.number),
        );
    let mut lines = vec![
        Line::from(Span::styled(title, styles.accent)),
        Line::from(Span::styled(
            "Posts your reply to each thread. Resolving closes the thread; leave it open for the \
             reviewer to close.",
            styles.muted,
        )),
    ];
    match answers.prepared {
        None => lines.push(Line::from(Span::styled(
            format!(
                "{} Asking GitHub whether the fix is there…",
                crate::statusline::spinner(app.guided_review_workspace.frame)
            ),
            styles.muted,
        ))),
        Some((pushed, uncommitted)) => {
            // A thread answered "fixed" points the reviewer at the pull request's head, so a fix
            // that never reached it is the one thing worth stopping for
            let warning = match (pushed, uncommitted) {
                (true, false) => None,
                (true, true) => {
                    Some("Some changes here are not committed yet, so they are not on GitHub.")
                }
                (false, false) => Some("Your fix is not on GitHub yet: push this branch first."),
                (false, true) => Some("Your fix is not on GitHub yet: commit and push it first."),
            };
            if let Some(warning) = warning {
                lines.push(Line::from(Span::styled(warning, styles.warning)));
            }
        }
    }
    lines.push(Line::from(""));

    let findings = app.review.session().map_or(&[][..], |session| &session.findings[..]);
    for (index, row) in answers.rows.iter().enumerate() {
        if let Some(finding) = findings.iter().find(|finding| finding.id == row.finding_id) {
            lines.extend(thread_row(app, answers, (index, row, finding), row_width));
        }
    }
    if let Some(error) = answers.error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(error.to_owned(), styles.error)));
    }
    if answers.sending {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "{} Answering on GitHub…",
                crate::statusline::spinner(app.guided_review_workspace.frame)
            ),
            styles.muted,
        )));
    }
    lines
}

pub(super) fn render_thread_answers(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    answers: &crate::guided_review::ThreadAnswers,
) {
    let styles = &app.theme_styles;
    let width = area.width.saturating_sub(4).clamp(50, 96).min(area.width);
    let row_width = usize::from(width.saturating_sub(4));
    let lines = answer_lines(app, answers, row_width);
    let findings = app.review.session().map_or(&[][..], |session| &session.findings[..]);

    // The reply is written in a field of its own under the list, where the cursor can be
    let replying = answers.replying.as_ref().map(|editor| {
        let label = answers
            .rows
            .get(answers.selected)
            .and_then(|row| findings.iter().find(|finding| finding.id == row.finding_id))
            .and_then(|finding| finding.github_thread.as_ref())
            .map_or_else(String::new, |thread| format!("Reply to {} · ", author(&thread.author)));
        (editor, Line::from(Span::styled(format!("{label}type below"), styles.muted)))
    });
    let inner_width = width.saturating_sub(3).max(1);
    let list_rows = crate::widgets::text_field::wrapped_rows(&lines, inner_width);
    let field_rows = replying
        .as_ref()
        .map_or(0, |(editor, _)| crate::widgets::text_field::rows(editor, inner_width).clamp(3, 8));
    let reply_rows = if replying.is_some() { field_rows + 2 } else { 0 };
    let rows = list_rows + reply_rows + 2 + crate::widgets::FOOTER_ROWS;
    let Some(popup) = fit_review_preview(frame, area, (width, rows, 10), app) else {
        return;
    };
    let hint = if answers.sending {
        " answering… "
    } else if answers.replying.is_some() {
        " ctrl+enter save · esc cancel "
    } else {
        " enter post · space resolve · c reply · ↑/↓ choose · esc cancel "
    };
    let block = overlay_block(app, Icon::GitHub.glyph(), "Answer on GitHub");
    frame.render_widget(Clear, popup);
    let inner = crate::widgets::render_footed(frame, block, popup, hint, None, &app.theme_styles);
    let [list_area, reply_area] = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(1),
        ratatui::layout::Constraint::Length(reply_rows),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), list_area);
    if let Some((editor, label)) = replying {
        let [_, label_area, field_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .areas(reply_area);
        frame.render_widget(Paragraph::new(label), label_area);
        app.guided_review_workspace.field_width.set(field_area.width);
        crate::widgets::text_field::render(frame, field_area, editor, styles);
    }
}
/// A thread's author as GitHub writes one, `@login`, or what GitHub shows for an account that is
/// gone.
fn author(login: &str) -> String {
    if login.is_empty() {
        "a deleted account".to_owned()
    } else {
        format!("@{login}")
    }
}
