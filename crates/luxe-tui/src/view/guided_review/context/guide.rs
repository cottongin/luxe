//! The guide pane — the walkthrough document, where its focus sits, and what its rows open.

use super::super::brief;
use super::super::overlays::chapter_kind_label;
use super::foot::guide_foot_lines;
use super::metadata::review_metadata_lines;
use crate::app::App;
use crate::workspace_layout::gutter_inset;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

/// Rows the guide spends on one chapter's files before the rest become a count.
///
/// A chapter can own forty files. The reader wants the shape of it and their own place in it, and
/// the count says the rest is there — stepping with `Ctrl+↑/↓` walks every one of them.
const MAX_CHAPTER_FILE_ROWS: usize = 6;

/// Rows the head and the foot claim, so the document knows what is left.
const GUIDE_HEAD_ROWS: u16 = 1;

/// Rows a review's title may take before it is cut with an ellipsis.
pub(super) const TITLE_ROWS: usize = 2;

/// Rows kept below what the guide follows when it has to scroll to reach it, so the file being
/// read is never the last thing on the pane.
const GUIDE_SELECTION_MARGIN: usize = 1;

/// The stretch of the guide document the pane keeps in view, in unwrapped lines.
struct GuideFocus {
    /// The line the position is on: the hunk being read, else its file, else the chapter's title
    at: usize,
    /// The last line of the block it belongs to — the open file's last hunk row — so the list
    /// `Space` steps through is on screen whole when it fits
    through: usize,
}

/// The slice of files to show: `rows` of them, keeping the selection visible.
///
/// Beyond that it prefers to show what comes *after* the cursor, since that is where the reading
/// is going.
fn file_window(total: usize, selected: Option<usize>, rows: usize) -> (usize, usize) {
    if total <= rows {
        return (0, total);
    }
    let selected = selected.unwrap_or(0);
    // One row of context behind the cursor, the rest ahead.
    let start = selected.saturating_sub(1).min(total.saturating_sub(rows));
    (start, start + rows)
}

/// Draws the guide pane: identity, the document, and what the review wants from the reader.
pub(in crate::view::guided_review) fn render_guide_pane(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    // Raised surface, inset off the divider, matching the Context Dock's presentation
    let screen = frame.area();
    frame.render_widget(Block::default().style(app.theme_styles.tool_bg), area);
    let inner = gutter_inset(area, screen);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut head = review_metadata_lines(app, review, inner.width);
    let mut foot = guide_foot_lines(app, review, inner.width);
    // A rule under the head and over the foot, so the part that scrolls reads as its own zone. Each
    // takes the place of the blank row that would space them off, so it costs the document
    // nothing
    let head_rule = head.last().is_some_and(|line| line.width() == 0);
    let foot_rule = foot.first().is_some_and(|line| line.width() == 0);
    if head_rule {
        head.pop();
    }
    if foot_rule {
        foot.remove(0);
    }
    let (head_rule, foot_rule) = (u16::from(head_rule), u16::from(foot_rule));
    let head_height = wrapped_height(&head, inner.width).min(inner.height);
    let foot_height = wrapped_height(&foot, inner.width)
        .min(inner.height.saturating_sub(head_height + head_rule + foot_rule));
    let zones = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(head_height),
            Constraint::Length(head_rule),
            Constraint::Min(GUIDE_HEAD_ROWS),
            Constraint::Length(foot_rule),
            Constraint::Length(foot_height),
        ])
        .split(inner);

    let rule = |zone: Rect| {
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(usize::from(zone.width)),
            app.theme_styles.border_muted,
        )))
    };
    frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: false }), zones[0]);
    // A zone with no rule in it has no height, so drawing it there draws nothing
    frame.render_widget(rule(zones[1]), zones[1]);
    render_guide_document(frame, zones[2], app, review);
    frame.render_widget(rule(zones[3]), zones[3]);
    if foot_height > 0 {
        frame.render_widget(Paragraph::new(foot).wrap(Wrap { trim: false }), zones[4]);
    }
}

/// Rows a block of lines occupies once wrapped to `width`.
fn wrapped_height(lines: &[Line<'static>], width: u16) -> u16 {
    lines
        .iter()
        .map(|line| {
            Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width) as u16
        })
        .sum()
}

