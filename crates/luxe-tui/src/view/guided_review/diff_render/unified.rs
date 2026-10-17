//! Unified review diff — the selected file's rows one above the other, with folds, the active bar
//! and the feedback under each line.

use super::annotations::{
    comment_lines, findings_at, line_anchor_matches, thread_lines, threads_at,
};
use super::source::{
    active_bar_span, diff_line_background, emphasis_style, full_source_spans,
    highlighted_source_spans, hunk_header_line, intraline_emphasis, ACTIVE_BAR_WIDTH,
};
use super::split::render_split_file_diff;
use super::{append_end_of_reading, render_non_text_diff, shown_file};
use crate::app::App;
use crate::guided_review::diff::{clip_terminal_cells, gutter_width, visible_rows, UnifiedDiffRow};
use crate::guided_review::visible_groups;
use crate::view::diff_lines::{changed_spans, clip_styled_spans, rendered_marker, tint_row};
use crate::view::emphasis;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Cells a unified row spends between its numbers and its source: a space after each number, and
/// the diff sign.
pub(super) const UNIFIED_NUMBER_CHROME: usize = 3;

pub(in crate::view::guided_review) fn render_first_file_diff(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    let Some(file) = shown_file(app, review) else {
        return;
    };
    if !file.is_text() {
        render_non_text_diff(frame, area, app, file);
        return;
    }

    if app.guided_review_workspace.split_layout_active_for(file) {
        render_split_file_diff(frame, area, app, file, review);
        return;
    }

    let width = gutter_width(&file.hunks);
    // One cell at the margin for the bar down the hunk being read, so `Space` is a shape moving
    // rather than a redraw of the same frame with different code in it.
    let source_width = usize::from(area.width)
        .saturating_sub(width.saturating_mul(2) + ACTIVE_BAR_WIDTH + UNIFIED_NUMBER_CHROME);
    let groups = visible_groups(file, &app.guided_review_workspace.fold);
    let rows = visible_rows(
        &groups,
        app.guided_review_workspace.position.line_offset,
        usize::from(area.height),
    );
    let emphasized = intraline_emphasis(&rows);
    let line_offset = app.guided_review_workspace.position.line_offset;
    // The rows the hunk under the cursor owns, so its bar runs the whole height of it even when
    // the window starts below its header.
    let active_rows = app
        .guided_review_workspace
        .position
        .hunk_id
        .as_ref()
        .and_then(|hunk_id| crate::guided_review::hunk_row_span(&groups, hunk_id));
    let mut lines: Vec<Line> = Vec::new();
    // (first visual Y, visual height, absolute row index) so click regions map to the right source
    // row despite blank inter-hunk spacers and wrapped continuation lines.
    let mut row_ys: Vec<(u16, u16, usize)> = Vec::new();
    let context = UnifiedContext {
        app,
        file,
        width,
        source_width,
        area_width: area.width,
        emphasized: &emphasized,
    };
    for (index, row) in rows.iter().enumerate() {
        // Separate stacked blocks with a blank row so each reads as a distinct one.
        if matches!(row, UnifiedDiffRow::HunkHeader { .. } | UnifiedDiffRow::Folded(_))
            && !lines.is_empty()
        {
            lines.push(Line::default());
        }
        let first_y = lines.len() as u16;
        let on_active_hunk =
            active_rows.as_ref().is_some_and(|span| span.contains(&(line_offset + index)));
        match *row {
            UnifiedDiffRow::Folded(run) => {
                let selected = app.guided_review_workspace.position.line_offset
                    == line_offset + index
                    && app.guided_review_workspace.position.hunk_id.is_none();
                lines.push(folded_run_line(app, run, selected, false));
            }
            UnifiedDiffRow::HunkHeader { hunk_id, old_range, new_range, heading } => {
                let header = (hunk_id, old_range, new_range, heading);
                lines.extend(hunk_header_rows(app, header, on_active_hunk, area.width));
            }
            UnifiedDiffRow::Source(line) => {
                lines.extend(unified_source_rows(&context, line, index, on_active_hunk));
                append_unified_annotations(&mut lines, app, file, line, usize::from(area.width));
            }
        }
        let visual_height = (lines.len() as u16).saturating_sub(first_y).max(1);
        row_ys.push((first_y, visual_height, line_offset + index));
    }
    publish_diff_rows(app, area, &row_ys, &rows, file);
    append_end_of_reading(&mut lines, app, review, (&groups, rows.len()), area.height);
    frame.render_widget(Paragraph::new(lines), area);
}

