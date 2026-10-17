//! Finding dialogs — a finding's details, and the composer that drafts one.

use super::super::context::submission_dispatch_label;
use super::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

pub(in crate::view::guided_review) fn render_finding_details(
    frame: &mut Frame,
    area: Rect,
    app: &App,
) {
    let Some((finding, revision, submission_dispatch)) = selected_finding_snapshot(app) else {
        // nothing to draw, so nothing is hidden: with no selection its keys do nothing anyway
        app.guided_review_workspace.settle_preview(true);
        return;
    };
    let width = area.width.saturating_sub(4).clamp(36, 72).min(area.width);
    let height = area.height.saturating_sub(4).min(18);
    let Some(popup) = crate::widgets::fit_review_preview(frame, area, (width, height, 10), app)
    else {
        return;
    };
    let mut lines = finding_details_rows(app, finding, revision, submission_dispatch);
    if !finding.thread.is_empty() {
        lines.extend(finding_thread_rows(app, finding, width));
    }
    let footer = match finding.state {
        luxe_core::guided_review::FindingState::Draft => " e edit · x delete · esc close ",
        luxe_core::guided_review::FindingState::Stale => " alt+r re-anchor and verify · esc close ",
        luxe_core::guided_review::FindingState::Resolved => " r reopen · esc close ",
        luxe_core::guided_review::FindingState::Open => " r resolve · esc close ",
        luxe_core::guided_review::FindingState::NeedsVerification => {
            " r verify and resolve · esc close "
        }
    };
    let block = overlay_block(app, Icon::Evidence.glyph(), "The exchange");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(frame, block, popup, footer, None, &app.theme_styles);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), body);
}

/// The selected finding, the revision its session pinned, and how the last submission carrying it
/// was dispatched.
fn selected_finding_snapshot(
    app: &App,
) -> Option<(
    &luxe_core::guided_review::ReviewFinding,
    &str,
    Option<luxe_core::guided_review::SubmissionDispatch>,
)> {
    app.review.session().and_then(|session| {
        app.guided_review_workspace
            .selected_finding_id
            .as_ref()
            .and_then(|id| session.findings.iter().find(|finding| &finding.id == id))
            .map(|finding| {
                let submission_dispatch = session
                    .submissions
                    .iter()
                    .rev()
                    .find(|submission| {
                        submission.findings.iter().any(|snapshot| snapshot.finding_id == finding.id)
                    })
                    .map(|submission| submission.dispatch);
                (finding, session.source.content_digest.as_str(), submission_dispatch)
            })
    })
}

/// A finding's title, state, anchor, the revision it is against, its words and its suggested fix.
fn finding_details_rows<'a>(
    app: &App,
    finding: &'a luxe_core::guided_review::ReviewFinding,
    revision: &str,
    submission_dispatch: Option<luxe_core::guided_review::SubmissionDispatch>,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(&finding.title, app.theme_styles.accent)),
        Line::from(Span::styled(
            match finding.severity {
                luxe_core::guided_review::FindingSeverity::Question => {
                    format!("question · {}", finding_state_label(finding))
                }
                _ => finding_state_label(finding).to_owned(),
            },
            app.theme_styles.muted,
        )),
        Line::from(Span::styled(anchor_summary(app, &finding.anchor), app.theme_styles.text)),
        Line::from(vec![
            Span::styled(
                submission_dispatch.map_or_else(
                    || "Pinned revision: ".to_owned(),
                    |dispatch| {
                        format!("Submitted snapshot ({}): ", submission_dispatch_label(dispatch))
                    },
                ),
                app.theme_styles.muted,
            ),
            Span::styled(clip_terminal_cells(revision, 0, 16), app.theme_styles.text),
        ]),
        Line::from(""),
        Line::from(finding.body.as_str()),
    ];
    if let Some(proposed_fix) = finding.proposed_fix.as_deref() {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("Suggested fix", app.theme_styles.muted)),
            Line::from(proposed_fix),
        ]);
    }
    lines
}

