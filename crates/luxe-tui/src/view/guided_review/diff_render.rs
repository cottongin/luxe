//! Guided Review diff rendering — unified/split hunks, gutters, finding markers,
//! and syntax-highlighted source spans.

mod annotations;
mod source;
mod split;
mod unified;

use crate::app::App;
use crate::guided_review::{visible_groups, VisibleGroup};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

#[cfg(test)]
pub(super) use annotations::comment_lines;
pub(super) use annotations::{display_author, is_feedback, thread_is_feedback};
pub(super) use unified::render_first_file_diff;

/// Renders the diff content pane: a left/top pad, a sticky `file › hunk` breadcrumb, then the
/// diff itself. The breadcrumb reinforces which file/hunk is shown as the reviewer moves, and the
/// left pad lifts the diff off the sidebar divider.
pub(super) fn render_content(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    narrate: bool,
) {
    // 1-cell top + left padding for breathing room off the divider and the screen top.
    let area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(1),
        height: area.height.saturating_sub(1),
    };
    if area.width == 0 || area.height == 0 {
        return;
    }
    // The narration comes first and full width: the reviewer is here to understand the change, and
    // the diff below is the evidence for what the prose just said.
    // Only the wide tier narrates here: the stacked tiers already carry the section prose in their
    // own context block, and saying it twice would cost the diff rows for nothing.
    let narration = narrate
        .then(|| super::narration::narration_lines(app, review, area.width.saturating_sub(1)))
        .flatten();
    let narration_height = narration.as_ref().map_or(0, |lines| {
        // Never let the story crowd out the code it describes.
        (lines.len() as u16 + 1).min(area.height / 2)
    });
    let area = if let (Some(lines), true) = (narration, narration_height > 0) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(narration_height), Constraint::Min(1)])
            .split(area);
        frame.render_widget(Paragraph::new(lines), split[0]);
        // A thin rule marks where the story ends and its evidence begins.
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(usize::from(split[0].width)),
                app.theme_styles.border_muted,
            ))),
            Rect { y: split[0].bottom().saturating_sub(1), height: 1, ..split[0] },
        );
        split[1]
    } else {
        area
    };
    // A blank spacer, then the diff. No breadcrumb: the narration above and the guide's foot
    // already say the path, the symbol and the step, and none of them is worth two rows to repeat.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    // Reserve the last column for the scrollbar so a long file says where you are in it; without
    // one the pane gave no sense of position or of how much was left.
    let body = rows[1];
    let (diff_area, scrollbar_area) = if body.width > 2 {
        (
            Rect { width: body.width.saturating_sub(1), ..body },
            Some(Rect { x: body.right().saturating_sub(1), width: 1, ..body }),
        )
    } else {
        (body, None)
    };
    render_first_file_diff(frame, diff_area, app, review);
    if let Some(scrollbar_area) = scrollbar_area {
        render_diff_scrollbar(frame, scrollbar_area, app, review);
    }
}

/// A proportional scrollbar for the selected file's visible rows.
fn render_diff_scrollbar(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    let total = visible_row_total(app, review);
    crate::widgets::steered_scrollbar(
        frame,
        &mut app.hit.borrow_mut(),
        area,
        crate::mouse::ScrollRegionId::ReviewDiff,
        app.guided_review_workspace.position.line_offset.min(total.saturating_sub(1)),
        total,
        usize::from(area.height),
        &app.theme_styles,
    );
}

/// The file the diff pane is showing: the selection, else the first item worth landing on.
pub(super) fn shown_file<'a>(
    app: &App,
    review: &'a luxe_core::guided_review::GuidedReview,
) -> Option<&'a luxe_core::guided_review::ReviewFile> {
    app.guided_review_workspace
        .position
        .file_id
        .as_ref()
        .and_then(|selected| review.files.iter().find(|file| &file.id == selected))
        .or_else(|| {
            review
                .files
                .iter()
                .find(|file| (file.is_text() && !file.hunks.is_empty()) || file.non_text.is_some())
        })
}

/// Total visible rows in the selected file, matching the renderer's row space.
fn visible_row_total(app: &App, review: &luxe_core::guided_review::GuidedReview) -> usize {
    shown_file(app, review)
        .map(|file| {
            visible_groups(file, &app.guided_review_workspace.fold)
                .iter()
                .map(VisibleGroup::rows)
                .sum()
        })
        .unwrap_or_default()
}

