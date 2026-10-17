//! The File view — a project file as it is now, in the column the preview takes.
//!
//! Numbered, highlighted the way the transcript highlights code, with a bar in the gutter beside
//! every line the file's change added when it has one. A drag over the code selects text, the way
//! it does in the transcript; a drag down the line numbers picks whole lines, and an action row
//! under the pick offers what a reader does next: ask the agent about them, or copy them.

use super::cells::{code_pieces, publish_and_paint, CodeCell};
use super::{sentence, MouseTarget};
use crate::app::App;
use crate::changes::{ChangesView, TextLoad};
use crate::mouse::PeekSelectionAction;
use crate::view::diff_lines::highlighted_source_spans;
use luxe_core::guided_review::ReviewLineKind;
use luxe_core::project_files::ProjectFile;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

/// Draws the File view's body, or the one line explaining why there is none.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &App, view: &ChangesView) {
    view.viewport_rows.set(usize::from(area.height).max(1));
    let Some(file) = loaded_text_or_notice(frame, area, app, view) else {
        return;
    };

    let (body, scrollbar) = match area.width > 2 {
        true => (
            Rect { width: area.width - 1, ..area },
            Some(Rect { x: area.right() - 1, width: 1, ..area }),
        ),
        false => (area, None),
    };
    let lines: Vec<&str> = file.text.lines().collect();
    let numbers = lines.len().max(1).to_string().len();
    // number, a space, the change bar, a space
    let gutter = numbers + 3;
    let columns = TextColumns {
        numbers,
        gutter,
        code: usize::from(body.width).saturating_sub(gutter).max(4),
        width: body.width,
        viewport: usize::from(body.height),
    };
    let offset = text_window(view, &lines, &columns);
    let TextFrame { mut rows, hits, cells } = text_rows(app, view, file, &lines, offset, &columns);
    publish_and_paint(app, view, &mut rows, &cells, body);
    frame.render_widget(Paragraph::new(rows.clone()), body);
    publish_text_hits(app, &rows, hits, body, gutter);
    if let Some(scrollbar) = scrollbar {
        crate::widgets::steered_scrollbar(
            frame,
            &mut app.hit.borrow_mut(),
            scrollbar,
            crate::mouse::ScrollRegionId::Changes,
            offset,
            lines.len(),
            columns.viewport,
            &app.theme_styles,
        );
    }
}

/// The file's text when it is here and is text; otherwise draws the line saying why not.
fn loaded_text_or_notice<'a>(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    view: &'a ChangesView,
) -> Option<&'a ProjectFile> {
    let styles = &app.theme_styles;
    let file = match &view.text {
        TextLoad::Ready(file) => file,
        other => {
            let (text, style) = match other {
                TextLoad::Failed(error) => (sentence(&error.to_string()), styles.warning),
                _ => ("Reading the file…".to_string(), styles.muted),
            };
            view.scroll_max.set(0);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))).wrap(Wrap { trim: true }),
                area,
            );
            return None;
        }
    };
    if file.binary {
        view.scroll_max.set(0);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "This file is not text, so there are no lines to show. Open it in your editor.",
                styles.warning,
            )))
            .wrap(Wrap { trim: true }),
            area,
        );
        return None;
    }
    Some(file)
}

/// How the body divides its width, and how many rows it shows.
struct TextColumns {
    /// Digits the line numbers take
    numbers: usize,
    gutter: usize,
    code: usize,
    width: u16,
    viewport: usize,
}

/// Publishes the largest offset that still puts the last line on screen, and returns this frame's
/// offset clamped to it.
fn text_window(view: &ChangesView, lines: &[&str], columns: &TextColumns) -> usize {
    let height = |index: usize| -> usize {
        match view.word_wrap {
            true => crate::tabstop::display_width(lines[index]).div_ceil(columns.code).max(1),
            false => 1,
        }
    };
    // The largest offset that still puts the last line on screen, measured from the end only
    let mut used = 0usize;
    let mut max = 0usize;
    for index in (0..lines.len()).rev() {
        used += height(index);
        if used > columns.viewport {
            max = index + 1;
            break;
        }
    }
    view.scroll_max.set(max);
    view.scroll.min(max)
}

/// The rows on screen, what a click on each means, and where each row's code sits.
#[derive(Default)]
struct TextFrame {
    rows: Vec<Line<'static>>,
    hits: Vec<(usize, MouseTarget)>,
    /// One entry per row: the code it shows, or nothing for the action row and the closing note
    cells: Vec<Vec<CodeCell>>,
}

impl TextFrame {
    /// Adds a row that holds no code.
    fn push_plain(&mut self, row: Line<'static>) {
        self.rows.push(row);
        self.cells.push(Vec::new());
    }
}

