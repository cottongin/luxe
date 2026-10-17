//! Dock tree — one row of the file tree: its lead, name and change cells, fitted to the rail.

use super::chrome::status_mark;
use crate::app::App;
use crate::files::TreeRow;
use crate::glyphs::{icon_cells, icon_label, Icon};
use crate::widgets::truncate_end as truncate_cells;
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// What a click on a tree row does.
pub(super) fn tree_row_target(row: &TreeRow) -> Option<crate::mouse::MouseTarget> {
    match row {
        TreeRow::Folder { path, .. } => Some(crate::mouse::MouseTarget::DockFolder(path.clone())),
        TreeRow::File { path, .. } => Some(crate::mouse::MouseTarget::DockFile(path.clone())),
        TreeRow::Note { .. } => None,
    }
}

/// One tree row: indent guides, a chevron on a folder, an icon, the name, and on the right what git
/// says about it.
///
/// A changed file wears the color and letter CHANGES gave it, and its ± counts; a closed folder with
/// a change somewhere under it carries a dot, so nothing changed can hide behind a fold. The row
/// showing in the Changes pane, the one the keyboard is on and the one under the pointer all wear
/// the selection slab, which means *this one* everywhere else in the product.
pub(super) fn tree_row_line(
    row: &TreeRow,
    width: usize,
    open: Option<&str>,
    seen: Option<&luxe_core::guided_review::SeenReview>,
    app: &App,
) -> Line<'static> {
    let styles = &app.theme_styles;
    let path = row.path();
    let hovered = matches!(
        app.pointer.hover.as_ref(),
        Some(crate::mouse::MouseTarget::DockFile(hovered) | crate::mouse::MouseTarget::DockFolder(hovered))
            if Some(hovered.as_str()) == path
    );
    let keyed = app.files.focused && app.files.cursor.as_deref() == path && path.is_some();
    // The band is where the pointer or the keyboard is; the file showing in the preview says so
    // in its name instead, so the two never look alike when they are on different rows
    let highlight = hovered || keyed;
    let showing = path.is_some() && path == open;

    let cells = match row {
        TreeRow::Note { depth, text } => TreeCells {
            depth: *depth,
            lead: TreeLead::Bare,
            name: text.clone(),
            name_style: styles.dim,
            right: Vec::new(),
        },
        // What `.gitignore` ignores is drawn faint, since it is there only because it was asked for
        TreeRow::Folder { name, depth, open: unfolded, holds_change, ignored, .. } => TreeCells {
            depth: *depth,
            lead: TreeLead::Folder { open: *unfolded },
            name: name.clone(),
            name_style: match ignored {
                true => styles.dim,
                false => styles.text.add_modifier(Modifier::BOLD),
            },
            right: match *holds_change && !*unfolded {
                true => vec![(" •".to_string(), styles.warning)],
                false => Vec::new(),
            },
        },
        TreeRow::File { name, depth, change, ignored, .. } => {
            let (name_style, right) = match change {
                Some(change) => {
                    let reviewed = seen.is_some_and(|seen| {
                        seen.covers_file(
                            &change.path,
                            change.additions.unwrap_or_default(),
                            change.deletions.unwrap_or_default(),
                        )
                    });
                    change_cells(
                        change.status,
                        change.additions,
                        change.deletions,
                        reviewed,
                        styles,
                    )
                }
                None if *ignored => (styles.dim, Vec::new()),
                None => (styles.text, Vec::new()),
            };
            TreeCells {
                depth: *depth,
                lead: TreeLead::File { name, faint: *ignored },
                name: name.clone(),
                name_style: match showing {
                    true => styles.accent.add_modifier(Modifier::BOLD),
                    false => name_style,
                },
                right,
            }
        }
    };
    tree_line(cells, width, highlight, app)
}

/// What a tree row draws, whichever list it sits in: FILES' tree, or a place under ELSEWHERE.
pub(super) struct TreeCells<'a> {
    /// Levels under the list's top, one `│ ` guide each
    pub(super) depth: usize,
    pub(super) lead: TreeLead<'a>,
    pub(super) name: String,
    pub(super) name_style: ratatui::style::Style,
    /// What sits at the right edge, after the name has given way
    pub(super) right: Vec<(String, ratatui::style::Style)>,
}

