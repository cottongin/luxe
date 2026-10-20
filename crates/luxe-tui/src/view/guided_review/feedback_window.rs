//! The feedback window — every comment in a list, and the one under the cursor read in full.
//!
//! One window for two jobs. `Alt+C` opens it to work through feedback: go to a comment, reply,
//! verify, edit. `u` there turns it to choosing what goes to the agent, each row with a checkbox. A
//! comment is often long and often markdown, especially one an agent or a reviewer on GitHub
//! wrote, so the list keeps to a line a comment and the pane beside it draws the whole thing.

use super::overlays::{anchor_summary, finding_state_glyph, finding_state_label, overlay_block};
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use crate::mouse::MouseTarget;
use luxe_core::guided_review::{FindingMessageAuthor, FindingSeverity, ReviewFinding};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// What the window is open for.
pub(super) enum FeedbackMode<'a> {
    /// Working through feedback
    Manage,
    /// Choosing what goes to the agent
    Send(&'a crate::guided_review::SendPreview),
}

/// What the send window says above the list; the manage window has no header.
fn header_lines(
    mode: &FeedbackMode<'_>,
    findings: &[&ReviewFinding],
    styles: &crate::theme::styles::ThemeStyles,
) -> Vec<Line<'static>> {
    match mode {
        FeedbackMode::Manage => Vec::new(),
        FeedbackMode::Send(send) => {
            let included =
                findings.iter().filter(|finding| !send.excluded.contains(&finding.id)).count();
            vec![
                Line::from(Span::styled(
                    match send.outcome {
                        Some(_) => "Finish the review by sending these comments to the agent",
                        None => "Send these comments to the agent",
                    },
                    styles.accent,
                )),
                Line::from(Span::styled(
                    format!("{included} of {} included", findings.len()),
                    styles.muted,
                )),
                Line::from(""),
            ]
        }
    }
}

/// Draws the reading pane and its scrollbar, and returns how far past its height the text runs.
fn render_preview(
    frame: &mut Frame,
    app: &App,
    preview: Vec<Line<'static>>,
    preview_rows: u16,
    preview_area: Rect,
) -> u16 {
    let styles = &app.theme_styles;
    let room = preview_rows.saturating_sub(preview_area.height);
    app.guided_review_workspace.feedback_room.set(room);
    let scroll = app.guided_review_workspace.feedback_scroll.min(room);
    let text_area = Rect { width: preview_area.width.saturating_sub(1), ..preview_area };
    frame.render_widget(
        Paragraph::new(preview).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        text_area,
    );
    crate::widgets::steered_scrollbar(
        frame,
        &mut app.hit.borrow_mut(),
        preview_area,
        crate::mouse::ScrollRegionId::ReviewFeedback,
        usize::from(scroll),
        usize::from(preview_rows),
        usize::from(preview_area.height),
        styles,
    );
    room
}

