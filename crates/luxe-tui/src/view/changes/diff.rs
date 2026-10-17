//! Changes diff — the diff's rows, windowed to the pane, unified or side by side.

use super::cells::{code_pieces, publish_and_paint, CodeCell};
use super::split::split_lines;
use crate::app::App;
use crate::changes::{ChangesView, PeekView};
use crate::mouse::PeekSide;
use crate::theme::styles::ThemeStyles;
use crate::view::diff_lines::{
    changed_spans, clip_styled_spans, diff_line_background, highlighted_source_spans,
    intraline_emphasis, line_style, owned_spans as owned, rendered_marker, tint_row,
    IntralineEmphasis,
};
use crate::view::emphasis;
use luxe_core::guided_review::{ReviewFile, ReviewLine, ReviewLineKind};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// One row of the pane's own row space.
///
/// Flat, like the review's, so the offset the reader scrolls is an index into something the
/// renderer can walk without materializing the whole file.
enum DiffRow<'a> {
    /// A hunk's orientation band, drawn under a blank when it is not the first.
    Header(&'a luxe_core::guided_review::ReviewHunk),
    /// One source line.
    Source(&'a ReviewLine),
}

/// Every hunk of `file`, windowed to what fits, with a scrollbar when there is more.
pub(super) fn render_diff(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    view: &ChangesView,
    file: &ReviewFile,
) {
    let (body, scrollbar) = if area.width > 2 {
        (
            Rect { width: area.width.saturating_sub(1), ..area },
            Some(Rect { x: area.right().saturating_sub(1), width: 1, ..area }),
        )
    } else {
        (area, None)
    };
    let viewport = usize::from(body.height);
    let numbers = crate::guided_review::diff::gutter_width(&file.hunks);

    // One row space for both layouts — a hunk header, then its lines — so the offset means the same
    // thing either way and toggling the layout keeps the reader where they were.
    let rows = diff_rows(file);
    let sources: Vec<Option<&ReviewLine>> = rows
        .iter()
        .map(|row| match row {
            DiffRow::Source(line) => Some(*line),
            DiffRow::Header(_) => None,
        })
        .collect();
    let emphasized = intraline_emphasis(&sources);

    let unified_gutter = numbers * 2 + 3;
    let unified_code = usize::from(body.width).saturating_sub(unified_gutter).max(4);
    let split = crate::view::diff_lines::split_layout(
        &app.theme_styles,
        body.width,
        numbers,
        0,
        file.language.as_deref(),
    );
    let side_by_side = view.split_active(app.config.diff.layout(), split.source, Some(file));

    // Scroll bounds come from the *unified* heights whichever layout is showing. Side-by-side pairs
    // rows together, so it always shows at least as much as unified from the same offset — which
    // makes this bound reachable in both and dead space at the bottom impossible in either.
    let heights: Vec<usize> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| row_height(row, index, view.word_wrap, unified_code))
        .collect();
    view.viewport_rows.set(viewport.max(1));
    view.scroll_max.set(max_offset(&heights, viewport));
    let offset = view.scroll.min(view.scroll_max.get());

    let (mut lines, cells) = if side_by_side {
        split_lines(file, &split, view, offset, viewport)
    } else {
        let context = UnifiedRowContext {
            file,
            styles: &app.theme_styles,
            word_wrap: view.word_wrap,
            emphasized: &emphasized,
            numbers,
            gutter_width: unified_gutter,
            code_width: unified_code,
            width: body.width,
        };
        unified_lines(&rows, &context, offset, viewport)
    };

    publish_and_paint(app, view, &mut lines, &cells, body);
    frame.render_widget(Paragraph::new(lines), body);
    if let Some(scrollbar) = scrollbar {
        let consumed: usize = heights.iter().take(offset).sum();
        crate::widgets::steered_scrollbar(
            frame,
            &mut app.hit.borrow_mut(),
            scrollbar,
            crate::mouse::ScrollRegionId::Changes,
            consumed,
            heights.iter().sum(),
            viewport,
            &app.theme_styles,
        );
    }
}

/// The pane's row space: each hunk's header, then its lines.
///
/// The same shape `visible_split_rows` counts in, so the two layouts share one offset and toggling
/// between them keeps the reader where they were.
fn diff_rows(file: &ReviewFile) -> Vec<DiffRow<'_>> {
    let mut rows = Vec::new();
    for hunk in &file.hunks {
        rows.push(DiffRow::Header(hunk));
        rows.extend(hunk.lines.iter().map(DiffRow::Source));
    }
    rows
}