/// What comes between a row's guides and its name.
pub(super) enum TreeLead<'a> {
    /// A chevron and a folder
    Folder { open: bool },
    /// The chevron's cells left blank, then the language mark for the file `name`
    File { name: &'a str, faint: bool },
    /// Nothing: a note stands where entries would
    Bare,
}

/// A changed file's name style and what its row says on the right: `+6 −0 M`.
///
/// A file with no counts prints none. Git says nothing about a binary blob, and rendering that
/// silence as `+0 −0` claims the file did not change, which is worse than saying nothing. A file
/// already looked at recedes, and keeps its counts so the reader can still see how big it was.
pub(super) fn change_cells(
    status: luxe_core::workspace::FileChangeStatus,
    additions: Option<u64>,
    deletions: Option<u64>,
    reviewed: bool,
    styles: &crate::theme::styles::ThemeStyles,
) -> (ratatui::style::Style, Vec<(String, ratatui::style::Style)>) {
    let mut right = Vec::new();
    if let (Some(additions), Some(deletions)) = (additions, deletions) {
        right.push((format!(" +{additions}"), styles.success));
        right.push((format!(" −{deletions}"), styles.error));
    }
    let (marker, marker_style) = status_mark(status, styles);
    right.push((format!(" {marker}"), marker_style));
    (if reviewed { styles.dim } else { marker_style }, right)
}

/// Draws a [`TreeCells`]: guides, lead, the name elided to fit, and the right edge.
pub(super) fn tree_line(
    cells: TreeCells<'_>,
    width: usize,
    highlight: bool,
    app: &App,
) -> Line<'static> {
    let styles = &app.theme_styles;
    let tool_bg = styles.tool_bg;
    let ink = |base: ratatui::style::Style| match highlight {
        true => base.patch(tool_bg).patch(styles.selected_bg),
        false => base.patch(tool_bg),
    };
    let mut left = vec![Span::styled("│ ".repeat(cells.depth), styles.border.patch(tool_bg))];
    let elide = match cells.lead {
        TreeLead::Folder { open } => {
            left.push(Span::styled(if open { "▾ " } else { "▸ " }, ink(styles.muted)));
            left.push(Span::styled(
                icon_label(Icon::Folder.glyph(), ""),
                ink(styles.accent_secondary),
            ));
            true
        }
        TreeLead::File { name, faint } => {
            let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
            let icon = crate::widgets::language_icon(&extension.to_ascii_lowercase());
            // A language the tier has no mark for keeps the mark's cells, so a file's name lines
            // up with its neighbors' whatever the font can draw
            let column = icon_cells(Icon::Folder.glyph());
            let mark = format!("{:<column$}", icon_label(icon.glyph(), ""));
            let mark_style = match faint {
                true => styles.dim,
                false => crate::widgets::language_hue(icon, styles),
            };
            left.push(Span::raw("  "));
            left.push(Span::styled(mark, ink(mark_style)));
            true
        }
        TreeLead::Bare => false,
    };
    left.push(Span::styled(cells.name, ink(cells.name_style)));
    let right: Vec<Span<'static>> =
        cells.right.into_iter().map(|(text, style)| Span::styled(text, ink(style))).collect();

    // The name is the one part that gives way: elided in the middle, so the extension survives
    let fixed: usize =
        left[..left.len() - 1].iter().map(|span| span.content.width()).sum::<usize>()
            + right.iter().map(|span| span.content.width()).sum::<usize>();
    let budget = width.saturating_sub(fixed + crate::widgets::SELECTION_FRAME);
    if let Some(name) = left.last_mut() {
        name.content = match elide {
            true => crate::widgets::truncate_middle(&name.content, budget).into(),
            false => truncate_cells(&name.content, budget).into(),
        };
    }
    let used: usize = left.iter().chain(&right).map(|span| span.content.width()).sum();
    let pad = width.saturating_sub(used + crate::widgets::SELECTION_FRAME);
    left.push(Span::styled(" ".repeat(pad), ink(styles.text)));
    left.extend(right);
    crate::widgets::selection_row_on(
        left,
        width,
        0,
        highlight,
        tool_bg.bg.unwrap_or(styles.slab.canvas),
        styles,
    )
}
