//! Dock elsewhere — the ELSEWHERE section: changes outside the project, by place and folded.

use super::anchors::{list_window, scrolls, DockHit, DockSection};
use super::chrome::{plural_files, section_heading_line, Section};
use super::tree::{change_cells, tree_line, TreeCells, TreeLead};
use crate::app::App;
use crate::glyphs::{icon_label, Icon};
use crate::widgets::truncate_end as truncate_cells;
use crate::workspace_dock::{DockChangeRow, DockExternal, DockExternalGroup, DockList};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

/// Rows the ELSEWHERE list may spend before it starts to scroll.
///
/// A flat budget rather than the leftover height FILES claims: this section is a *pointer* to
/// work done somewhere else, and two elastic sections on one rail would each be sized by the
/// other. What keeps it complete at that size is the scrollbar — every place and every file under
/// it is a wheel notch away, so nothing needs a display cap.
const EXTERNAL_ROWS_MAX: usize = 6;

/// Floor for that budget on a short rail, so the section is a small viewport rather than none.
const EXTERNAL_ROWS_MIN: usize = 3;

/// Rows ELSEWHERE may spend on a dock of `available_height`.
pub(super) fn external_row_cap(available_height: usize) -> usize {
    EXTERNAL_ROWS_MAX.min(available_height / 4).max(EXTERNAL_ROWS_MIN)
}

/// The ELSEWHERE section: what this session changed outside the project.
///
/// Deliberately without a review affordance, unlike FILES. A review is built from the project's
/// Git worktree and its chapters end in commits to it, so offering to open one over a file that
/// lives somewhere else would promise something the review cannot do.
///
/// A *peek* is a different promise and one this section can keep, so its file rows are clickable
/// exactly like the project's: another worktree answers the same `git diff` the project does, and
/// a file in no repository is measured against the copy the session retained before it wrote
/// there. A row drawn in the project's own hand that did nothing would teach a reader that a file
/// name only sometimes opens its change.
///
/// What keeps it from growing without end is that it *recedes*: a file that has been looked at
/// dims, a place whose files have all been looked at folds to its one heading, and a click on any
/// heading folds or unfolds it by hand. Nothing is hidden that has not been seen, and a place the
/// agent touches again comes back. The rest is a viewport: the list keeps a bounded height and
/// scrolls, so a place three deep in the session is reachable rather than counted.
pub(super) fn external_lines(external: &DockExternal, app: &App, row_cap: usize) -> DockSection {
    let mut lines = vec![Line::default(), section_heading_line(Section::Elsewhere, app)];
    let mut hits = Vec::new();
    lines.push(external_headline(external, app));

    let all = external_rows(external, app);
    let cap = row_cap.max(1);
    let (offset, width) = list_window(app, DockList::Elsewhere, all.len(), cap, 0);

    let start = lines.len();
    // A window that opens under a heading repeats it, or its first rows sit under nothing at all.
    for index in external_context(&all, offset) {
        external_row_line(&all[index], width, app, &mut lines, &mut hits);
    }
    let mut drawn = 0usize;
    for row in all.iter().skip(offset).take(cap.saturating_sub(lines.len() - start)) {
        external_row_line(row, width, app, &mut lines, &mut hits);
        drawn += 1;
    }
    // A window ending on a heading promises rows that are not under it, and this list nests twice,
    // so a place and the folder under it can both land at the end. A *folded* place is whole on its
    // own — its count is the row — so only open headings are dropped.
    while drawn > 0 && all[offset + drawn - 1].is_open_heading() {
        if matches!(all[offset + drawn - 1], ExternalRow::Place { .. }) {
            hits.pop();
        }
        lines.pop();
        drawn -= 1;
    }
    let end = lines.len();

    DockSection {
        lines,
        hits,
        list: scrolls(DockList::Elsewhere, (start, end), offset, all.len(), cap),
    }
}

/// One row of the ELSEWHERE list, flat so a window into it is a `skip` and a `take`.
pub(super) enum ExternalRow<'a> {
    /// A place, and whether it is showing its heading alone.
    Place { group: &'a DockExternalGroup, folded: bool },
    /// A directory heading or a file, under the place above it.
    Under { group: &'a DockExternalGroup, row: DockChangeRow },
}

impl ExternalRow<'_> {
    /// Whether this row is a heading with rows owed underneath it.
    fn is_open_heading(&self) -> bool {
        match self {
            Self::Place { folded, .. } => !folded,
            Self::Under { row, .. } => matches!(row, DockChangeRow::Directory(_)),
        }
    }
}

