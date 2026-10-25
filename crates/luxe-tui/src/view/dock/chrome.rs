//! Dock chrome — section headings, status marks and the plain row helpers every section uses.

use crate::app::App;
use crate::theme::styles::Depth;
use crate::workspace_dock::DockChanges;
use luxe_core::config::StatuslineStyle;
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

/// The dock's marker and color for `status`, for another surface that shows a file's Git status
/// and must show it the same way.
pub(crate) fn status_mark(
    status: luxe_core::workspace::FileChangeStatus,
    theme: &crate::theme::styles::ThemeStyles,
) -> (&'static str, ratatui::style::Style) {
    (crate::marks::change(status), status_style(status, theme))
}

fn status_style(
    status: luxe_core::workspace::FileChangeStatus,
    theme: &crate::theme::styles::ThemeStyles,
) -> ratatui::style::Style {
    use luxe_core::workspace::FileChangeStatus as Status;
    match status {
        Status::Added | Status::Untracked => theme.success,
        Status::Modified => theme.warning,
        Status::Deleted => theme.error,
        Status::Renamed => theme.info,
    }
}

pub(super) fn plural_files(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

/// The whole-worktree total, dimmed under the headline.
pub(super) fn change_summary_line(changes: &DockChanges, app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} in the worktree · ", plural_files(changes.total_files)),
            app.theme_styles.dim.patch(app.theme_styles.tool_bg),
        ),
        Span::styled(
            format!("+{}", change_count(changes.additions)),
            app.theme_styles.success.patch(app.theme_styles.tool_bg),
        ),
        Span::styled(
            format!(" -{}", change_count(changes.deletions)),
            app.theme_styles.error.patch(app.theme_styles.tool_bg),
        ),
    ])
}

fn change_count(count: Option<u64>) -> String {
    count.map_or_else(|| "?".into(), |count| count.to_string())
}

/// A section of the rail, and the surface its heading badge is cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Project,
    Plan,
    Files,
    Elsewhere,
    Model,
    Usage,
    Session,
}

impl Section {
    /// The word on the badge.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Project => "PROJECT",
            Self::Plan => "PLAN",
            Self::Files => "FILES",
            Self::Elsewhere => "ELSEWHERE",
            Self::Model => "MODEL",
            Self::Usage => "USAGE",
            Self::Session => "SESSION",
        }
    }

    /// How far this section's badge has receded from the accent.
    ///
    /// The bar's rule stood on end. The accent undiluted at both anchors — PROJECT pinned to the top
    /// of the rail and MODEL pinned to the bottom beside the composer, the same two facts the bar
    /// anchors itself with — and everything between them receding: the work in flight one step back,
    /// the ambient accounting two.
    ///
    /// Depth belongs to the section rather than to where it landed. ELSEWHERE comes and goes with
    /// the session, so a positional ramp would re-shade the whole rail every time it appeared.
    const fn depth(self) -> Depth {
        match self {
            Self::Project | Self::Model => Depth::Hero,
            Self::Plan | Self::Files => Depth::Raised,
            Self::Elsewhere | Self::Usage | Self::Session => Depth::Recessed,
        }
    }
}

/// Heading for a dock section: its label cut into a slab, at the section's own depth.
///
/// The badge hugs its word rather than spanning the rail. A slab across a whole row already means
/// *chosen* everywhere else in the product ([`crate::widgets::selection_row`]), and at forty cells
/// the chamfer is a fortieth of the shape and stops reading as one. Its leading edge sits on the
/// column its body rows start at, so a section reads as one block.
///
/// `statusline.style = "flat"` draws the label the way the rail always did — bold accent text on the
/// panel — because the shape is one look across both forms of this surface.
pub(super) fn section_heading_line(section: Section, app: &App) -> Line<'static> {
    Line::from(heading_spans(section, app))
}

/// A heading with `trailing` riding bare beside the badge.
///
/// The badge stays one word on every section, so the run of them down the rail reads as structure;
/// what the heading summarizes follows it on the canvas. Same division the bar makes between a
/// module's pill and the strings that ride bare on it.
pub(super) fn section_heading_with(section: Section, trailing: String, app: &App) -> Line<'static> {
    let styles = &app.theme_styles;
    let mut spans = heading_spans(section, app);
    spans.push(Span::styled(format!(" {trailing}"), styles.muted.patch(styles.tool_bg)));
    Line::from(spans)
}

pub(super) fn heading_spans(section: Section, app: &App) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    if app.config.statusline.style() == StatuslineStyle::Flat {
        return vec![Span::styled(
            section.label(),
            styles.accent.add_modifier(Modifier::BOLD).patch(styles.tool_bg),
        )];
    }
    let canvas = styles.tool_bg.bg.unwrap_or(styles.slab.canvas);
    crate::widgets::badge(section.label(), section.depth(), canvas, styles)
}

pub(super) fn text_line(text: String, app: &App) -> Line<'static> {
    Line::from(vec![Span::styled(text, app.theme_styles.text.patch(app.theme_styles.tool_bg))])
}

pub(super) fn muted_line(text: impl Into<String>, app: &App) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.into(),
        app.theme_styles.muted.patch(app.theme_styles.tool_bg),
    )])
}
