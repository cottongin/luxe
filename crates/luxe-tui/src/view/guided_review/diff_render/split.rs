//! Side-by-side review diff — old and new columns over the same rows, each pair's source and the
//! feedback either side of it carries.

use super::annotations::{
    comment_lines, findings_at, line_anchor_matches, thread_lines, threads_at,
};
use super::append_end_of_reading;
use super::source::{active_bar_span, hunk_header_line, ACTIVE_BAR_WIDTH};
use super::unified::folded_run_line;
use crate::app::App;
use crate::guided_review::diff::{gutter_width, visible_split_rows, SplitDiffCell, SplitRow};
use crate::guided_review::visible_groups;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// One old/new pair's source, as one line or, wrapped, as many as it takes, behind the active-hunk
/// bar. Pairing is exact here, so the changed spans are too — no run-matching heuristic.
fn append_split_pair(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    layout: &crate::view::diff_lines::SplitLayout<'_>,
    (old, new): (SplitDiffCell<'_>, SplitDiffCell<'_>),
    (bar_span, word_wrap): (&Span<'static>, bool),
) {
    let changed = match (old, new) {
        // over the drawn text, like every other intra-line pass: the ranges are byte offsets
        // into the string the renderer slices
        (SplitDiffCell::Source(old), SplitDiffCell::Source(new)) if old.kind != new.kind => {
            luxe_core::intraline::intraline_change(
                &crate::tabstop::display_text(&old.text),
                &crate::tabstop::display_text(&new.text),
            )
        }
        _ => luxe_core::intraline::IntralineChange::default(),
    };
    let (old_style, new_style) = (split_cell_style(app, old), split_cell_style(app, new));
    if word_wrap {
        let wrapped = layout
            .wrapped_pair((old, &changed.removed, &old_style), (new, &changed.added, &new_style));
        lines.extend(wrapped.into_iter().map(|line| {
            let mut spans = vec![bar_span.clone()];
            spans.extend(line.spans);
            Line::from(spans)
        }));
        return;
    }
    use crate::view::diff_lines::SplitSide;
    let mut spans = vec![bar_span.clone()];
    spans.extend(layout.cell_spans(SplitSide::Old, old, &changed.removed, &old_style));
    spans.push(layout.divider());
    spans.extend(layout.cell_spans(SplitSide::New, new, &changed.added, &new_style));
    lines.push(Line::from(spans));
}

/// The comments and earlier threads anchored to either side of a pair, each once.
///
/// Both sides, because a pair can carry a deletion's comment on its old line and an addition's on
/// its new one; looking only at the new side lost the first whenever the second had no old line.
/// A context line sits on both sides, so what it carries is kept once.
fn pair_annotations(
    app: &App,
    file: &luxe_core::guided_review::ReviewFile,
    (old, new): (SplitDiffCell<'_>, SplitDiffCell<'_>),
    width: usize,
) -> Vec<Line<'static>> {
    let mut findings: Vec<luxe_core::guided_review::ReviewFinding> = Vec::new();
    let mut threads: Vec<&luxe_core::guided_review::EarlierThread> = Vec::new();
    // old first, as the unified pane and the left column read
    for cell in [old, new] {
        let SplitDiffCell::Source(line) = cell else { continue };
        for finding in findings_at(app, |anchor| line_anchor_matches(anchor, &file.id, line)) {
            if !findings.iter().any(|kept| kept.id == finding.id) {
                findings.push(finding);
            }
        }
        for thread in threads_at(app, &file.id, line) {
            if !threads.iter().any(|kept| std::ptr::eq(*kept, thread)) {
                threads.push(thread);
            }
        }
    }
    let mut lines = comment_lines(app, &findings, width);
    lines.extend(thread_lines(app, &threads, width));
    lines
}

/// Renders the selected file side by side: old on the left, new on the right.
///
/// Shares the unified row space (`position.line_offset`), so scrolling, hunk selection, click
/// mapping, search and the findings gutter behave exactly as they do unified. The previous split
/// renderer paged one hunk against a private offset and could not show a file.
pub(super) fn render_split_file_diff(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    file: &luxe_core::guided_review::ReviewFile,
    review: &luxe_core::guided_review::GuidedReview,
) {
    let number_width = gutter_width(&file.hunks);
    // The active-hunk bar sits outside both columns, at the margin, so the rule between them does
    // not move as the cursor does.
    let bar = ACTIVE_BAR_WIDTH as u16;
    let mut layout = crate::view::diff_lines::split_layout(
        &app.theme_styles,
        area.width.saturating_sub(bar),
        number_width,
        0,
        file.language.as_deref(),
    );
    // Wrapping supersedes horizontal scrolling, and the workspace already zeroes the offset when
    // it turns on; passing it regardless keeps the two paths reading from one source of truth.
    layout.column = app.guided_review_workspace.diff_column_offset;
    let groups = visible_groups(file, &app.guided_review_workspace.fold);
    let rows = visible_split_rows(
        &groups,
        app.guided_review_workspace.position.line_offset,
        usize::from(area.height),
    );

    let active_rows = app
        .guided_review_workspace
        .position
        .hunk_id
        .as_ref()
        .and_then(|hunk_id| crate::guided_review::hunk_row_span(&groups, hunk_id));
    let word_wrap = app.guided_review_workspace.word_wrap;
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
    let mut row_indices: Vec<usize> = Vec::with_capacity(rows.len());
    // Visual height per logical row: what it drew, wrapped lines and comment cards included.
    let mut heights: Vec<u16> = Vec::with_capacity(rows.len());
    let drawn = SplitRowContext { app, file, layout: &layout, word_wrap, width: area.width };
    for row in &rows {
        row_indices.push(row.unified_index());
        let on_active_hunk =
            active_rows.as_ref().is_some_and(|span| span.contains(&row.unified_index()));
        heights.push(append_split_row(&mut lines, &drawn, row, on_active_hunk));
    }
    publish_split_rows(app, area, &row_indices, &heights, &rows, file);
    append_end_of_reading(&mut lines, app, review, (&groups, rows.len()), area.height);
    frame.render_widget(Paragraph::new(lines), area);
}