/// A hunk's header row, with the comment cards anchored to the whole hunk under it.
fn hunk_header_rows(
    app: &App,
    (hunk_id, old_range, new_range, heading): (
        &luxe_core::guided_review::HunkId,
        luxe_core::guided_review::LineRange,
        luxe_core::guided_review::LineRange,
        Option<&str>,
    ),
    on_active_hunk: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines =
        vec![hunk_header_line(app, hunk_id, old_range, new_range, heading, on_active_hunk)];
    use luxe_core::guided_review::FindingAnchor;
    let on_hunk = |anchor: &FindingAnchor| match anchor {
        FindingAnchor::Hunk { hunk_id: at, .. } => at == hunk_id,
        _ => false,
    };
    lines.extend(comment_lines(app, &findings_at(app, on_hunk), usize::from(width)));
    lines
}

/// What every unified source row is drawn against.
struct UnifiedContext<'a> {
    app: &'a App,
    file: &'a luxe_core::guided_review::ReviewFile,
    /// Digits each line number takes
    width: usize,
    source_width: usize,
    area_width: u16,
    emphasized: &'a crate::view::diff_lines::IntralineEmphasis,
}

/// One source line as its visual rows: the active bar, both numbers and the sign, then the source
/// wrapped under that gutter or clipped at the diff's column offset.
fn unified_source_rows(
    ctx: &UnifiedContext<'_>,
    line: &luxe_core::guided_review::ReviewLine,
    index: usize,
    on_active_hunk: bool,
) -> Vec<Line<'static>> {
    let (app, file, width) = (ctx.app, ctx.file, ctx.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let old = line.old_line.map_or_else(|| " ".repeat(width), |number| format!("{number:>width$}"));
    let new = line.new_line.map_or_else(|| " ".repeat(width), |number| format!("{number:>width$}"));
    let style = match line.kind {
        luxe_core::guided_review::ReviewLineKind::Addition => app.theme_styles.success,
        luxe_core::guided_review::ReviewLineKind::Deletion => app.theme_styles.error,
        _ => app.theme_styles.text,
    };
    // The fixed gutter (active bar + old/new numbers + diff marker) prefixing the
    // first visual line; wrapped continuations align their source under it.
    let gutter = || {
        vec![
            active_bar_span(app, on_active_hunk),
            Span::styled(format!("{old} {new} "), app.theme_styles.muted),
            Span::styled(rendered_marker(line.kind), style),
        ]
    };
    let source = crate::tabstop::display_text(&line.text);
    // Tint add/removed rows with a full-width bar; a search match wins so the located
    // line stays obvious over the add/del tint.
    let row_bg = if app.guided_review_workspace.line_matches_search(&source) {
        Some(app.theme_styles.selected_bg)
    } else {
        diff_line_background(line.kind, app)
    };
    let finish = |mut spans: Vec<Span<'static>>| match row_bg {
        Some(background) => Line::from(tint_row(spans, background, ctx.area_width)),
        None => {
            spans.shrink_to_fit();
            Line::from(spans)
        }
    };
    if app.guided_review_workspace.word_wrap {
        // Soft-wrap the full source under the gutter with an aligned continuation glyph,
        // instead of horizontal scrolling (`w` toggles this).
        let full = full_source_spans(line, changed_spans(ctx.emphasized, index), style, file, app);
        let continuation_pad = ACTIVE_BAR_WIDTH + width * 2 + 2;
        for (segment_index, segment) in
            crate::widgets::wrap_cells(full, ctx.source_width).into_iter().enumerate()
        {
            let mut spans = if segment_index == 0 {
                gutter()
            } else {
                vec![Span::styled(
                    format!(
                        "{}{}",
                        " ".repeat(continuation_pad),
                        crate::glyphs::Mark::Continuation
                    ),
                    app.theme_styles.dim,
                )]
            };
            spans.extend(segment);
            lines.push(finish(spans));
        }
        // Guarantee at least one visual line (e.g. an empty added line).
        if lines.is_empty() {
            lines.push(finish(gutter()));
        }
        return lines;
    }
    let column_offset = app.guided_review_workspace.diff_column_offset;
    let source_spans = match line.kind {
        luxe_core::guided_review::ReviewLineKind::Context => {
            let clipped = clip_terminal_cells(&source, column_offset, ctx.source_width);
            highlighted_source_spans(&clipped, file.language.as_deref(), app)
        }
        // Mark only the spans that changed; everything either side keeps the line's
        // own color.
        _ => crate::view::diff_lines::owned_spans(clip_styled_spans(
            &emphasis::parts(
                &source,
                changed_spans(ctx.emphasized, index),
                style,
                emphasis_style(app, style, line),
            ),
            column_offset,
            ctx.source_width,
        )),
    };
    let mut spans = gutter();
    spans.extend(source_spans);
    lines.push(match row_bg {
        Some(background) => Line::from(tint_row(spans, background, ctx.area_width)),
        None => Line::from(spans),
    });
    lines
}