/// Appends the end-of-reading marker under the rows when the pane has room for it, so the last
/// screen of a file is not indistinguishable from a scroll that stopped working.
///
/// Args: `(lines, app, review, (groups, shown), height)` — the visible groups and how many rows
/// from the offset the pane drew.
fn append_end_of_reading(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    (groups, shown): (&[VisibleGroup<'_>], usize),
    height: u16,
) {
    let total_rows: usize = groups.iter().map(VisibleGroup::rows).sum();
    let at_file_end =
        app.guided_review_workspace.position.line_offset.saturating_add(shown) >= total_rows;
    if (lines.len() as u16) < height {
        if let Some(marker) = end_of_reading_marker(app, review, at_file_end) {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(marker, app.theme_styles.muted)));
        }
    }
}

/// The marker under the file's last row: where the reading stops, and what carries it on.
///
/// The pane draws one file at a time, so "these rows ran out" and "there is nothing more to read"
/// look identical from the bottom of a scroll. Naming which one it is, and the key past it, is what
/// lets scrolling clamp at the edge without reading as a stuck scrollbar.
///
/// At a chapter's end it also reports whether the chapter counted as reviewed, because that is the
/// one place the distinction shows: stepping here with `Space` records the chapter, scrolling here
/// does not, and nothing else on screen says which of the two just happened.
fn end_of_reading_marker(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    at_file_end: bool,
) -> Option<String> {
    let workspace = &app.guided_review_workspace;
    if !at_file_end {
        return None;
    }
    if !workspace.at_scope_end(review) {
        return Some("— end of file —".into());
    }
    let index = review
        .chapters
        .iter()
        .position(|chapter| workspace.position.chapter_id.as_ref() == Some(&chapter.id))?;
    let reviewed = workspace.is_chapter_walked(&review.chapters[index].id);
    let ending = if reviewed { "chapter reviewed" } else { "end of chapter" };
    // The end of the reading is where the reader asks what comes next
    let finish = super::chord(app, luxe_core::keymap::UiAction::ReviewFinish, "alt+enter");
    Some(match review.chapters.get(index + 1) {
        Some(next) => format!("— {ending} · ] {} —", next.title),
        None if reviewed => format!("— last chapter reviewed · space or {finish} finishes —"),
        None => format!("— end of the last chapter · space or {finish} finishes —"),
    })
}