/// Draws the window.
pub(super) fn render_feedback_window(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    mode: FeedbackMode<'_>,
) {
    let styles = &app.theme_styles;
    let (findings, selected) = rows(app, &mode);
    if findings.is_empty() {
        render_empty(frame, area, app);
        return;
    }
    let width = area.width.saturating_sub(6).clamp(60, 150).min(area.width);
    let list_width = (width * 2 / 5).clamp(28, 52);
    // What the reading pane's text wraps at: the frame and its pad, the list, the divider, and the
    // scrollbar's column. Counted wider, the last rows of a long comment could never be reached
    let preview_width = width.saturating_sub(3 + list_width + 3 + 1).max(1);
    let finding = findings.get(selected).copied();
    let preview = finding.map_or_else(Vec::new, |finding| preview_lines(app, finding));
    let preview_rows = crate::widgets::text_field::wrapped_rows(&preview, preview_width);
    let header = header_lines(&mode, &findings, styles);
    let header_rows = u16::try_from(header.len()).unwrap_or(0);
    let list_rows = u16::try_from(findings.len()).unwrap_or(u16::MAX);
    let height =
        (header_rows + list_rows.max(preview_rows).max(8) + 2 + crate::widgets::FOOTER_ROWS)
            .min(area.height.saturating_sub(2))
            .min(area.height);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let (title, glyph) = match &mode {
        FeedbackMode::Manage => ("Feedback", Icon::Findings.glyph()),
        FeedbackMode::Send(_) => ("Send to agent", Icon::Finish.glyph()),
    };
    let block = overlay_block(app, glyph, title);
    let inner = crate::widgets::footed_body(&block, popup);
    frame.render_widget(Clear, popup);

    let [header_area, body] = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(header_rows),
        ratatui::layout::Constraint::Min(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(header), header_area);
    let [list_area, divider, preview_area] = ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Length(list_width),
        ratatui::layout::Constraint::Length(3),
        ratatui::layout::Constraint::Min(1),
    ])
    .areas(body);

    // The list scrolls to keep the cursor in it
    let offset = selected.saturating_sub(usize::from(list_area.height.saturating_sub(1)));
    let row_width = usize::from(list_area.width);
    let mut regions = app.hit.borrow_mut();
    let lines: Vec<Line<'static>> = findings
        .iter()
        .enumerate()
        .skip(offset)
        .take(usize::from(list_area.height))
        .map(|(index, finding)| {
            if matches!(mode, FeedbackMode::Manage) {
                let y = list_area.y + u16::try_from(index - offset).unwrap_or(0);
                regions.push(
                    Rect::new(list_area.x, y, list_area.width, 1),
                    MouseTarget::ReviewFindingRow(finding.id.clone()),
                );
            }
            list_row(app, &mode, finding, index == selected, row_width)
        })
        .collect();
    drop(regions);
    // `trim: false`: the rows arrive laid out, and trimming ate the frame off unselected ones
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), list_area);
    let rule: Vec<Line<'static>> =
        (0..divider.height).map(|_| Line::from(Span::styled(" │ ", styles.border_muted))).collect();
    frame.render_widget(Paragraph::new(rule), divider);

    let room = render_preview(frame, app, preview, preview_rows, preview_area);

    let scrolls = if room > 0 { " · pgup/pgdn scroll" } else { "" };
    let hint = match mode {
        FeedbackMode::Manage => {
            // The preview it opens lists what goes, so the hint needs no count to disagree with
            // the guide's, which leaves out what the agent already has
            let sends = if crate::app::window_sends(app).is_some() { " · u send" } else { "" };
            format!(" {}{sends}{scrolls} · esc close ", manage_keys(app))
        }
        FeedbackMode::Send(_) => {
            format!(" enter send · space include · ↑/↓ choose{scrolls} · esc cancel ")
        }
    };
    crate::widgets::render_footed(frame, block, popup, &hint, None, styles);
}

/// The comments the window lists, and which of them is under the cursor.
fn rows<'a>(app: &'a App, mode: &FeedbackMode<'_>) -> (Vec<&'a ReviewFinding>, usize) {
    let Some(session) = app.review.session() else {
        return (Vec::new(), 0);
    };
    match mode {
        FeedbackMode::Manage => {
            let findings = crate::guided_review::ordered_findings(&session.findings);
            let selected = app
                .guided_review_workspace
                .selected_finding_id
                .as_ref()
                .and_then(|id| findings.iter().position(|finding| &finding.id == id))
                .unwrap_or(0);
            (findings, selected)
        }
        FeedbackMode::Send(send) => {
            let ids = crate::app::input::unsent_finding_ids(app);
            let findings: Vec<&ReviewFinding> =
                session.findings.iter().filter(|finding| ids.contains(&finding.id)).collect();
            let selected = send.selected.min(findings.len().saturating_sub(1));
            (findings, selected)
        }
    }
}

/// One comment in the list: its mark, its label, its title, and where it stands.
fn list_row(
    app: &App,
    mode: &FeedbackMode<'_>,
    finding: &ReviewFinding,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let styles = &app.theme_styles;
    let (mark, state, dim) = match mode {
        FeedbackMode::Manage => (finding_state_glyph(finding), finding_state_label(finding), false),
        FeedbackMode::Send(send) => {
            let excluded = send.excluded.contains(&finding.id);
            let mark = if excluded { "○" } else { crate::glyphs::Mark::Yes.glyph() };
            let kind = if finding.severity == FindingSeverity::Question { "question" } else { "" };
            (mark, kind, excluded)
        }
    };
    let state = if state.is_empty() { String::new() } else { format!("  {state} ") };
    let title_width = width.saturating_sub(3 + state.width() + crate::widgets::SELECTION_FRAME);
    let text = if dim { styles.dim } else { styles.text };
    crate::widgets::selection_row(
        vec![
            Span::styled(if selected { "›" } else { " " }, styles.accent),
            Span::styled(mark, if dim { styles.dim } else { styles.accent }),
            Span::styled(" ", text),
            Span::styled(clip_terminal_cells_ellipsis(&plain(&finding.title), title_width), text),
            Span::styled(state, styles.muted),
        ],
        width,
        selected,
        styles,
    )
}

/// A title as the reader sees it drawn: a comment's first line keeps its markdown, and `**`
/// around a word reads as noise in a list.
pub(super) fn plain(title: &str) -> String {
    title.replace("**", "").replace("__", "").replace('`', "")
}