/// Every place and everything under it, in the order the rail draws them.
///
/// Each place's files are grouped by directory through the same directory reduction the changes have always had, so a
/// folder reads the same on both. Files at a place's own root get no heading: the place above them
/// already is their directory, and a `/ (root)` row under it would say nothing twice.
pub(super) fn external_rows<'a>(external: &'a DockExternal, app: &App) -> Vec<ExternalRow<'a>> {
    let mut rows = Vec::new();
    for group in &external.groups {
        let seen: Vec<bool> = group.files.iter().map(|file| peeked(app, group, file)).collect();
        let folded = folded(app, group, &seen);
        rows.push(ExternalRow::Place { group, folded });
        if folded {
            continue;
        }
        rows.extend(
            crate::workspace_dock::change_rows(&group.files, |file| peeked(app, group, file))
                .into_iter()
                .filter(|row| !matches!(row, DockChangeRow::Directory(at) if at.directory == "."))
                .map(|row| ExternalRow::Under { group, row }),
        );
    }
    rows
}

/// The headings a window opening at `offset` must repeat, as indices into `rows`.
///
/// Up to two, because the list nests twice: the place, and the directory inside it. A window that
/// opens on a heading of its own needs neither.
pub(super) fn external_context(rows: &[ExternalRow<'_>], offset: usize) -> Vec<usize> {
    let nested = match rows.get(offset) {
        Some(ExternalRow::Place { .. }) | None => return Vec::new(),
        Some(ExternalRow::Under { row, .. }) => {
            matches!(row, DockChangeRow::File { path, .. } if path.contains('/'))
        }
    };
    let place = rows[..offset].iter().rposition(|row| matches!(row, ExternalRow::Place { .. }));
    let mut context: Vec<usize> = place.into_iter().collect();
    if nested {
        let directory = rows[..offset].iter().rposition(|row| {
            matches!(row, ExternalRow::Under { row, .. } if matches!(row, DockChangeRow::Directory(_)))
        });
        context.extend(directory.filter(|index| Some(*index) > place));
    }
    context
}

/// Draws one ELSEWHERE row, recording what a click on it means.
///
/// Drawn by the same hand as FILES' tree, since the two usually sit one above the other: a place is
/// a folder row, its directories nest under it, and a file wears its language's mark, a name in
/// its git color, and its ± and status letter on the right. What a place says on the right is
/// its own: the branch while it is open, and the count it is holding back once folded.
fn external_row_line(
    row: &ExternalRow<'_>,
    width: usize,
    app: &App,
    lines: &mut Vec<Line<'static>>,
    hits: &mut Vec<DockHit>,
) {
    let styles = &app.theme_styles;
    match row {
        ExternalRow::Place { group, folded } => {
            hits.push(DockHit::row(
                lines.len(),
                crate::mouse::MouseTarget::DockExternalPlace(fold_key(group)),
            ));
            let hovered = matches!(
                app.pointer.hover.as_ref(),
                Some(crate::mouse::MouseTarget::DockExternalPlace(place)) if *place == fold_key(group)
            );
            lines.push(tree_line(place_cells(group, *folded, width, app), width, hovered, app));
        }
        ExternalRow::Under { row: DockChangeRow::Directory(at), .. } => {
            let cells = TreeCells {
                depth: 1,
                lead: TreeLead::Folder { open: true },
                name: at.directory.clone(),
                // A directory whose files have all been looked at recedes
                name_style: match at.reviewed {
                    true => styles.dim,
                    false => styles.text.add_modifier(Modifier::BOLD),
                },
                right: Vec::new(),
            };
            lines.push(tree_line(cells, width, false, app));
        }
        ExternalRow::Under {
            group,
            row: DockChangeRow::File { name, path, status, additions, deletions, reviewed },
        } => {
            let peek = group.peek(path);
            hits.push(DockHit::row(
                lines.len(),
                crate::mouse::MouseTarget::DockChangeFile(peek.clone()),
            ));
            let hovered = matches!(
                app.pointer.hover.as_ref(),
                Some(crate::mouse::MouseTarget::DockChangeFile(hovered)) if *hovered == peek
            );
            let showing = app.changes.as_ref().is_some_and(|view| view.path == peek);
            let (name_style, right) =
                change_cells(*status, *additions, *deletions, *reviewed, styles);
            let cells = TreeCells {
                // A file at the place's own root sits directly under it; one in a folder sits under
                // that folder's heading
                depth: if path.contains('/') { 2 } else { 1 },
                lead: TreeLead::File { name, faint: false },
                name: name.clone(),
                name_style: match showing {
                    true => styles.accent.add_modifier(Modifier::BOLD),
                    false => name_style,
                },
                right,
            };
            lines.push(tree_line(cells, width, hovered, app));
        }
    }
}

/// One place: where it is, and on the right the branch when it is a checkout, or the count it is
/// holding back while folded.
///
/// The branch is not decoration here. A worktree's row lists everything Git reports there,
/// including changes this session did not make, and naming the branch is what tells a reader they
/// are looking at another repository's whole working tree rather than at a list of luxe's edits.
/// A folded place trades it for the count, which is what a heading standing alone has to say, and
/// recedes, since folded reads as dealt with. A branch never takes more than a third of the row:
/// the place is what the row is for.
fn place_cells<'a>(
    group: &DockExternalGroup,
    folded: bool,
    width: usize,
    app: &App,
) -> TreeCells<'a> {
    let styles = &app.theme_styles;
    let branch = group.branch.as_deref().unwrap_or_default();
    let files = group.files.len() + group.remaining.unwrap_or(0);
    let right = match (folded, group.git && !branch.is_empty()) {
        (true, _) => vec![(format!(" {}", plural_files(files)), styles.dim)],
        (false, true) => vec![(
            format!(
                " {}",
                icon_label(Icon::Branch.glyph(), &truncate_cells(branch, (width / 3).max(6)))
            ),
            styles.dim,
        )],
        (false, false) => Vec::new(),
    };
    TreeCells {
        depth: 0,
        lead: TreeLead::Folder { open: !folded },
        name: group.location.clone(),
        name_style: match folded {
            true => styles.dim,
            false => styles.text.add_modifier(Modifier::BOLD),
        },
        right,
    }
}