/// The exchange in order, attributed, each message cut to two lines.
///
/// Showing only the agent's latest note hid the reviewer's own replies — the half that makes this
/// a conversation rather than a verdict.
fn finding_thread_rows(
    app: &App,
    finding: &luxe_core::guided_review::ReviewFinding,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Thread — an agent report is a claim; reviewer verification required",
            app.theme_styles.muted,
        )),
    ];
    let evidence_width = usize::from(width.saturating_sub(7));
    for entry in &finding.thread {
        let style = match entry.author {
            luxe_core::guided_review::FindingMessageAuthor::Reviewer => app.theme_styles.text,
            _ => app.theme_styles.info,
        };
        lines.push(Line::from(Span::styled(
            format!("{}:", entry.author.label()),
            app.theme_styles.muted,
        )));
        let mut body = entry.body.lines();
        for line in body.by_ref().take(2) {
            lines.push(Line::from(Span::styled(
                clip_terminal_cells(line, 0, evidence_width),
                style,
            )));
        }
        if body.next().is_some() {
            lines.push(Line::from(Span::styled("  …", app.theme_styles.muted)));
        }
    }
    lines
}

pub(in crate::view::guided_review) fn finding_state_glyph(
    finding: &luxe_core::guided_review::ReviewFinding,
) -> &'static str {
    use luxe_core::guided_review::{FindingSeverity, FindingState};

    match finding.state {
        FindingState::Resolved => crate::glyphs::Mark::Yes.glyph(),
        FindingState::Stale => "~",
        FindingState::NeedsVerification => "?",
        FindingState::Draft => "○",
        FindingState::Open => match finding.severity {
            FindingSeverity::Question => "?",
            _ => "!",
        },
    }
}

pub(in crate::view::guided_review) fn finding_state_label(
    finding: &luxe_core::guided_review::ReviewFinding,
) -> &'static str {
    use luxe_core::guided_review::FindingState;

    if finding.github_thread.as_ref().is_some_and(|thread| thread.resolved) {
        return "resolved on GitHub";
    }
    match finding.state {
        FindingState::Draft | FindingState::Open if finding.posted.is_some() => "on GitHub",
        FindingState::Draft => "draft",
        FindingState::Open => "open",
        FindingState::NeedsVerification => "to verify",
        FindingState::Resolved => "verified",
        FindingState::Stale => "stale",
    }
}

pub(in crate::view::guided_review) fn render_finding_draft(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    draft: &crate::guided_review::FindingDraft,
) {
    let anchor = anchor_summary(app, &draft.anchor);
    let editing_draft = app.guided_review_workspace.editing_finding_id.is_some();
    let (asking, fixing) = finding_draft_intent(app, draft);
    // The location, unlabeled. "Anchor:" named the mechanism holding the comment in place, which
    // is the reviewer's least pressing question about a box that just opened on the line they were
    // already looking at.
    let head = vec![
        Line::from(Span::styled(anchor, app.theme_styles.muted)),
        Line::from(""),
        Line::from(Span::styled(
            if asking { "Question:" } else { "Comment:" },
            app.theme_styles.muted,
        )),
    ];
    let mut tail = Vec::new();
    if fixing {
        tail.push(Line::from(Span::styled(
            "Sends the question, its answer and this to the agent as a fix request.",
            app.theme_styles.muted,
        )));
    }
    if let Some(error) = draft.error.as_deref() {
        tail.push(Line::from(Span::styled(error.to_string(), app.theme_styles.error)));
    }
    if editing_draft {
        tail.push(Line::from(Span::styled(
            "Draft edit only — submitted feedback stays immutable.",
            app.theme_styles.muted,
        )));
    }
    // Sized after the rows exist, not before: a fixed box left half of itself empty under a short
    // comment. The field grows with what is written, up to a point, and scrolls past it.
    let width = area.width.min(88);
    let inner_width = width.saturating_sub(3).max(1);
    let field_rows = crate::widgets::text_field::rows(draft.editor(), inner_width).clamp(3, 12);
    let head_rows = crate::widgets::text_field::wrapped_rows(&head, inner_width);
    let tail_rows = crate::widgets::text_field::wrapped_rows(&tail, inner_width);
    let height =
        (head_rows + field_rows + tail_rows + 2 + crate::widgets::FOOTER_ROWS).min(area.height);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let (title, hint) = finding_draft_chrome(draft, editing_draft, asking, fixing);
    let block = overlay_block(app, Icon::Compose.glyph(), title);
    frame.render_widget(Clear, popup);
    let inner = crate::widgets::render_footed(frame, block, popup, hint, None, &app.theme_styles);
    let [head_area, field_area, tail_area] = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(head_rows),
        ratatui::layout::Constraint::Min(1),
        ratatui::layout::Constraint::Length(tail_rows),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: true }), head_area);
    app.guided_review_workspace.field_width.set(field_area.width);
    crate::widgets::text_field::render(frame, field_area, draft.editor(), &app.theme_styles);
    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: true }), tail_area);
}

