//! Finish and commit — the dialog that ends a review, and the preview before its commit.

use super::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use crate::guided_review::FinishReviewChoice;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub(in crate::view::guided_review) fn render_finish_review(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    selected: FinishReviewChoice,
) {
    let width = area.width.saturating_sub(4).clamp(40, 64).min(area.width);
    let mut lines = finish_header(app);
    lines.extend(finish_choice_rows(app, selected, width));
    // As tall as what it says, wrapped where it will be drawn, the two borders and the footer
    let rows = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(width.saturating_sub(3)) as u16;
    let height = rows.saturating_add(2 + crate::widgets::FOOTER_ROWS);
    let Some(popup) = crate::widgets::fit_whole_review_preview(frame, area, (width, height), app)
    else {
        return;
    };
    let surface_style = app.theme_styles.text.patch(app.theme_styles.tool_bg);
    let block = overlay_block(app, Icon::Finish.glyph(), "Finish review").style(surface_style);
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(
        frame,
        block,
        popup,
        "enter finish · ↑/↓ choose · esc cancel",
        None,
        &app.theme_styles,
    );
    // `trim: false`, or the leading frame is stripped from every option except the chosen one and
    // the list steps sideways as the cursor moves. The prose above the options starts at the
    // margin, so it wraps the same either way.
    frame
        .render_widget(Paragraph::new(lines).style(surface_style).wrap(Wrap { trim: false }), body);
}

/// What finishing would carry, and the second-press notice when blocking comments remain.
fn finish_header(app: &App) -> Vec<Line<'static>> {
    // Name exactly what a submission carries. Severity travels as priority metadata, so nothing a
    // reviewer wrote is left behind by the mode they pick.
    let unsent = crate::app::input::unsent_finding_ids(app);
    let again = app.review.session().map_or(0, |session| {
        session
            .findings
            .iter()
            .filter(|finding| {
                unsent.contains(&finding.id)
                    && finding.state == luxe_core::guided_review::FindingState::Open
            })
            .count()
    });
    // The title says what the dialog is for; the first line says what finishing would carry
    let mut lines = vec![
        Line::from(Span::styled(finish_carries(unsent.len(), again), app.theme_styles.muted)),
        Line::from(""),
    ];
    if app.guided_review_workspace.finish_review_accept_confirmed {
        lines.extend([
            Line::from(Span::styled(
                "Blocking comments remain. Press enter again to accept.",
                app.theme_styles.accent,
            )),
            Line::from(""),
        ]);
    }
    lines
}

/// The finish dialog's choices, the one under the cursor highlighted, each clipped to one row.
fn finish_choice_rows(app: &App, selected: FinishReviewChoice, width: u16) -> Vec<Line<'static>> {
    // Choices clip rather than wrap: a detail that spilled onto a second, unindented line read as
    // an extra option, which is a dangerous thing to misread in a dialog that submits work.
    //
    // width - 2 borders - 1 left pad, the same sum the findings drawer makes: the pad belongs to
    // `overlay_block`, and a row sized to the border-to-border width overruns it by exactly one
    // cell, wraps, and opens a gap under whichever option the cursor is on.
    let row_width = usize::from(width.saturating_sub(3));
    let choice_row = |choice: FinishReviewChoice, label: &str, detail: &str| {
        let marker = if choice == selected { "›" } else { " " };
        let head = format!("{marker} {label}");
        let detail = clip_terminal_cells_ellipsis(
            &format!(" — {detail}"),
            row_width.saturating_sub(head.width() + crate::widgets::SELECTION_FRAME),
        );
        // The cursor row carries the rail's selection highlight. Tinting it accent alone made the
        // chosen option read as the *dimmest* row against its bright neighbors.
        crate::widgets::selection_row(
            vec![
                Span::styled(head, app.theme_styles.text),
                Span::styled(detail, app.theme_styles.muted),
            ],
            row_width,
            choice == selected,
            &app.theme_styles,
        )
    };
    let mut lines = Vec::new();
    lines.extend([
        choice_row(FinishReviewChoice::Accept, "Accept", "mark it reviewed; the agent isn't told"),
        choice_row(
            FinishReviewChoice::AcceptAndCommit,
            "Accept and commit",
            "accept, then commit each chapter",
        ),
        choice_row(
            FinishReviewChoice::RequestFixes,
            "Request fixes",
            "ask the agent to address them",
        ),
        choice_row(
            FinishReviewChoice::CommentOnly,
            "Comment only",
            "send them without requesting edits",
        ),
    ]);
    if app.guided_review_workspace.finish_offers_answers {
        lines.push(choice_row(
            FinishReviewChoice::AnswerOnGitHub,
            "Answer on GitHub",
            "reply to and resolve its threads",
        ));
    }
    if app.guided_review_workspace.finish_offers_opening {
        lines.push(choice_row(
            FinishReviewChoice::OpenPullRequest,
            "Open pull request",
            "commit, push, and open it on GitHub",
        ));
    }
    if app.guided_review_workspace.finish_offers_stack {
        lines.push(choice_row(
            FinishReviewChoice::OpenStack,
            "Open as a stack",
            "one pull request per chapter, each on the last",
        ));
    }
    lines.push(choice_row(
        FinishReviewChoice::Cancel,
        "Keep reviewing",
        "nothing is sent; comments stay",
    ));
    lines
}