/// What a place's fold state is filed under — where it is, which is what the reader clicked.
fn fold_key(group: &DockExternalGroup) -> String {
    luxe_core::paths::display(&group.root)
}

/// Whether this file is unchanged since the reader last looked at it.
///
/// The ± has to match too: a file touched again since is new work, whatever was read before. Same
/// rule [`DockChanges::unreviewed`] applies to the project, for the same reason.
fn peeked(
    app: &App,
    group: &DockExternalGroup,
    file: &crate::workspace_dock::DockChangeItem,
) -> bool {
    app.external_seen.get(&group.peek_path(file))
        == Some(&(file.additions.unwrap_or_default(), file.deletions.unwrap_or_default()))
}

/// Whether the ELSEWHERE place filed under `place` is showing its heading alone right now.
///
/// The renderer owns the rule, so a click that toggles it asks here rather than keeping a second
/// copy that could disagree with what is on screen.
pub(crate) fn external_place_folded(app: &App, place: &str) -> bool {
    let Some(external) = app.workspace.as_ref().and_then(DockExternal::from_snapshot) else {
        return false;
    };
    let Some(group) = external.groups.iter().find(|group| fold_key(group) == place) else {
        return false;
    };
    let seen: Vec<bool> = group.files.iter().map(|file| peeked(app, group, file)).collect();
    folded(app, group, &seen)
}

/// Whether a place shows its heading alone: because the reader said so, or because there is
/// nothing under it they have not already seen.
///
/// The automatic half waits while the pane is showing one of its files. Peeking a place's last
/// unread file marks it seen, and folding on that would take the row out from under the reader in
/// the same frame their click opened it. An explicit choice still wins either way — a fold the
/// reader asked for is not a state to be second-guessed.
fn folded(app: &App, group: &DockExternalGroup, seen: &[bool]) -> bool {
    if let Some(chosen) = app.external_folds.get(&fold_key(group)) {
        return *chosen;
    }
    let showing = app
        .changes
        .as_ref()
        .is_some_and(|view| group.files.iter().any(|file| group.peek_path(file) == view.path));
    !showing && !seen.is_empty() && seen.iter().all(|seen| *seen)
}

/// `3 files outside this project`. No ± total: every file row below carries its own, and a sum
/// over places that may be whole other worktrees says less than the rows it would repeat.
fn external_headline(external: &DockExternal, app: &App) -> Line<'static> {
    let styles = &app.theme_styles;
    let files = external.total_files;
    Line::from(Span::styled(
        format!("{files} file{} outside this project", if files == 1 { "" } else { "s" }),
        styles.text.patch(styles.tool_bg),
    ))
}
