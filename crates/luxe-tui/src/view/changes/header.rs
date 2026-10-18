//! Changes header — the file's identity, its path and the view tabs above the diff.

use crate::app::App;
use crate::changes::{ChangedFile, ChangesView, PeekView};
use crate::mouse::{ChangesFooterAction, MouseTarget};
use crate::widgets::truncate_start as truncate_path_left;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// `M  src/app/toasts.rs   +6 −8            2 of 5`, then a rule.
///
/// The position counter is the part that makes the pane a browser rather than a dead end: it says
/// there are others and that stepping is a thing you can do, which is what the footer's bracket
/// keys then explain.
pub(super) fn render_header(frame: &mut Frame, area: Rect, app: &App, view: &ChangesView) {
    let line = header_line(app, view, area);
    frame.render_widget(Paragraph::new(line), Rect { height: 1, ..area });
    if area.height > 1 {
        frame.render_widget(
            Block::default().borders(Borders::TOP).border_style(app.theme_styles.border_muted),
            Rect { y: area.y + 1, height: 1, ..area },
        );
    }
}

/// Cells kept between the file's details and the FILE / DIFF switch, so its slab never touches them.
const SWITCH_GAP: usize = 2;
/// The least of a path worth showing: below this the position goes before the path does.
const PATH_FLOOR: usize = 8;

/// The header's one line, laid out so it fits: every fixed part is measured first and the path gets
/// what they leave. When even a stub of path will not fit, the position goes whole, never `8 of`.
fn header_line(app: &App, view: &ChangesView, area: Rect) -> Line<'static> {
    let width = usize::from(area.width);
    let entry = view.current();
    let lead = lead_span(app, view, entry);
    let details = detail_spans(app, view, entry);
    let tabs = match view.changed() {
        true => view_tabs(view.view, app),
        false => ViewTabs::default(),
    };
    let mut position = position_label(view);
    let fixed = |position: &str| {
        let tail = tabs.width() + position_cells(position);
        cells(std::slice::from_ref(&lead)) + cells(&details) + tail + SWITCH_GAP
    };
    if fixed(&position) + PATH_FLOOR > width {
        position.clear();
    }
    // Cut from the left: the tail of a path is the file, which is the thing being named. A file
    // outside the project arrives absolute and is said the way a person would say it
    let budget = width.saturating_sub(fixed(&position)).max(PATH_FLOOR);
    let path = truncate_path_left(&home_relative(&view.path), budget);
    let mut spans =
        vec![lead, Span::styled(path, app.theme_styles.text.add_modifier(Modifier::BOLD))];
    spans.extend(details);
    push_tail(&mut spans, (app, area), tabs, position);
    Line::from(spans)
}

/// `8 of 14`, or nothing for a single file.
fn position_label(view: &ChangesView) -> String {
    match (view.position(), view.files.len()) {
        (Some(index), total) if total > 1 => format!("{} of {total}", index + 1),
        _ => String::new(),
    }
}

/// The position's cells with the two that part it from the switch, or none when it is absent.
fn position_cells(position: &str) -> usize {
    match position.is_empty() {
        true => 0,
        false => position.width() + 2,
    }
}

fn cells(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum()
}

/// A changed file leads with its git letter; any other file with the mark of its language.
fn lead_span(app: &App, view: &ChangesView, entry: Option<&ChangedFile>) -> Span<'static> {
    let styles = &app.theme_styles;
    match entry {
        Some(file) => {
            Span::styled(format!("{} ", marker_of(file.status)), status_style(file.status, app))
        }
        None => {
            let extension = view.path.rsplit_once('.').map_or("", |(_, extension)| extension);
            let icon = crate::widgets::language_icon(&extension.to_ascii_lowercase());
            Span::styled(
                crate::glyphs::icon_label(icon.glyph(), ""),
                crate::widgets::language_hue(icon, styles),
            )
        }
    }
}

/// What follows the path: the File view's length (and that it was cut short), and `+6 −8`.
fn detail_spans(app: &App, view: &ChangesView, entry: Option<&ChangedFile>) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    let mut spans = Vec::new();
    if let (PeekView::File, crate::changes::TextLoad::Ready(file)) = (view.view, &view.text) {
        let lines = file.text.lines().count();
        let mut meta = format!("  {lines} line{}", if lines == 1 { "" } else { "s" });
        if file.truncated {
            meta.push_str(" · cut short");
        }
        spans.push(Span::styled(meta, styles.muted));
    }
    if let Some((additions, deletions)) = entry.and_then(|file| file.additions.zip(file.deletions))
    {
        spans.push(Span::styled(format!("  +{additions}"), styles.success));
        spans.push(Span::styled(format!(" −{deletions}"), styles.error));
    }
    spans
}

