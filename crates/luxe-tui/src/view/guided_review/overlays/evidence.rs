//! Finding evidence — the fix dialog, with the source where the finding was and where it is now.

use super::finding::anchor_summary;
use super::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

/// What changed at a comment's anchor since it was submitted, so `NeedsVerification` is checked
/// against evidence rather than trust.
///
/// A model saying "fixed" is a claim. Without this the only way to test it was to refresh the whole
/// review and hunt for the line by hand, which in practice meant pressing `r` and believing it.
pub(in crate::view::guided_review) fn render_finding_fix(frame: &mut Frame, area: Rect, app: &App) {
    let Some((finding, submitted)) = app.review.session().and_then(|session| {
        let finding = app
            .guided_review_workspace
            .selected_finding_id
            .as_ref()
            .and_then(|id| session.findings.iter().find(|finding| &finding.id == id))?;
        let submitted = session
            .submissions
            .iter()
            .rev()
            .flat_map(|submission| &submission.findings)
            .find(|snapshot| snapshot.finding_id == finding.id);
        Some((finding, submitted))
    }) else {
        return;
    };
    let width = area.width.saturating_sub(4).clamp(48, 96);
    let height = area.height.saturating_sub(4).clamp(12, 22);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let row_width = usize::from(width.saturating_sub(4));
    let mut lines = vec![
        Line::from(Span::styled(finding.title.clone(), app.theme_styles.accent)),
        Line::from(Span::styled(anchor_summary(app, &finding.anchor), app.theme_styles.muted)),
        Line::from(""),
    ];

    match submitted.and_then(|snapshot| snapshot.evidence.source.as_deref()) {
        Some(source) => {
            lines.push(Line::from(Span::styled("When you commented", app.theme_styles.muted)));
            lines.push(Line::from(Span::styled(
                format!("  {}", clip_terminal_cells_ellipsis(source.trim(), row_width)),
                app.theme_styles.error,
            )));
        }
        None => lines.push(Line::from(Span::styled(
            "This comment was not anchored to a source line.",
            app.theme_styles.muted,
        ))),
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Now", app.theme_styles.muted)));
    let submitted_source = submitted.and_then(|snapshot| snapshot.evidence.source.as_deref());
    match current_anchor_lines(app, finding, row_width) {
        Some(current) if !current.is_empty() => lines.extend(current),
        // The anchor is gone. Remediation usually *moves* code — renames a function, extracts a
        // helper — so "refresh and look for it yourself" abandons the reviewer at exactly the
        // moment the tool should help. Look for the line they commented on instead.
        _ => lines.extend(relocated_lines(app, submitted_source, row_width)),
    }

    if let Some(agent) = finding
        .thread
        .iter()
        .rev()
        .find(|entry| entry.author == luxe_core::guided_review::FindingMessageAuthor::Agent)
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Agent report — a claim, not proof",
            app.theme_styles.muted,
        )));
        for line in agent.body.lines().take(3) {
            lines.push(Line::from(Span::styled(
                format!("  {}", clip_terminal_cells_ellipsis(line, row_width)),
                app.theme_styles.text,
            )));
        }
    }

    let block = overlay_block(app, Icon::Evidence.glyph(), "What changed here");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(
        frame,
        block,
        popup,
        "r verify and resolve · c reply · esc close",
        None,
        &app.theme_styles,
    );
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// Where the commented-on source went, when its anchor no longer resolves.
fn relocated_lines(
    app: &App,
    submitted_source: Option<&str>,
    row_width: usize,
) -> Vec<Line<'static>> {
    let needle = submitted_source.map(str::trim).filter(|text| !text.is_empty());
    let found = needle.and_then(|needle| {
        let review = app.guided_review.as_ref()?;
        review.files.iter().find_map(|file| {
            let hunk = file
                .hunks
                .iter()
                .find(|hunk| hunk.lines.iter().any(|line| line.text.trim() == needle))?;
            let path = file
                .new_path
                .as_deref()
                .or(file.old_path.as_deref())
                .map_or_else(|| "(unknown path)".to_string(), |path| path.display().to_string());
            Some((path, hunk.heading.clone()))
        })
    });
    match found {
        Some((path, heading)) => {
            let location = match heading {
                Some(heading) => format!("{path} · {heading}"),
                None => path,
            };
            vec![
                Line::from(Span::styled(
                    "  the anchor is gone, but that line now lives at",
                    app.theme_styles.warning,
                )),
                Line::from(Span::styled(
                    format!("  {}", clip_terminal_cells_ellipsis(&location, row_width)),
                    app.theme_styles.info,
                )),
            ]
        }
        None if needle.is_some() => vec![Line::from(Span::styled(
            "  that line is gone from this revision entirely",
            app.theme_styles.warning,
        ))],
        None => vec![Line::from(Span::styled(
            "  this anchor no longer resolves — refresh the review (ctrl+r)",
            app.theme_styles.warning,
        ))],
    }
}

/// The anchored hunk as the current revision has it, or `None` when the anchor is gone.
fn current_anchor_lines(
    app: &App,
    finding: &luxe_core::guided_review::ReviewFinding,
    row_width: usize,
) -> Option<Vec<Line<'static>>> {
    use luxe_core::guided_review::{DiffSide, FindingAnchor, ReviewLineKind};
    let review = app.guided_review.as_ref()?;
    let (file_id, hunk_id, side_line) = match &finding.anchor {
        FindingAnchor::Line { file_id, hunk_id, side, line, .. } => {
            (file_id, hunk_id, Some((*side, *line)))
        }
        FindingAnchor::Hunk { file_id, hunk_id } => (file_id, hunk_id, None),
        _ => return None,
    };
    let file = review.files.iter().find(|file| &file.id == file_id)?;
    let hunk = file.hunks.iter().find(|hunk| &hunk.id == hunk_id)?;
    // Center a short window on the anchored line so the fix is readable without the whole hunk.
    let anchor_index = side_line.and_then(|(side, line)| {
        hunk.lines.iter().position(|source| match side {
            DiffSide::Old => source.old_line == Some(line),
            DiffSide::New => source.new_line == Some(line),
        })
    });
    let start = anchor_index.map_or(0, |index| index.saturating_sub(3));
    let window = hunk.lines.iter().skip(start).take(8);
    Some(
        window
            .enumerate()
            .map(|(offset, line)| {
                let marker = match line.kind {
                    ReviewLineKind::Addition => '+',
                    ReviewLineKind::Deletion => '-',
                    _ => ' ',
                };
                let style = match line.kind {
                    ReviewLineKind::Addition => app.theme_styles.success,
                    ReviewLineKind::Deletion => app.theme_styles.error,
                    _ => app.theme_styles.text,
                };
                let cursor = if anchor_index == Some(start + offset) { "›" } else { " " };
                Line::from(Span::styled(
                    format!(
                        "{cursor} {marker}{}",
                        clip_terminal_cells_ellipsis(
                            &crate::tabstop::display_text(&line.text),
                            row_width.saturating_sub(3),
                        )
                    ),
                    style,
                ))
            })
            .collect(),
    )
}