/// The document: the change's intent, then a block per chapter with its own prose and files.
///
/// Scrolls to keep the selection in view and holds no scroll state to do it. The offset is a
/// function of the position and the pane height, recomputed every frame, so there is nothing to
/// fall out of step with the cursor and nothing for a wheel to fight.
fn render_guide_document(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // The bar's lane is reserved whether or not it is drawn: appearing and disappearing as content
    // crosses the viewport would reflow every line of prose beside it while the reader scrolled.
    let text = Rect { width: area.width.saturating_sub(1), ..area };
    if text.width == 0 {
        return;
    }
    let width = usize::from(text.width);
    let (lines, anchors, focus) = guide_document(app, review, width);
    // Wrapped here rather than by the widget, so a rendered row and a document line mean the same
    // thing — which is what lets the scroll offset count what the reader can see and a click land
    // on the row under the pointer. One pass, recording where each line landed: measuring every
    // height and then wrapping again did the same work twice on every frame.
    let mut rows: Vec<Line<'static>> = Vec::with_capacity(lines.len() + lines.len() / 4);
    let mut starts: Vec<usize> = Vec::with_capacity(lines.len() + 1);
    for line in &lines {
        starts.push(rows.len());
        rows.extend(crate::widgets::wrap_line(line, width));
    }
    starts.push(rows.len());

    let height = usize::from(area.height);
    let total = rows.len();
    let last = total.saturating_sub(height.min(total));
    // Scroll only as far as it takes to keep the selection in view. Centering it instead would
    // push the change's own intent off the top the moment a review opened, which is the one thing
    // on this surface that has to be read first. A block taller than the pane gives way to the
    // line the position is actually on.
    let through = starts[focus.through + 1] + GUIDE_SELECTION_MARGIN;
    let following = through.saturating_sub(height).min(starts[focus.at]).min(last);
    let workspace = &app.guided_review_workspace;
    // A cursor that has moved since the wheel was used takes the guide back: a pinned offset that
    // outlived the reason it was pinned is a pane stuck somewhere nobody is looking.
    let cursor = workspace.position.fingerprint();
    if workspace.guide_scroll_anchor.get() != cursor {
        workspace.guide_scroll.set(None);
        workspace.guide_scroll_anchor.set(cursor);
    }
    let offset = match workspace.guide_scroll.get() {
        Some(pinned) => {
            let clamped = pinned.min(last);
            workspace.guide_scroll.set(Some(clamped));
            clamped
        }
        None => following,
    };
    workspace.guide_drawn_offset.set(offset);
    workspace.guide_room.set(last);

    {
        let mut regions = app.hit.borrow_mut();
        for anchor in &anchors {
            let Some(visible) = starts[anchor.line].checked_sub(offset).filter(|row| *row < height)
            else {
                continue;
            };
            let row_height = starts[anchor.line + 1]
                .saturating_sub(starts[anchor.line])
                .max(1)
                .min(height - visible);
            regions.push(
                Rect::new(text.x, text.y + visible as u16, text.width, row_height as u16),
                anchor.target.clone(),
            );
        }
    }
    rows.drain(..offset.min(rows.len()));
    rows.truncate(height);
    frame.render_widget(Paragraph::new(rows), text);
    crate::widgets::steered_scrollbar(
        frame,
        &mut app.hit.borrow_mut(),
        Rect { x: area.x + area.width.saturating_sub(1), width: 1, ..area },
        crate::mouse::ScrollRegionId::ReviewGuide,
        offset,
        total,
        height,
        &app.theme_styles,
    );
}

/// A document row a click can land on.
struct GuideAnchor {
    /// Index into the unwrapped document lines.
    line: usize,
    /// What clicking it means.
    target: crate::mouse::MouseTarget,
}

/// The guide document's lines, the rows a pointer can act on, and what the pane follows.
///
/// It follows the most specific thing the position names, with the block around it. Following the
/// chapter's title alone let a chapter with a few paragraphs of prose walk its files off the bottom
/// of the pane while `Space` stepped through them.
fn guide_document(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    width: usize,
) -> (Vec<Line<'static>>, Vec<GuideAnchor>, GuideFocus) {
    let mut doc = GuideDocument {
        lines: brief::intent_lines(app, review, width),
        anchors: Vec::new(),
        focus: GuideFocus { at: 0, through: 0 },
    };
    let owners = brief::FileOwners::of(review);
    for (index, chapter) in review.chapters.iter().enumerate() {
        chapter_document(&mut doc, app, review, &owners, (index, chapter), width);
    }
    (doc.lines, doc.anchors, doc.focus)
}

/// The guide document as it is built: its lines, what its rows open, and where the reading is.
struct GuideDocument {
    lines: Vec<Line<'static>>,
    anchors: Vec<GuideAnchor>,
    focus: GuideFocus,
}