/// The line of the new file at the top of the pane, for opening the file there: the first row in
/// view, a hunk's header standing for its first line.
pub(crate) fn line_in_view(view: &ChangesView) -> Option<u32> {
    // The File view's rows are the file's own lines, and a pick says which one the reader means
    if view.view == PeekView::File {
        let standing = view.cursor.filter(|_| view.focused).unwrap_or_else(|| view.top_line());
        return Some(view.selected_lines().map_or(standing, |(first, _)| first));
    }
    let file = view.file()?;
    let mut row = 0usize;
    for hunk in &file.hunks {
        if row >= view.scroll {
            return (hunk.new_range.start > 0).then_some(hunk.new_range.start);
        }
        row += 1;
        for index in 0..hunk.lines.len() {
            if row >= view.scroll {
                return crate::guided_review::new_side_line(hunk, index);
            }
            row += 1;
        }
    }
    None
}

/// Visual lines one row occupies at this width.
///
/// A hunk header past the first carries the blank that separates it from the hunk above, so the
/// separator costs a line without costing a row — which is what keeps the row space identical to
/// the one side-by-side counts in.
fn row_height(row: &DiffRow<'_>, index: usize, word_wrap: bool, code_width: usize) -> usize {
    match row {
        DiffRow::Header(_) => 1 + usize::from(index > 0),
        DiffRow::Source(line) if word_wrap => {
            let cells = crate::tabstop::display_width(&line.text);
            cells.div_ceil(code_width.max(1)).max(1)
        }
        DiffRow::Source(_) => 1,
    }
}

/// What every unified row is drawn against.
struct UnifiedRowContext<'a> {
    file: &'a ReviewFile,
    styles: &'a ThemeStyles,
    word_wrap: bool,
    emphasized: &'a IntralineEmphasis,
    /// Digits each line number takes
    numbers: usize,
    gutter_width: usize,
    code_width: usize,
    width: u16,
}

/// One unified row as the visual lines it occupies.
fn render_row(row: &DiffRow<'_>, index: usize, ctx: &UnifiedRowContext<'_>) -> Vec<Line<'static>> {
    match row {
        DiffRow::Header(hunk) => {
            let header = hunk_header_line(
                hunk.old_range,
                hunk.new_range,
                hunk.heading.as_deref(),
                ctx.styles,
                Some(ctx.width),
            );
            if index > 0 {
                vec![Line::default(), header]
            } else {
                vec![header]
            }
        }
        DiffRow::Source(line) if ctx.word_wrap => wrapped_source_rows(line, index, ctx),
        DiffRow::Source(line) => vec![clipped_source_row(line, index, ctx)],
    }
}

/// A source line's `old new sign` gutter.
fn unified_gutter(line: &ReviewLine, ctx: &UnifiedRowContext<'_>) -> Vec<Span<'static>> {
    let numbers = ctx.numbers;
    let old =
        line.old_line.map_or_else(|| " ".repeat(numbers), |number| format!("{number:>numbers$}"));
    let new =
        line.new_line.map_or_else(|| " ".repeat(numbers), |number| format!("{number:>numbers$}"));
    vec![
        Span::styled(format!("{old} {new} "), ctx.styles.muted),
        Span::styled(rendered_marker(line.kind), line_style(line.kind, ctx.styles)),
    ]
}

/// A source line wrapped to the code column, each row after the first behind a continuation mark.
fn wrapped_source_rows(
    line: &ReviewLine,
    index: usize,
    ctx: &UnifiedRowContext<'_>,
) -> Vec<Line<'static>> {
    let styles = ctx.styles;
    let changed = changed_spans(ctx.emphasized, index);
    let background = diff_line_background(line.kind, styles);
    let full = crate::view::diff_lines::source_spans(
        line,
        changed,
        ctx.file.language.as_deref(),
        styles,
        false,
    );
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (segment_index, segment) in
        crate::widgets::wrap_cells(full, ctx.code_width).into_iter().enumerate()
    {
        let mut spans = if segment_index == 0 {
            unified_gutter(line, ctx)
        } else {
            vec![Span::styled(
                format!(
                    "{}{}",
                    " ".repeat(ctx.gutter_width.saturating_sub(1)),
                    crate::glyphs::Mark::Continuation
                ),
                styles.dim,
            )]
        };
        spans.extend(segment);
        lines.push(finish(spans, background, ctx.width));
    }
    if lines.is_empty() {
        lines.push(finish(unified_gutter(line, ctx), background, ctx.width));
    }
    lines
}