/// The rows on screen from `offset`: numbered lines, the action row under a pick, and a note when
/// the file goes on past what was read.
fn text_rows(
    app: &App,
    view: &ChangesView,
    file: &ProjectFile,
    lines: &[&str],
    offset: usize,
    columns: &TextColumns,
) -> TextFrame {
    let (gutter, viewport) = (columns.gutter, columns.viewport);
    let picked = view.selected_lines();
    let context = LineContext {
        language: view.path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()),
        added: added_lines(view),
        picked,
    };
    let mut frame = TextFrame::default();
    for (index, line) in lines.iter().enumerate().skip(offset) {
        if frame.rows.len() >= viewport {
            break;
        }
        push_line_rows(&mut frame, app, view, (index, line), &context, columns);
        let closes_pick = picked.is_some_and(|(_, last)| last as usize == index + 1);
        if closes_pick && !app.pointer.peek_dragging && frame.rows.len() < viewport {
            let (row, regions) = action_row(app, view, gutter);
            let at = frame.rows.len();
            frame.hits.extend(regions.into_iter().map(|(_, target)| (at, target)));
            frame.push_plain(row);
        }
    }
    if file.truncated && frame.rows.len() < viewport && offset + frame.rows.len() >= lines.len() {
        frame.push_plain(Line::from(Span::styled(
            format!(
                "{}… the file goes on past {} — open it in your editor for the rest",
                " ".repeat(gutter),
                human_bytes(file.text.len() as u64)
            ),
            app.theme_styles.muted,
        )));
    }
    frame
}

/// What every numbered line is drawn against.
struct LineContext {
    language: Option<String>,
    added: HashSet<u32>,
    picked: Option<(u32, u32)>,
}

/// One numbered line as the rows it wraps to, each with its gutter, its tint and its code.
fn push_line_rows(
    frame: &mut TextFrame,
    app: &App,
    view: &ChangesView,
    (index, line): (usize, &str),
    context: &LineContext,
    columns: &TextColumns,
) {
    let styles = &app.theme_styles;
    let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
    let in_pick = context.picked.is_some_and(|(first, last)| (first..=last).contains(&number));
    // The keyboard's line, while the keyboard is here: its number lit and a mark beside it
    let standing = view.focused && view.cursor == Some(number);
    let lead = line_lead(app, (index, number), standing, in_pick, context, columns.numbers);
    let segments = source_row_segments(
        line,
        context.language.as_deref(),
        columns.code,
        view.word_wrap,
        styles,
    );
    let pieces = code_pieces(
        line,
        index,
        crate::mouse::PeekSide::Whole,
        (columns.gutter, columns.code),
        view.word_wrap,
    );
    for (segment_index, segment) in segments.into_iter().enumerate() {
        if frame.rows.len() >= columns.viewport {
            break;
        }
        let mut row = match segment_index {
            0 => lead.clone(),
            _ => vec![Span::styled(
                format!("{}{} ", " ".repeat(columns.gutter - 2), crate::glyphs::Mark::Continuation),
                styles.dim,
            )],
        };
        row.extend(segment);
        frame.hits.push((frame.rows.len(), MouseTarget::PeekLine(number)));
        frame.cells.push(pieces.get(segment_index).copied().into_iter().collect());
        frame.rows.push(match (in_pick, standing) {
            (true, _) => paint(row, true, columns.width, styles.pick_bg()),
            (false, true) => paint(row, true, columns.width, styles.cursor_line_bg()),
            (false, false) => paint(row, false, columns.width, styles.cursor_line_bg()),
        });
    }
}

/// The line number, the keyboard's mark beside it, and the change bar.
fn line_lead(
    app: &App,
    (index, number): (usize, u32),
    standing: bool,
    in_pick: bool,
    context: &LineContext,
    numbers: usize,
) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    let number_style = match (standing, in_pick) {
        (true, _) => styles.accent.add_modifier(ratatui::style::Modifier::BOLD),
        (false, true) => styles.text,
        (false, false) => styles.dim,
    };
    vec![
        Span::styled(
            format!("{:>numbers$}{}", index + 1, if standing { "›" } else { " " }),
            number_style,
        ),
        Span::styled(if context.added.contains(&number) { "▎ " } else { "  " }, styles.success),
    ]
}

/// One source line highlighted and either wrapped to the code column or clipped to it, marked
/// where it runs past the edge.
fn source_row_segments(
    line: &str,
    language: Option<&str>,
    code: usize,
    word_wrap: bool,
    styles: &crate::theme::styles::ThemeStyles,
) -> Vec<Vec<Span<'static>>> {
    let source = crate::tabstop::display_text(line);
    let spans = highlighted_source_spans(&source, language, styles);
    match word_wrap {
        true => crate::widgets::wrap_cells(spans, code),
        false => {
            let clipped = crate::tabstop::display_width(line) > code;
            let mut kept = crate::view::diff_lines::clip_styled_spans(
                &spans.iter().map(|span| (span.content.as_ref(), span.style)).collect::<Vec<_>>(),
                0,
                if clipped { code - 1 } else { code },
            )
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect::<Vec<_>>();
            if clipped {
                kept.push(Span::styled("…", styles.dim));
            }
            vec![kept]
        }
    }
}