/// The comment in full: what kind it is and where it stands, where it is, what it says, and what
/// has been said about it since.
fn preview_lines(app: &App, finding: &ReviewFinding) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let kind = match finding.severity {
        FindingSeverity::Question => "Question",
        _ if finding.github_thread.is_some() => "Reviewer comment",
        _ => "Comment",
    };
    let head = Span::styled(format!("{kind} · {}", finding_state_label(finding)), styles.accent);
    let mut lines = vec![Line::from(head)];
    let mut place = anchor_summary(app, &finding.anchor);
    if let Some(thread) = finding.github_thread.as_ref() {
        let author = if thread.author.is_empty() { "a deleted account" } else { &thread.author };
        place = format!("{place} · from {author} on GitHub");
    }
    lines.push(Line::from(Span::styled(place, styles.muted)));
    lines.push(Line::from(""));
    lines.extend(crate::markdown::render_markdown(&finding.body, styles));
    if let Some(fix) = finding.proposed_fix.as_deref().filter(|fix| !fix.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Proposed fix", styles.muted)));
        lines.extend(crate::markdown::render_markdown(fix, styles));
    }
    for message in &finding.thread {
        lines.push(Line::from(""));
        let who = match message.author {
            FindingMessageAuthor::Reviewer => "You",
            FindingMessageAuthor::Agent => "The agent",
            FindingMessageAuthor::System => "luxe",
        };
        lines.push(Line::from(Span::styled(who, styles.muted)));
        lines.extend(crate::markdown::render_markdown(&message.body, styles));
    }
    lines
}

/// What the keys do to the comment under the cursor, which depends on where it stands.
fn manage_keys(app: &App) -> &'static str {
    use luxe_core::guided_review::FindingState;
    let selected = app.review.session().and_then(|session| {
        app.guided_review_workspace
            .selected_finding_id
            .as_ref()
            .and_then(|id| session.findings.iter().find(|finding| &finding.id == id))
    });
    if crate::app::input::retryable_submission_for_selected_finding(app).is_some() {
        return "enter go to it · r retry sending";
    }
    if crate::app::input::queued_submission_for_selected_finding(app).is_some() {
        return "enter go to it · r send now · x cancel sending";
    }
    let question = selected
        .filter(|finding| finding.severity == luxe_core::guided_review::FindingSeverity::Question);
    match (question, selected.map(|finding| (finding.state, finding.posted.is_some()))) {
        (Some(finding), _) => {
            let resolved = finding.state == FindingState::Resolved;
            match (crate::app::reply_asks_for_fix(app, finding), resolved) {
                (true, false) => "enter go to it · c ask for a fix · a follow up · r resolve",
                (true, true) => "enter go to it · c ask for a fix · a follow up · r reopen",
                (false, false) => "enter go to it · a follow up · r resolve",
                (false, true) => "enter go to it · a follow up · r reopen",
            }
        }
        _ => manage_comment_keys(selected.map(|finding| (finding.state, finding.posted.is_some()))),
    }
}

/// The keys for a comment, rather than a question, by where it stands.
fn manage_comment_keys(
    selected: Option<(luxe_core::guided_review::FindingState, bool)>,
) -> &'static str {
    use luxe_core::guided_review::FindingState;
    match selected {
        Some((FindingState::Draft, true)) => "enter go to it · o open on GitHub",
        Some((FindingState::Draft, false)) => "enter go to it · e edit · x delete",
        Some((FindingState::Stale, _)) => "enter go to it · alt+r re-anchor and verify",
        Some((FindingState::Resolved, _)) => "enter go to it · r reopen · c reply",
        Some((FindingState::Open, _)) => "enter go to it · r resolve · c reply",
        Some((FindingState::NeedsVerification, _)) => {
            "enter go to it · v what changed · r verify · c reply"
        }
        None => "↑/↓ choose",
    }
}

/// No feedback yet: say how to leave some.
fn render_empty(frame: &mut Frame, area: Rect, app: &App) {
    let styles = &app.theme_styles;
    let lines = vec![
        Line::from(Span::styled("No feedback yet", styles.text)),
        Line::from(""),
        Line::from(Span::styled("Put the cursor on a line, hunk or chapter:", styles.muted)),
        Line::from(Span::styled("c comments on it · a asks the agent about it", styles.muted)),
        Line::from(Span::styled("Either one stays anchored to this revision.", styles.muted)),
    ];
    let width = area.width.saturating_sub(4).clamp(32, 56).min(area.width);
    let height = (7 + crate::widgets::FOOTER_ROWS).min(area.height);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let block = overlay_block(app, Icon::Findings.glyph(), "Feedback");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(frame, block, popup, "esc close", None, styles);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests;
