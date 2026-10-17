//! Changes pane rendering — one file's diff, where the transcript usually is.
//!
//! Three bands: a header naming the file and where it sits in the change, the diff, and a footer of
//! the keys that do something here. The diff is drawn from the review's own [`ReviewFile`] through
//! [`crate::view::diff_lines`], so a line looks the same in both surfaces — which matters, because
//! `^G` from this pane opens the review on the file the reader is already looking at.
//!
//! What it deliberately does not have: comments, chapters, folds, a source picker.
//! Those are the review, and the footer says where to find them.

mod cells;
mod diff;
mod footer;
mod header;
mod split;
mod text;

use crate::app::App;
use crate::changes::{ChangesLoad, ChangesView, PeekView};
use crate::mouse::{MouseTarget, ScrollRegionId};
pub(crate) use diff::line_in_view;
use diff::render_diff;
use footer::render_footer;
use header::{render_header, sentence};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Draws the pane into the column the conversation would have had.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let Some(view) = app.changes.as_ref() else {
        return;
    };
    if area.width < 8 || area.height < 3 {
        return;
    }
    // A rule over the footer, like the one under the header: straight after the last line of code
    // the keys read as one more line of it. And one under it, because the composer's own hints sit
    // right below, and two rows of keys with nothing between them read as one list of them.
    let foot = match area.height {
        7.. => 3,
        5..=6 => 2,
        _ => 1,
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1), Constraint::Length(foot)])
        .split(area);
    render_header(frame, rows[0], app, view);
    match view.view {
        PeekView::File => text::render(frame, rows[1], app, view),
        PeekView::Diff => render_body(frame, rows[1], app, view),
    }
    let footer = Rect { y: rows[2].y + u16::from(foot >= 2), height: 1, ..rows[2] };
    if foot >= 2 {
        frame.render_widget(
            Block::default().borders(Borders::TOP).border_style(app.theme_styles.border_muted),
            Rect { height: 1, ..rows[2] },
        );
    }
    if foot == 3 {
        frame.render_widget(
            Block::default().borders(Borders::TOP).border_style(app.theme_styles.border_muted),
            Rect { y: rows[2].y + 2, height: 1, ..rows[2] },
        );
    }
    // The footer labels the layout toggle by what it will *do*, so it has to be told what the body
    // decided — which depends on the width the body was handed, not on the preference alone.
    let side_by_side = splits_at(app, view, rows[1].width);
    render_footer(frame, footer, app, view, side_by_side);
    app.hit.borrow_mut().push_scroll(rows[1], ScrollRegionId::Changes);
    view.side_by_side.set(side_by_side);
    if let Some(file) = view.file() {
        view.two_sided.set(crate::guided_review::has_two_sides(file));
    }
}

/// Whether the diff draws side by side in a body of this width.
///
/// Also asked before the pane has a width at all, to size the column it gets beside the
/// conversation ([`super::peek_diff_column`]).
///
/// Between files there is nothing to measure — [`ChangesView::show`] drops the loaded diff and the
/// capture takes a frame or two to arrive — so this holds what the last frame resolved instead of
/// falling to `false`, which would make the footer's layout shortcut vanish and come back on
/// *every* file switch and reflow every hint to its right, a flicker rather than an honest change.
pub(super) fn splits_at(app: &App, view: &ChangesView, width: u16) -> bool {
    let Some(file) = view.file() else {
        return view.side_by_side.get();
    };
    // Mirrors what `render_diff` reserves: one column for the scrollbar, then two equal columns.
    let body = width.saturating_sub(u16::from(width > 2));
    let numbers = crate::guided_review::diff::gutter_width(&file.hunks);
    let layout = crate::view::diff_lines::split_layout(
        &app.theme_styles,
        body,
        numbers,
        0,
        file.language.as_deref(),
    );
    view.split_active(app.config.diff.layout(), layout.source, Some(file))
}

/// The diff, or the one line that explains why there is not one.
fn render_body(frame: &mut Frame, area: Rect, app: &App, view: &ChangesView) {
    view.viewport_rows.set(usize::from(area.height).max(1));
    let styles = &app.theme_styles;
    let notice: Option<(String, Style)> = match &view.load {
        ChangesLoad::Loading => Some(("Reading the change…".to_string(), styles.muted)),
        ChangesLoad::Unchanged => Some((
            "Nothing has changed in this file since the last commit.".to_string(),
            styles.muted,
        )),
        ChangesLoad::Failed(error) => Some((sentence(&error.to_string()), styles.warning)),
        ChangesLoad::Ready(file) if !file.is_text() => Some((
            file.non_text.as_ref().map_or_else(
                || "This file has no lines to show.".to_string(),
                |change| format!("{} — open the full review to work with it.", change.summary),
            ),
            styles.warning,
        )),
        ChangesLoad::Ready(file) => {
            render_diff(frame, area, app, view, file);
            None
        }
    };
    if let Some((text, style)) = notice {
        view.scroll_max.set(0);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))).wrap(Wrap { trim: true }),
            area,
        );
    }
}