/// Publishes each row's click: the line it shows, or the action whose words it carries.
///
/// A line's click covers only its gutter: the code beside it is selectable text, and a press there
/// has to start a selection rather than a line pick.
fn publish_text_hits(
    app: &App,
    rows: &[Line<'static>],
    hits: Vec<(usize, MouseTarget)>,
    body: Rect,
    gutter: usize,
) {
    let gutter = u16::try_from(gutter).unwrap_or(u16::MAX).min(body.width);
    let mut hit = app.hit.borrow_mut();
    for (row, target) in hits {
        let y = body.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= body.bottom() {
            continue;
        }
        match &target {
            MouseTarget::PeekSelection(action) => {
                if let Some((x, width)) = action_columns(&rows[row], *action) {
                    hit.push(Rect::new(body.x + x, y, width, 1), target);
                }
            }
            _ => hit.push(Rect::new(body.x, y, gutter, 1), target),
        }
    }
}

/// The new-file lines the change added, for the gutter bar.
fn added_lines(view: &ChangesView) -> HashSet<u32> {
    view.file()
        .map(|file| {
            file.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == ReviewLineKind::Addition)
                .filter_map(|line| line.new_line)
                .collect()
        })
        .unwrap_or_default()
}

/// A row, tinted across the pane's width while picked.
fn paint(spans: Vec<Span<'static>>, picked: bool, width: u16, tint: Style) -> Line<'static> {
    match picked {
        true => Line::from(crate::view::diff_lines::tint_row(spans, tint, width)),
        false => Line::from(spans),
    }
}

/// Labels of the action row, in order. Open in editor is not among them: the footer offers it
/// already, and it cannot carry the pick with it.
const ACTIONS: [(PeekSelectionAction, &str); 3] = [
    (PeekSelectionAction::Ask, "ask about these lines"),
    (PeekSelectionAction::Copy, "copy"),
    (PeekSelectionAction::Clear, "clear"),
];

/// `Alt+L ask about lines 12–30 · copy · clear`, under the last picked line.
fn action_row(
    app: &App,
    view: &ChangesView,
    gutter: usize,
) -> (Line<'static>, Vec<(usize, MouseTarget)>) {
    let styles = &app.theme_styles;
    let chord = app
        .keymap
        .chords_for(luxe_core::keymap::UiAction::AskAboutSelection)
        .and_then(|chords| chords.first())
        .map(ToString::to_string);
    let mut spans = vec![Span::raw(" ".repeat(gutter))];
    let mut regions = Vec::new();
    for (index, (action, label)) in ACTIONS.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", styles.border_muted));
        }
        let target = MouseTarget::PeekSelection(*action);
        let hovered = app.pointer.hover.as_ref() == Some(&target);
        if let (PeekSelectionAction::Ask, Some(chord)) = (action, chord.as_ref()) {
            spans.push(Span::styled(format!("{chord} "), styles.accent));
        }
        let label = match (action, view.selected_lines()) {
            (PeekSelectionAction::Ask, Some((first, last))) if first != last => {
                format!("ask about lines {first}–{last}")
            }
            (PeekSelectionAction::Ask, Some((line, _))) => format!("ask about line {line}"),
            _ => (*label).to_string(),
        };
        spans.push(Span::styled(label, if hovered { styles.text } else { styles.muted }));
        regions.push((0, target));
    }
    (Line::from(spans), regions)
}

/// Where one action's words sit in its rendered row, for its click region.
fn action_columns(row: &Line<'_>, action: PeekSelectionAction) -> Option<(u16, u16)> {
    let index = ACTIONS.iter().position(|(candidate, _)| *candidate == action)?;
    // Spans: indent, then per action an optional chord, its label, and a separator between
    let mut column = 0usize;
    let mut seen = 0usize;
    let mut spans = row.spans.iter().skip(1).peekable();
    while let Some(span) = spans.next() {
        let cells = span.content.width();
        if span.content == " · " {
            seen += 1;
            column += cells;
            continue;
        }
        if seen == index {
            let mut width = cells;
            // the chord and its label act together
            if let Some(next) = spans.peek().filter(|next| next.content != " · ") {
                width += next.content.width();
            }
            let start = row.spans[0].content.width() + column;
            return Some((u16::try_from(start).ok()?, u16::try_from(width).ok()?));
        }
        column += cells;
    }
    None
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        bytes if bytes >= 1024 * 1024 => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
        bytes if bytes >= 1024 => format!("{} KB", bytes / 1024),
        bytes => format!("{bytes} bytes"),
    }
}