/// A source line clipped to the code column, marked where it runs past the edge.
fn clipped_source_row(
    line: &ReviewLine,
    index: usize,
    ctx: &UnifiedRowContext<'_>,
) -> Line<'static> {
    let (styles, code_width) = (ctx.styles, ctx.code_width);
    let style = line_style(line.kind, styles);
    let changed = changed_spans(ctx.emphasized, index);
    let background = diff_line_background(line.kind, styles);
    // Without wrapping there is no horizontal scroll either, so a line that runs past
    // the edge has to say so — a clipped statement reads as a complete one, which in a
    // diff is a claim about the code.
    let source = crate::tabstop::display_text(&line.text);
    let clipped = crate::tabstop::display_width(&line.text) > code_width;
    let room = if clipped { code_width.saturating_sub(1) } else { code_width };
    let mut spans = unified_gutter(line, ctx);
    spans.extend(match line.kind {
        ReviewLineKind::Context => highlighted_source_spans(
            &crate::guided_review::diff::clip_terminal_cells(&source, 0, room),
            ctx.file.language.as_deref(),
            styles,
        ),
        _ => owned(clip_styled_spans(
            &emphasis::parts(
                &source,
                changed,
                style,
                styles.diff_emphasis(style, line.kind == ReviewLineKind::Addition),
            ),
            0,
            room,
        )),
    });
    if clipped {
        spans.push(Span::styled("…", styles.dim));
    }
    finish(spans, background, ctx.width)
}

/// The largest offset that still puts the last row on screen.
fn max_offset(heights: &[usize], viewport: usize) -> usize {
    let mut used = 0usize;
    for (index, height) in heights.iter().enumerate().rev() {
        used = used.saturating_add(*height);
        if used > viewport {
            return index + 1;
        }
    }
    0
}

/// The drawn rows, and where each one's code sits.
pub(super) type DrawnRows = (Vec<Line<'static>>, Vec<Vec<CodeCell>>);

/// The unified list: `old new sign source`, one row per line.
fn unified_lines(
    rows: &[DiffRow<'_>],
    ctx: &UnifiedRowContext<'_>,
    offset: usize,
    viewport: usize,
) -> DrawnRows {
    let mut drawn: DrawnRows = (Vec::with_capacity(viewport), Vec::with_capacity(viewport));
    for (index, row) in rows.iter().enumerate().skip(offset) {
        if drawn.0.len() >= viewport {
            break;
        }
        let pieces = match row {
            DiffRow::Source(line) => {
                let at = (ctx.gutter_width, ctx.code_width);
                code_pieces(&line.text, index, PeekSide::Whole, at, ctx.word_wrap)
            }
            DiffRow::Header(_) => Vec::new(),
        };
        for (piece, line) in render_row(row, index, ctx).into_iter().enumerate() {
            if drawn.0.len() >= viewport {
                break;
            }
            drawn.0.push(line);
            drawn.1.push(pieces.get(piece).copied().into_iter().collect());
        }
    }
    drawn
}

/// `@@ -1,2 +1,2 @@  fn sum` — the band that opens a hunk, in either layout.
pub(super) fn hunk_header_line(
    old: luxe_core::guided_review::LineRange,
    new: luxe_core::guided_review::LineRange,
    heading: Option<&str>,
    styles: &crate::theme::styles::ThemeStyles,
    clip: Option<u16>,
) -> Line<'static> {
    let heading = heading
        .filter(|heading| !heading.trim().is_empty())
        .map(|heading| format!("  {heading}"))
        .unwrap_or_default();
    let text = format!("@@ -{},{} +{},{} @@{heading}", old.start, old.count, new.start, new.count);
    let text = match clip {
        Some(width) => {
            crate::guided_review::diff::clip_terminal_cells(&text, 0, usize::from(width))
        }
        None => text,
    };
    Line::from(Span::styled(text, styles.accent))
}

/// Detaches spans from the source line they were clipped out of.
///
/// The pane hands finished `Line`s back to its caller, so nothing may still borrow the review file
/// they came from.
/// Closes one rendered row, laying the changed-row tint across its full width.
fn finish(spans: Vec<Span<'static>>, background: Option<Style>, width: u16) -> Line<'static> {
    match background {
        Some(background) => Line::from(tint_row(spans, background, width)),
        None => Line::from(spans),
    }
}