/// One chapter's part of the guide: its number and kind, its title, then either the one line that
/// accounts for it or its rationale and its files.
fn chapter_document(
    doc: &mut GuideDocument,
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    owners: &brief::FileOwners<'_>,
    (index, chapter): (usize, &luxe_core::guided_review::ReviewChapter),
    width: usize,
) {
    let styles = &app.theme_styles;
    let lines = &mut doc.lines;
    let current = app.guided_review_workspace.position.chapter_id.as_ref() == Some(&chapter.id);
    let roll = app.review.budget.chapter(&chapter.id);
    let accounted = roll.needs_review == 0;
    lines.push(Line::from(vec![
        Span::styled(format!("{:02} / {:02}   ", index + 1, review.chapters.len()), styles.dim),
        Span::styled(chapter_kind_label(chapter.kind).to_uppercase(), styles.dim),
        Span::styled(
            if app.guided_review_workspace.is_chapter_walked(&chapter.id) {
                "   reviewed"
            } else {
                ""
            },
            styles.success,
        ),
    ]));
    if current {
        doc.focus = GuideFocus { at: lines.len(), through: lines.len() };
    }
    doc.anchors.push(GuideAnchor {
        line: lines.len(),
        target: crate::mouse::MouseTarget::ReviewGuideChapter(chapter.id.clone()),
    });
    lines.push(Line::from(vec![
        Span::styled(if current { "\u{203a} " } else { "  " }, styles.accent),
        Span::styled(
            chapter.title.clone(),
            if accounted {
                styles.muted
            } else if current {
                styles.accent.add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                styles.accent
            },
        ),
    ]));
    // An accounted-for chapter recedes to one line and says what accounts for it. It is still
    // reachable — `]` steps into it like any other — it just does not spend the page.
    if accounted {
        lines.push(Line::from(Span::styled(
            format!("    {}", chapter_accounted_label(app, review, chapter)),
            styles.dim,
        )));
        lines.push(Line::default());
        return;
    }
    lines.extend(brief::prose_lines(&chapter.rationale, app, width, "    "));
    if let Some(impact) = chapter.impact.as_deref() {
        lines.extend(brief::prose_lines(impact, app, width, "    \u{2192} "));
    }
    lines.push(Line::default());
    chapter_file_rows(doc, app, &owners.chapter_files(chapter), current, width);
    doc.lines.push(Line::default());
}

/// A chapter's files, windowed to [`MAX_CHAPTER_FILE_ROWS`] around the one being read, which
/// opens to its hunks.
fn chapter_file_rows(
    doc: &mut GuideDocument,
    app: &App,
    files: &[&luxe_core::guided_review::ReviewFile],
    current: bool,
    width: usize,
) {
    let styles = &app.theme_styles;
    let lines = &mut doc.lines;
    let selected_file = app.guided_review_workspace.position.file_id.as_ref();
    let selected_index =
        current.then(|| files.iter().position(|file| Some(&file.id) == selected_file)).flatten();
    let (start, end) = file_window(files.len(), selected_index, MAX_CHAPTER_FILE_ROWS);
    if start > 0 {
        lines.push(Line::from(Span::styled(format!("    \u{2026} {start} above"), styles.dim)));
    }
    for (position, file) in files.iter().enumerate().take(end).skip(start) {
        let open = current && selected_index == Some(position);
        doc.anchors.push(GuideAnchor {
            line: lines.len(),
            target: crate::mouse::MouseTarget::ReviewGuideFile(file.id.clone()),
        });
        let file_line = lines.len();
        lines.push(brief::file_line(app, file, open, width));
        if !open {
            continue;
        }
        // Only the file being read opens to its hunks. That is what makes stepping legible:
        // the mark moves down a list you can see, on every press.
        let (rows, active) = hunk_rows(app, file, width);
        doc.focus = GuideFocus {
            at: active.map_or(file_line, |active| file_line + 1 + active),
            through: file_line + rows.len(),
        };
        lines.extend(rows);
    }
    if end < files.len() {
        lines.push(Line::from(Span::styled(
            format!("    \u{2026} {} more", files.len() - end),
            styles.dim,
        )));
    }
}

/// One row per hunk in the file being read, with the active one marked, and which row that is.
fn hunk_rows(
    app: &App,
    file: &luxe_core::guided_review::ReviewFile,
    width: usize,
) -> (Vec<Line<'static>>, Option<usize>) {
    let styles = &app.theme_styles;
    let active = app.guided_review_workspace.position.hunk_id.as_ref();
    let shown: Vec<&luxe_core::guided_review::ReviewHunk> = file
        .hunks
        .iter()
        .filter(|hunk| !app.guided_review_workspace.is_hunk_hidden(file, hunk))
        .collect();
    let at = shown.iter().position(|hunk| active == Some(&hunk.id));
    let rows = shown
        .into_iter()
        .map(|hunk| {
            let current = active == Some(&hunk.id);
            let label = hunk
                .symbol
                .as_ref()
                .and_then(|symbol| symbol.name.clone())
                .or_else(|| hunk.heading.clone())
                .unwrap_or_else(|| format!("line {}", hunk.new_range.start));
            Line::from(vec![
                Span::styled(if current { "     \u{2503} " } else { "       " }, styles.accent),
                Span::styled(
                    crate::guided_review::diff::clip_terminal_cells_ellipsis(
                        &label,
                        width.saturating_sub(8),
                    ),
                    if current { styles.text.patch(styles.selected_bg) } else { styles.muted },
                ),
            ])
        })
        .collect();
    (rows, at)
}

/// What accounts for a chapter nothing in which needs the reviewer.
fn chapter_accounted_label(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
    chapter: &luxe_core::guided_review::ReviewChapter,
) -> String {
    let _ = review;
    luxe_core::guided_review::shared_reason(&app.review.budget, chapter).map_or_else(
        || "nothing here needs you".into(),
        |accounted| format!("{} \u{2014} nothing here needs you", accounted.label()),
    )
}
