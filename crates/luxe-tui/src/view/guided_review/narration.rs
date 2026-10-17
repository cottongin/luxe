//! The narration band — this change's own sentence, above the diff that shows it.
//!
//! What the reader is looking at, and what it does: `path · symbol · hunk 2 of 2`, the guide's
//! sentence for *this* hunk, and the graph facts that change a judgement about the file. It changes
//! on every step, which is what makes advancing a visible event rather than a redraw of the same
//! frame with different code in it.
//!
//! The chapter's own prose is deliberately absent: it lives in the guide pane, beside the file list
//! it introduces. Two granularities, one home each — saying the chapter's rationale here as well
//! would spend the diff's rows repeating what is already on screen.
//!
//! Bounded on purpose. `z` folds it away for a reviewer who wants every row of code.

use crate::app::App;
use luxe_core::guided_review::{GuidedReview, ReviewChapter, ReviewItemRef};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Rows this item's sentence may occupy before it is clipped.
const MAX_ITEM_ROWS: u16 = 3;

/// The narration for the current position, or `None` when there is nothing to say.
pub(super) fn narration_lines(
    app: &App,
    review: &GuidedReview,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    if !app.guided_review_workspace.narration_visible || width < 40 {
        return None;
    }
    let chapter = current_chapter(app, review)?;
    let styles = &app.theme_styles;
    let mut lines: Vec<Line<'static>> = Vec::new();

    match current_item_note(app, review, chapter) {
        Some((heading, note)) => {
            lines.push(Line::from(item_heading_spans(heading, styles)));
            lines.extend(wrapped(&note, width, styles.text, MAX_ITEM_ROWS));
        }
        None => {
            let heading = item_heading(app, review)?;
            lines.push(Line::from(item_heading_spans(heading, styles)));
        }
    }
    lines.extend(current_file_attention_lines(app, review, styles));
    (!lines.is_empty()).then_some(lines)
}

/// `path   symbol   hunk 2 of 3` — what the diff below belongs to.
fn item_heading_spans(
    heading: ItemHeading,
    styles: &crate::theme::styles::ThemeStyles,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(heading.path, styles.info)];
    if let Some(symbol) = heading.symbol {
        spans.push(Span::styled("   ", styles.dim));
        spans.push(Span::styled(symbol, styles.text.add_modifier(ratatui::style::Modifier::BOLD)));
    }
    if let Some((at, total)) = heading.hunk {
        spans.push(Span::styled(format!("   hunk {at} of {total}"), styles.muted));
    }
    spans
}

/// What the heading names.
struct ItemHeading {
    /// Project-relative path of the file being read.
    path: String,
    /// The symbol the hunk sits in, when the diff named one.
    symbol: Option<String>,
    /// `(at, total)` hunks in this file, when the cursor is on one.
    hunk: Option<(usize, usize)>,
}

/// The heading for the current position, or `None` when there is no file to name.
fn item_heading(app: &App, review: &GuidedReview) -> Option<ItemHeading> {
    let position = &app.guided_review_workspace.position;
    let file =
        position.file_id.as_ref().and_then(|id| review.files.iter().find(|file| &file.id == id))?;
    let path = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map(|path| path.display().to_string())?;
    let index =
        position.hunk_id.as_ref().and_then(|id| file.hunks.iter().position(|hunk| &hunk.id == id));
    let symbol = index.and_then(|index| file.hunks.get(index)).and_then(|hunk| {
        hunk.symbol.as_ref().and_then(|symbol| symbol.name.clone()).or_else(|| hunk.heading.clone())
    });
    Some(ItemHeading { path, symbol, hunk: index.map(|index| (index + 1, file.hunks.len())) })
}

/// What the harness noticed about the file on screen, said where the reviewer is already reading.
///
/// The Brief ranks these too, but a list you consult before starting is not where a concern gets
/// resolved — here it sits under the prose that should have explained it, next to the diff that
/// shows it, one `c` away from becoming a comment the agent has to answer.
fn current_file_attention_lines(
    app: &App,
    review: &GuidedReview,
    styles: &crate::theme::styles::ThemeStyles,
) -> Vec<Line<'static>> {
    let Some(path) = current_path(app, review) else {
        return Vec::new();
    };
    let mut signals: Vec<&luxe_core::guided_review::AttentionSignal> = app
        .review
        .flags
        .iter()
        .filter(|signal| signal.path.as_deref() == Some(path.as_str()))
        .collect();
    // one line per reason: the flags keep a signal per flagged line, and two credential lines in
    // one file are one thing to say
    signals.dedup_by(|left, right| left.kind == right.kind && left.detail == right.detail);
    signals
        .into_iter()
        .take(MAX_ATTENTION_ROWS)
        .map(|signal| {
            // Strip the path the heading right above already carries; what is left continues it.
            let detail = signal
                .detail
                .strip_prefix(&format!("{path}: "))
                .or_else(|| signal.detail.strip_prefix(&format!("{path} ")))
                .unwrap_or(&signal.detail)
                .to_owned();
            Line::from(vec![
                Span::styled("   · ", styles.warning),
                Span::styled(detail, styles.warning),
            ])
        })
        .collect()
}

/// Attention rows one section's file may add before the prose stops being prose.
const MAX_ATTENTION_ROWS: usize = 2;

/// The project-relative path of the file being read.
fn current_path(app: &App, review: &GuidedReview) -> Option<String> {
    let file = app
        .guided_review_workspace
        .position
        .file_id
        .as_ref()
        .and_then(|id| review.files.iter().find(|file| &file.id == id))?;
    file.new_path.as_deref().or(file.old_path.as_deref()).map(|path| path.display().to_string())
}

/// The chapter the narration describes: the one the cursor is in.
fn current_chapter<'a>(app: &App, review: &'a GuidedReview) -> Option<&'a ReviewChapter> {
    let selected = app.guided_review_workspace.position.chapter_id.as_ref()?;
    review.chapters.iter().find(|chapter| &chapter.id == selected)
}

/// `(file · symbol, note)` for the selected item, when the guide wrote a sentence for it.
fn current_item_note(
    app: &App,
    review: &GuidedReview,
    chapter: &ReviewChapter,
) -> Option<(ItemHeading, String)> {
    let position = &app.guided_review_workspace.position;
    let file =
        position.file_id.as_ref().and_then(|id| review.files.iter().find(|file| &file.id == id))?;
    let note = match position.hunk_id.as_ref() {
        Some(hunk_id) => chapter.note_for_hunk(hunk_id),
        None => chapter.note_for(&ReviewItemRef::File(file.id.clone())),
    }?;
    Some((item_heading(app, review)?, note.to_owned()))
}

/// Soft-wraps prose to `width`, clipped to `max_rows` with an ellipsis on the last kept row.
fn wrapped(text: &str, width: u16, style: Style, max_rows: u16) -> Vec<Line<'static>> {
    let width = usize::from(width).max(20);
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate =
            if current.is_empty() { word.to_owned() } else { format!("{current} {word}") };
        if candidate.width() > width && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            current = word.to_owned();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    let clipped = rows.len() > usize::from(max_rows);
    rows.truncate(usize::from(max_rows));
    if clipped {
        if let Some(last) = rows.last_mut() {
            let budget = width.saturating_sub(1);
            while last.width() > budget {
                last.pop();
            }
            last.push('…');
        }
    }
    rows.into_iter().map(|row| Line::from(Span::styled(row, style))).collect()
}