/// Whether the draft asks the agent a question, and whether, as a comment on a question's thread,
/// it asks for the fix.
fn finding_draft_intent(app: &App, draft: &crate::guided_review::FindingDraft) -> (bool, bool) {
    // No severity is on screen, so the title and the field label alone say which verb opened the
    // box — and they have to, because a question and a comment do different things to the agent
    // and leave at different times.
    let asking = draft.severity == luxe_core::guided_review::FindingSeverity::Question;
    // A comment on a question's thread asks for the fix, and takes the question and its answer
    let fixing = !asking
        && draft.reply_to.as_ref().is_some_and(|id| {
            app.review.session().is_some_and(|session| {
                session.findings.iter().any(|finding| {
                    &finding.id == id
                        && finding.severity == luxe_core::guided_review::FindingSeverity::Question
                })
            })
        });
    (asking, fixing)
}

/// The composer's title and its footer's keys, named for the verb that opened it.
fn finding_draft_chrome(
    draft: &crate::guided_review::FindingDraft,
    editing_draft: bool,
    asking: bool,
    fixing: bool,
) -> (&'static str, &'static str) {
    // A comment on the review as a whole is what a posted review carries as its summary
    let summary = draft.anchor == luxe_core::guided_review::FindingAnchor::Review;
    let title = match (editing_draft, asking, summary) {
        _ if fixing => "Ask for a fix",
        (true, _, _) => "Edit comment",
        (false, true, _) => "Ask the agent about this",
        (false, false, true) => "Review summary",
        (false, false, false) => "Comment on this",
    };
    // A question leaves the moment it is saved; a comment waits to be sent. That is the one thing
    // about the two verbs a reviewer cannot infer from the box in front of them.
    let hint = match (asking, fixing) {
        (true, _) => "ctrl+enter ask the agent · esc cancel",
        (false, true) => "ctrl+enter send now · esc cancel",
        (false, false) => "ctrl+enter save · esc cancel",
    };
    (title, hint)
}

/// A reviewer-facing anchor summary: `src/event.rs:143 (new)` rather than an opaque ID pair.
///
/// Always names the file: "new line 143" is unusable once a review spans more than one path.
pub(in crate::view::guided_review) fn anchor_summary(
    app: &App,
    anchor: &luxe_core::guided_review::FindingAnchor,
) -> String {
    use luxe_core::guided_review::{DiffSide, FindingAnchor};
    let path_of = |file_id: &luxe_core::guided_review::FileId| {
        app.guided_review
            .as_ref()
            .and_then(|review| review.files.iter().find(|file| &file.id == file_id))
            .and_then(|file| file.new_path.as_deref().or(file.old_path.as_deref()))
            .map(|path| path.display().to_string())
    };
    match anchor {
        FindingAnchor::Line { file_id, side, line, .. } => {
            let side = match side {
                DiffSide::Old => "old",
                DiffSide::New => "new",
            };
            match path_of(file_id) {
                Some(path) => format!("{path}:{line} ({side})"),
                None => format!("{side} line {line}"),
            }
        }
        FindingAnchor::Hunk { file_id, hunk_id } => {
            let heading = app
                .guided_review
                .as_ref()
                .and_then(|review| {
                    review
                        .files
                        .iter()
                        .flat_map(|file| &file.hunks)
                        .find(|hunk| &hunk.id == hunk_id)
                })
                .and_then(|hunk| hunk.heading.clone());
            match (path_of(file_id), heading) {
                (Some(path), Some(heading)) => format!("{path} · {heading}"),
                (Some(path), None) => format!("{path} · selected hunk"),
                (None, _) => "selected hunk".into(),
            }
        }
        FindingAnchor::Chapter { chapter_id } => app
            .guided_review
            .as_ref()
            .and_then(|review| review.chapters.iter().find(|chapter| &chapter.id == chapter_id))
            .map_or_else(
                || "current chapter".into(),
                |chapter| format!("chapter · {}", chapter.title),
            ),
        FindingAnchor::Review => "entire review".into(),
    }
}

/// The chapter's presentation priority, so supporting and generated sections are labeled rather
/// than looking like more core work.
pub(in crate::view::guided_review) fn chapter_kind_label(
    kind: luxe_core::guided_review::ChapterKind,
) -> &'static str {
    use luxe_core::guided_review::ChapterKind;
    match kind {
        ChapterKind::Core => "core",
        ChapterKind::Consequence => "consequence",
        ChapterKind::Test => "tests",
        ChapterKind::Supporting => "supporting",
        ChapterKind::Generated => "generated",
    }
}