/// What every split row is drawn against.
struct SplitRowContext<'a> {
    app: &'a App,
    file: &'a luxe_core::guided_review::ReviewFile,
    layout: &'a crate::view::diff_lines::SplitLayout<'a>,
    word_wrap: bool,
    width: u16,
}

/// Appends one logical split row — a folded run, a hunk header with its cards, or a pair with the
/// feedback either side of it — and returns how many visual lines it drew.
fn append_split_row(
    lines: &mut Vec<Line<'static>>,
    ctx: &SplitRowContext<'_>,
    row: &SplitRow<'_>,
    on_active_hunk: bool,
) -> u16 {
    let (app, file) = (ctx.app, ctx.file);
    let bar_span = active_bar_span(app, on_active_hunk);
    match row {
        SplitRow::Folded { run, unified_index } => {
            let selected = app.guided_review_workspace.position.line_offset == *unified_index
                && app.guided_review_workspace.position.hunk_id.is_none();
            lines.push(folded_run_line(app, *run, selected, true));
            1
        }
        SplitRow::Header { hunk_id, old_range, new_range, heading, .. } => {
            let before = lines.len();
            lines.push(split_header_line(
                app,
                hunk_id,
                *old_range,
                *new_range,
                *heading,
                on_active_hunk,
            ));
            lines.extend(comment_lines(
                app,
                &findings_at(app, |anchor| {
                    matches!(anchor, luxe_core::guided_review::FindingAnchor::Hunk { hunk_id: at, .. } if at == *hunk_id)
                }),
                usize::from(ctx.width),
            ));
            // a header carrying comment cards is that many rows tall, or every click below
            // it lands a row short
            (lines.len() - before) as u16
        }
        SplitRow::Pair { old, new, .. } => {
            let before = lines.len();
            append_split_pair(lines, app, ctx.layout, (*old, *new), (&bar_span, ctx.word_wrap));
            lines.extend(pair_annotations(app, file, (*old, *new), usize::from(ctx.width)));
            (lines.len() - before).max(1) as u16
        }
    }
}

/// Same click contract as unified: a row maps to its unified index, so a click anchors a comment
/// at the line the reviewer pointed at — including a wrapped row, across every visual line it
/// occupies.
fn publish_split_rows(
    app: &App,
    area: Rect,
    row_indices: &[usize],
    heights: &[u16],
    rows: &[SplitRow<'_>],
    file: &luxe_core::guided_review::ReviewFile,
) {
    let mut regions = app.hit.borrow_mut();
    let mut y = area.y;
    for ((unified, height), row) in row_indices.iter().zip(heights).zip(rows) {
        if y >= area.bottom() {
            break;
        }
        let region_height = (*height).min(area.bottom().saturating_sub(y));
        let target = if matches!(row, SplitRow::Folded { .. }) {
            crate::mouse::MouseTarget::ReviewFoldedRun(file.id.clone())
        } else {
            crate::mouse::MouseTarget::ReviewDiffRow(*unified)
        };
        regions.push(Rect::new(area.x, y, area.width, region_height), target);
        y = y.saturating_add(*height);
    }
}

/// What the review knows about one side of a split row that the shared layout does not: whether a
/// search hit is claiming the row's background.
fn split_cell_style(app: &App, cell: SplitDiffCell<'_>) -> crate::view::diff_lines::SplitCellStyle {
    let SplitDiffCell::Source(line) = cell else {
        return crate::view::diff_lines::SplitCellStyle::default();
    };
    let searched =
        app.guided_review_workspace.line_matches_search(&crate::tabstop::display_text(&line.text));
    crate::view::diff_lines::SplitCellStyle {
        marker: String::new(),
        background: searched.then_some(app.theme_styles.selected_bg),
        plain_wash: searched,
    }
}

/// A hunk header spanning both columns, carrying the same selection and finding markers as unified.
fn split_header_line(
    app: &App,
    hunk_id: &luxe_core::guided_review::HunkId,
    old_range: luxe_core::guided_review::LineRange,
    new_range: luxe_core::guided_review::LineRange,
    heading: Option<&str>,
    active: bool,
) -> Line<'static> {
    hunk_header_line(app, hunk_id, old_range, new_range, heading, active)
}