/// What finishing sends: every comment not yet with the agent, and again any it has but has not
/// answered, which the guide's count of unsent leaves out.
pub(super) fn finish_carries(comments: usize, again: usize) -> String {
    let noun = if comments == 1 { "comment" } else { "comments" };
    match (comments, again) {
        (0, _) => "No comments to send — add one with c.".to_owned(),
        (_, 0) => format!("Sends {comments} {noun}."),
        (_, again) if again == comments => format!("Sends {comments} {noun} again."),
        _ => format!("Sends {comments} {noun}, {again} of them again."),
    }
}

/// The commit preview: the exact subjects and bodies about to enter the log.
///
/// Chapters are already a hunk-exact partition of the change, so committing them is mechanical —
/// but writing history is irreversible enough that the messages get read first.
pub(in crate::view::guided_review) fn render_commit_preview(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    preview: &crate::guided_review::CommitPreview,
) {
    use luxe_core::guided_review::{chapter_commits, squashed_commit};
    let Some(review) = app.guided_review.as_ref() else {
        return;
    };
    let rendered = if preview.squash {
        squashed_commit(review, &[]).map(|commit| vec![commit])
    } else {
        chapter_commits(review, &[])
    };
    let width = area.width.saturating_sub(4).clamp(48, 92);
    let row_width = usize::from(width.saturating_sub(4));
    let mut lines = Vec::new();
    match rendered {
        Ok(commits) => {
            lines.push(Line::from(Span::styled(
                match commits.len() {
                    _ if preview.squash => "One commit for the whole review".to_owned(),
                    1 => "One commit, for the review's only chapter".to_owned(),
                    count => format!("{count} commits, one per chapter"),
                },
                app.theme_styles.accent,
            )));
            lines.push(Line::from(Span::styled(
                "Staged from the reviewed revision. Never pushes, never amends.",
                app.theme_styles.muted,
            )));
            lines.push(Line::from(""));
            for commit in &commits {
                lines.push(Line::from(Span::styled(
                    clip_terminal_cells_ellipsis(&commit.subject, row_width),
                    app.theme_styles.text,
                )));
                // The rationale is the reason the chapter exists, and it becomes the commit body.
                if let Some(body) = commit.message.lines().nth(2) {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", clip_terminal_cells_ellipsis(body, row_width)),
                        app.theme_styles.muted,
                    )));
                }
                for skipped in &commit.skipped {
                    lines.push(Line::from(Span::styled(
                        format!("  ! {skipped} cannot be committed from the review"),
                        app.theme_styles.warning,
                    )));
                }
            }
        }
        Err(error) => {
            lines.push(Line::from(Span::styled(error.to_string(), app.theme_styles.error)))
        }
    }
    if preview.running {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Committing…", app.theme_styles.accent)));
    }
    let footer = if preview.squash {
        " enter commit · s one per chapter · esc cancel "
    } else {
        " enter commit · s squash into one · esc cancel "
    };
    let block = overlay_block(app, Icon::Finish.glyph(), "Commit the review");
    // Size to the commit plan actually being previewed — a fixed box left a large empty well under
    // a two-line message, which reads as a rendering fault rather than a deliberate frame.
    let body = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = u16::try_from(body.clone().block(block.clone()).line_count(width))
        .unwrap_or(u16::MAX)
        .saturating_add(crate::widgets::FOOTER_ROWS)
        .clamp(7, area.height.saturating_sub(4).max(7));
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let inner = crate::widgets::render_footed(frame, block, popup, footer, None, &app.theme_styles);
    frame.render_widget(body, inner);
}
