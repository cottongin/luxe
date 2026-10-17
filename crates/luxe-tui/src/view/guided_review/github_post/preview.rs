//! Post preview — the selected item as GitHub will show it: its Markdown and the code it is on.

use super::items::{Item, OnLines};
use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use luxe_core::guided_review::DiffSide;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Diff lines above a line comment, its own included, the way GitHub shows a comment's context.
const EXCERPT_LINES: usize = 4;

/// The most lines of a comment on a run of lines the excerpt shows, ending at the run's last.
const EXCERPT_MOST: usize = 8;

/// The selected item's text as rows of `width`: where it goes, the diff it sits under when it sits
/// on lines, then the words.
pub(super) fn pane_lines(app: &App, item: &Item, width: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines =
        vec![Line::from(Span::styled(item.place.clone(), styles.accent)), Line::from("")];
    if let Some(on) = &item.on {
        let excerpt = excerpt(app, on, width);
        if !excerpt.is_empty() {
            lines.extend(excerpt);
            lines.push(Line::from(""));
        }
    }
    let post = app.guided_review_workspace.github_post.as_ref();
    let summary = item.on.is_none();
    let drafting = post.and_then(|post| post.drafting_since).filter(|_| summary);
    if let Some(since) = drafting {
        lines.push(drafting_line(app, since));
        if !item.body.is_empty() {
            lines.push(Line::from(""));
        }
    }
    if item.body.is_empty() {
        if drafting.is_none() {
            lines.push(Line::from(Span::styled("No summary · c write one", styles.dim)));
        }
    } else {
        lines.extend(crate::markdown::render_markdown(&item.body, styles));
    }
    let mut lines: Vec<Line<'static>> =
        lines.iter().flat_map(|line| crate::widgets::wrap_line(line, width)).collect();
    if let Some(note) = post.and_then(|post| post.draft_note.as_deref()).filter(|_| summary) {
        lines.push(Line::from(""));
        lines.extend(crate::github_detail::draft_note_lines(note, width, styles));
    }
    lines
}

/// The wait for a drafted summary, drawn where the summary will land: who is writing it, and a
/// count, because a minute of a still line reads as a hang.
fn drafting_line(app: &App, since: std::time::Instant) -> Line<'static> {
    let styles = &app.theme_styles;
    let model = match app.status.model.trim() {
        "" => "The agent",
        model => model,
    };
    Line::from(vec![
        Span::styled(
            format!("{} drafting ", crate::statusline::spinner(app.guided_review_workspace.frame)),
            styles.accent,
        ),
        Span::styled(
            format!(
                "\u{b7} {model} is drafting the review summary \u{b7} {}",
                crate::statusline::format_elapsed_compact(
                    app.frame_now().saturating_duration_since(since)
                )
            ),
            styles.muted,
        ),
    ])
}

/// The diff a line comment sits under, as GitHub shows it: up to [`EXCERPT_LINES`] lines ending at
/// the comment's last, numbered and colored the way the review draws them, with its own lines
/// marked. Empty when the lines are not in the review, which leaves the heading to say where.
fn excerpt(app: &App, on: &OnLines, width: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let number = |line: &luxe_core::guided_review::ReviewLine| match on.side {
        DiffSide::Old => line.old_line,
        DiffSide::New => line.new_line,
    };
    let Some(hunk_lines) = app.guided_review.as_ref().and_then(|review| {
        let file = review.files.iter().find(|file| {
            let path = match on.side {
                DiffSide::Old => file.old_path.as_deref(),
                DiffSide::New => file.new_path.as_deref(),
            };
            path.is_some_and(|path| luxe_core::paths::display(path) == on.path)
        })?;
        file.hunks.iter().find_map(|hunk| {
            let end = hunk.lines.iter().position(|line| number(line) == Some(on.end))?;
            let start = hunk.lines.iter().position(|line| number(line) == Some(on.start));
            let from = start
                .unwrap_or(end)
                .min(end.saturating_sub(EXCERPT_LINES - 1))
                .max((end + 1).saturating_sub(EXCERPT_MOST));
            Some(&hunk.lines[from..=end])
        })
    }) else {
        return Vec::new();
    };
    let digits = hunk_lines
        .iter()
        .filter_map(|line| line.new_line.or(line.old_line))
        .max()
        .map_or(1, |most| most.to_string().len());
    hunk_lines
        .iter()
        .map(|line| {
            use luxe_core::guided_review::ReviewLineKind;
            let (sign, fill) = match line.kind {
                ReviewLineKind::Addition => ("+", styles.diff_add_bg),
                ReviewLineKind::Deletion => ("-", styles.diff_del_bg),
                _ => (" ", Style::default()),
            };
            let commented = number(line).is_some_and(|n| (on.start..=on.end).contains(&n));
            let shown = line.new_line.or(line.old_line).map_or_else(String::new, |n| n.to_string());
            let lead = format!("{shown:>digits$} {sign} ");
            let room = width.saturating_sub(1 + lead.chars().count());
            let text = clip_terminal_cells_ellipsis(&line.text.replace('\t', "    "), room);
            let pad = room.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
            Line::from(vec![
                Span::styled(if commented { "▎" } else { " " }, styles.accent),
                Span::styled(lead, styles.muted.patch(fill)),
                Span::styled(format!("{text}{}", " ".repeat(pad)), styles.text.patch(fill)),
            ])
        })
        .collect()
}
