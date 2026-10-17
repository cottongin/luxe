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
    let styles = &app.theme_styles;
    let entry = view.current();
    let width = usize::from(area.width);

    let position = match (view.position(), view.files.len()) {
        (Some(index), total) if total > 1 => format!("{} of {total}", index + 1),
        _ => String::new(),
    };
    let counts = entry.and_then(count_label).unwrap_or_default();
    // Give the path whatever the fixed parts leave, and cut it from the left: the tail of a path
    // is the file, which is the thing being named. A file outside the project arrives absolute and
    // is said the way a person would say it, which is also what tells the reader at a glance that
    // they are not looking at project code.
    let reserved = position.chars().count() + counts.chars().count() + 6;
    let titled = home_relative(&view.path);
    let path = truncate_path_left(&titled, width.saturating_sub(reserved).max(8));

    // A changed file leads with its git letter; any other file with the mark of its language
    let lead = match entry {
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
    };
    let mut spans = vec![lead];
    spans.push(Span::styled(path, styles.text.add_modifier(Modifier::BOLD)));
    // What the File view is showing: how long it is, and that it was cut short when it was
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
    // A changed file has two faces, and the one not showing is a click away
    let tabs = match view.changed() {
        true => view_tabs(view.view, app),
        false => ViewTabs::default(),
    };
    let tabs_width: usize = tabs.spans.iter().map(|span| span.content.width()).sum();
    let used: usize = spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
    let tail = tabs_width + position.chars().count() + usize::from(!position.is_empty()) * 2;
    if tail > 0 {
        let pad = width.saturating_sub(used + tail);
        spans.push(Span::raw(" ".repeat(pad)));
        let tabs_at = used + pad;
        if let Some((start, cells)) = tabs.other {
            if let (Ok(x), Ok(cells)) = (u16::try_from(tabs_at + start), u16::try_from(cells)) {
                app.hit.borrow_mut().push(
                    Rect::new(area.x + x, area.y, cells, 1),
                    MouseTarget::ChangesFooter(ChangesFooterAction::ToggleView),
                );
            }
        }
        spans.extend(tabs.spans);
        if !position.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(position, styles.muted));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), Rect { height: 1, ..area });
    if area.height > 1 {
        frame.render_widget(
            Block::default().borders(Borders::TOP).border_style(styles.border_muted),
            Rect { y: area.y + 1, height: 1, ..area },
        );
    }
}

/// The FILE / DIFF switch, and where in it the face that is not showing sits.
#[derive(Default)]
struct ViewTabs {
    spans: Vec<Span<'static>>,
    /// `(start, cells)` of the hidden face, measured from the first span — the click target
    other: Option<(usize, usize)>,
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

/// `+6 −8`, or nothing at all when git could not count (a binary blob).
fn count_label(file: &ChangedFile) -> Option<String> {
    let (additions, deletions) = file.additions.zip(file.deletions)?;
    Some(format!("  +{additions} −{deletions}"))
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