/// The switch and the position, pushed to the right edge with at least [`SWITCH_GAP`] before them,
/// and the hidden face of the switch published as the click that flips the view.
fn push_tail(
    spans: &mut Vec<Span<'static>>,
    (app, area): (&App, Rect),
    tabs: ViewTabs,
    position: String,
) {
    let tail = tabs.width() + position_cells(&position);
    if tail == 0 {
        return;
    }
    let used = cells(spans);
    let pad = usize::from(area.width).saturating_sub(used + tail).max(SWITCH_GAP);
    spans.push(Span::raw(" ".repeat(pad)));
    publish_switch(app, area, used + pad, tabs.other);
    spans.extend(tabs.spans);
    if !position.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(position, app.theme_styles.muted));
    }
}

/// Publishes the switch's hidden face, `other` cells into a switch drawn at column `at`, as the
/// click that flips the view.
fn publish_switch(app: &App, area: Rect, at: usize, other: Option<(usize, usize)>) {
    let Some((start, width)) = other else { return };
    if let (Ok(x), Ok(width)) = (u16::try_from(at + start), u16::try_from(width)) {
        let target = MouseTarget::ChangesFooter(ChangesFooterAction::ToggleView);
        app.hit.borrow_mut().push(Rect::new(area.x + x, area.y, width, 1), target);
    }
}

/// The FILE / DIFF switch, and where in it the face that is not showing sits.
#[derive(Default)]
struct ViewTabs {
    spans: Vec<Span<'static>>,
    /// `(start, cells)` of the hidden face, measured from the first span — the click target
    other: Option<(usize, usize)>,
}

impl ViewTabs {
    fn width(&self) -> usize {
        cells(&self.spans)
    }
}

/// `FILE DIFF`, both cut into badges: the showing face lit, the other on the idle surface.
///
/// Both in capitals and both slabs, so the pair reads as a switch with two positions rather than a
/// heading with a caption beside it. They are the same width, so neither label moves when the view
/// flips, and the hidden face lifts under the pointer to say it is the one a click reaches.
fn view_tabs(showing: PeekView, app: &App) -> ViewTabs {
    let styles = &app.theme_styles;
    let canvas = styles.background.bg.unwrap_or(styles.slab.canvas);
    let hovered = app.pointer.hover.as_ref()
        == Some(&MouseTarget::ChangesFooter(ChangesFooterAction::ToggleView));
    let mut tabs = ViewTabs::default();
    for (face, label) in [(PeekView::File, "FILE"), (PeekView::Diff, "DIFF")] {
        if !tabs.spans.is_empty() {
            tabs.spans.push(Span::raw(" "));
        }
        let start: usize = tabs.spans.iter().map(|span| span.content.width()).sum();
        let badge = match face == showing {
            true => {
                crate::widgets::badge(label, crate::theme::styles::Depth::Raised, canvas, styles)
            }
            false => crate::widgets::idle_badge(label, hovered, canvas, styles),
        };
        if face != showing {
            tabs.other = Some((start, badge.iter().map(|span| span.content.width()).sum()));
        }
        tabs.spans.extend(badge);
    }
    tabs
}

/// A path as a person would say it: `~/.config/ghostty/config`, and a project-relative path
/// unchanged.
fn home_relative(path: &str) -> String {
    if luxe_core::file_diff::is_external_path(path) {
        return luxe_core::paths::home_relative(std::path::Path::new(path));
    }
    path.to_string()
}

/// A typed error's message as a line a person reads.
///
/// Library errors are lowercase by convention because they are usually a clause inside a larger
/// sentence. Here the error *is* the sentence, and starting it in lower case reads as a fragment.
pub(super) fn sentence(message: &str) -> String {
    let mut characters = message.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + characters.as_str()
    })
}

fn marker_of(status: luxe_core::FileChangeStatus) -> &'static str {
    use luxe_core::FileChangeStatus as Status;
    match status {
        Status::Added => "A",
        Status::Modified => "M",
        Status::Deleted => "D",
        Status::Renamed => "R",
        Status::Untracked => "?",
    }
}

/// Git-status color, matching the dock rail the reader clicked from.
fn status_style(status: luxe_core::FileChangeStatus, app: &App) -> Style {
    use luxe_core::FileChangeStatus as Status;
    match status {
        Status::Added | Status::Untracked => app.theme_styles.success,
        Status::Modified => app.theme_styles.warning,
        Status::Deleted => app.theme_styles.error,
        Status::Renamed => app.theme_styles.info,
    }
}