pub(super) fn render_non_text_diff(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    file: &luxe_core::guided_review::ReviewFile,
) {
    let mut lines = Vec::new();
    let Some(change) = file.non_text.as_ref() else {
        lines.push(Line::from(Span::styled("Non-text change", app.theme_styles.warning)));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    let kind = match change.kind {
        luxe_core::guided_review::NonTextChangeKind::Binary => "Binary change",
        luxe_core::guided_review::NonTextChangeKind::Submodule => "Submodule change",
        luxe_core::guided_review::NonTextChangeKind::ModeOnly => "File mode change",
        luxe_core::guided_review::NonTextChangeKind::Rename => "Rename-only change",
        luxe_core::guided_review::NonTextChangeKind::Copy => "Copy-only change",
    };
    lines.push(Line::from(Span::styled(kind, app.theme_styles.warning)));
    lines.push(Line::from(Span::styled(&change.summary, app.theme_styles.text)));
    lines.push(Line::from(Span::styled("space reviewed", app.theme_styles.muted)));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

#[cfg(test)]
mod chrome_tests {
    /// The workspace decides whether a column fits; this file lays the column out. They measure
    /// the pane's fixed costs separately — the model may not reach into the view for a constant —
    /// so the two numbers are held equal here rather than hoped to be.
    #[test]
    fn the_split_gate_measures_what_the_renderer_lays_out() {
        assert_eq!(
            crate::guided_review::SPLIT_PANE_CHROME,
            crate::view::diff_lines::SPLIT_DIVIDER_CELLS + super::source::ACTIVE_BAR_WIDTH,
        );
        assert_eq!(
            crate::guided_review::UNIFIED_ROW_CHROME,
            super::source::ACTIVE_BAR_WIDTH + super::unified::UNIFIED_NUMBER_CHROME,
        );
    }
}

#[cfg(test)]
mod reply_preview_tests {
    /// The card previews a reply's opening paragraph as it reads, not its markup.
    #[test]
    fn a_reply_previews_its_first_paragraph_as_text() {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("luxe").expect("luxe resolves");
        let styles = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "luxe");
        let reply =
            "**It's an off-by-one**, in `untrusted`.\nSecond line.\n\n```\ncode\n```\nMore.";
        assert_eq!(
            super::annotations::first_paragraph(reply, &styles),
            "It's an off-by-one, in untrusted. Second line."
        );
    }
}

#[cfg(test)]
mod split_feedback_tests {
    use crate::fixture::{seeded_app, FixtureScenario};
    use luxe_core::guided_review::FindingId;

    /// A split review with a comment on the row at unified `offset`, saying `body`.
    fn commented(offset: usize, body: &str) -> crate::app::App {
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewSplit).expect("fixture");
        let files = app.guided_review.as_ref().expect("review").files.clone();
        app.guided_review_workspace.position.line_offset = offset;
        app.guided_review_workspace.position.hunk_id = None;
        app.guided_review_workspace.begin_finding_draft(&files);
        let draft = app.guided_review_workspace.finding_draft.as_mut().expect("draft");
        draft.set_body(body);
        let finding = draft.to_review_finding(FindingId::new()).expect("finding");
        app.review.session_mut().expect("session").add_finding(finding).expect("added");
        app.guided_review_workspace.discard_finding_draft();
        app.guided_review_workspace.position.line_offset = 0;
        app
    }

    /// Whether `body` is drawn in the card right under the first row drawing `line`, in the diff:
    /// the card's top border, then its text.
    fn carded_under(drawn: &[String], line: &str, body: &str) -> bool {
        let y = drawn.iter().position(|row| row.contains(line)).expect("the line is drawn");
        drawn.get(y + 2).is_some_and(|row| row.contains(body))
    }

    /// The drawn rows of the review surface.
    fn rows(app: &mut crate::app::App, width: u16, height: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        app.update(crate::app::Msg::TerminalResized { width, height });
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("drawn");
        let buffer = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
    }

    /// Comment cards under a split hunk header take rows, and the clicks below them still land on
    /// the row drawn there.
    ///
    /// The break this catches: a header with cards recorded a height of one, so every click
    /// region below it sat a card's worth of rows too high.
    #[test]
    fn split_header_feedback_does_not_shift_later_clicks() {
        let mut app = commented(0, "About this whole hunk.");
        let drawn = rows(&mut app, 200, 40);
        assert!(carded_under(&drawn, "@@ -4,3", "About this whole hunk."), "{drawn:#?}");
        let y = drawn.iter().position(|row| row.contains("pub struct Notification {")).unwrap();
        let hit = app.hit.borrow();
        let at = drawn[y].find("pub struct").unwrap();
        let x = unicode_width::UnicodeWidthStr::width(&drawn[y][..at]) as u16;
        assert_eq!(
            hit.at(x, y as u16).cloned(),
            Some(crate::mouse::MouseTarget::ReviewDiffRow(1)),
            "the click on the first context row is that row"
        );
    }

    /// A context line sits on both sides of its split row, and its comment is still drawn once.
    #[test]
    fn a_context_lines_comment_is_drawn_once() {
        let mut app = commented(1, "Said once, not twice.");
        let drawn = rows(&mut app, 200, 40);
        let cards = drawn.iter().filter(|row| row.contains("│ Said once, not twice.")).count();
        assert_eq!(cards, 1, "{drawn:#?}");
    }

    /// Wrapping a split diff keeps the comments under their lines.
    #[test]
    fn wrapping_a_split_diff_keeps_its_feedback() {
        let mut app = commented(1, "Wrapped, and still here.");
        app.guided_review_workspace.word_wrap = true;
        let drawn = rows(&mut app, 200, 40);
        assert!(
            carded_under(&drawn, "pub struct Notification {", "Wrapped, and still here."),
            "{drawn:#?}"
        );
    }

    /// A comment on a deletion shows in the split row it shares with the addition beside it.
    ///
    /// The break this catches: a pair looked up comments on its new side only, and an addition has
    /// no old line, so the deletion's comment was never drawn.
    #[test]
    fn paired_deletion_keeps_old_side_feedback() {
        let mut app = commented(2, "The removed field mattered.");
        let drawn = rows(&mut app, 200, 40);
        let said = carded_under(&drawn, "pub category: String", "The removed field mattered.");
        assert!(said, "{drawn:#?}");
    }

    /// With the cursor on an addition paired with a deletion, the split pane draws that pair.
    #[test]
    fn a_selected_addition_reaches_the_split_screen() {
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewSplit).expect("fixture");
        app.guided_review_workspace.position.line_offset = 3;
        let drawn = rows(&mut app, 200, 40);
        let shown = drawn.iter().any(|row| row.contains("pub category: NotificationCategory"));
        assert!(shown, "{drawn:#?}");
    }
}