/// The comments and earlier threads anchored to a source line, under it.
///
/// The comment sits under the line it is about, the way every review tool shows one: a bare `R1` in
/// a gutter, with the words in a drawer over the far corner, makes "where did my comment go" a
/// question at all.
fn append_unified_annotations(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    file: &luxe_core::guided_review::ReviewFile,
    line: &luxe_core::guided_review::ReviewLine,
    width: usize,
) {
    lines.extend(comment_lines(
        app,
        &findings_at(app, |anchor| line_anchor_matches(anchor, &file.id, line)),
        width,
    ));
    lines.extend(thread_lines(app, &threads_at(app, &file.id, line), width));
}

/// Publish a click region per rendered source/header row → its absolute index (spanning its
/// wrapped visual lines), so a click moves the cursor/anchor there. A collapsed run is the one
/// row whose click means "open this" rather than "put the cursor here".
fn publish_diff_rows(
    app: &App,
    area: Rect,
    row_ys: &[(u16, u16, usize)],
    rows: &[UnifiedDiffRow<'_>],
    file: &luxe_core::guided_review::ReviewFile,
) {
    let mut regions = app.hit.borrow_mut();
    for ((y, height, absolute), row) in row_ys.iter().zip(rows) {
        let row_y = area.y.saturating_add(*y);
        if row_y >= area.bottom() {
            continue;
        }
        let region_height = (*height).min(area.bottom().saturating_sub(row_y));
        let target = if matches!(row, UnifiedDiffRow::Folded(_)) {
            crate::mouse::MouseTarget::ReviewFoldedRun(file.id.clone())
        } else {
            crate::mouse::MouseTarget::ReviewDiffRow(*absolute)
        };
        regions.push(Rect::new(area.x, row_y, area.width, region_height), target);
    }
}

/// The row that stands in for a run of collapsed mechanical hunks.
///
/// It is a row, not an absence: whatever the classifier decided, the reviewer can see that
/// something is there, what kind of thing it is, how much of it there is, and how to open it.
pub(super) fn folded_run_line(
    app: &App,
    run: crate::guided_review::FoldedRun,
    selected: bool,
    full_width: bool,
) -> Line<'static> {
    let style = if selected {
        app.theme_styles.muted.patch(app.theme_styles.selected_bg)
    } else {
        app.theme_styles.muted
    };
    let marker =
        format!("{}{} ", if selected { "› " } else { "  " }, crate::glyphs::Mark::FoldClosed);
    let mut spans = vec![
        Span::styled(marker, style),
        Span::styled(run.summary(), style),
        Span::styled("  ·  enter opens", app.theme_styles.dim),
    ];
    if full_width {
        spans.push(Span::styled(String::new(), style));
    }
    Line::from(spans)
}
