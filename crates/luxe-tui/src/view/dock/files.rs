//! Dock files — the FILES section: its heading, filter, tree viewport and the ways into the change.

use super::anchors::{list_window, scrolls, DockHit, DockSection};
use super::chrome::{change_summary_line, heading_spans, muted_line, Section};
use super::plan::PLAN_LOOKAHEAD;
use super::row_width;
use super::tree::{tree_row_line, tree_row_target};
use crate::app::App;
use crate::files::{TreeFilter, TreeRow};
use crate::workspace_dock::{DockChangeItem, DockChanges, DockList};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// Fixed rows FILES spends whatever the tree holds: a leading blank, the heading, a headline, and
/// the row of ways into the change under the tree.
pub(super) const FILES_FIXED_ROWS: usize = 4;

/// Floor for the tree's viewport, so FILES does not vanish on a crowded rail.
///
/// The tree is a viewport onto the whole project, so a budget of nothing would not be a smaller
/// section but an unreachable one. On a tall dock with a long plan and a list of places beside it
/// the leftover height can be zero, and the answer is to keep a few rows and let the panel's own
/// scroll reach what sits below.
pub(super) const FILE_ROWS_MIN: usize = 5;

/// How many tree rows FILES may show: everything the other sections and its own fixed rows leave.
///
/// No ceiling, unlike the CHANGES list it replaced. A list of changes had a natural length and a
/// tall dock spent on it was wasted; a tree has the whole project behind it, and the rail's spare
/// height is exactly what browsing it wants.
pub(super) fn files_row_cap(available_height: usize, reserved: usize) -> usize {
    // The floor scales down with the panel so a very short dock still spends most of itself on the
    // other sections; above that it is a flat few rows, whatever else is competing.
    let floor = FILE_ROWS_MIN.min(available_height / 4);
    available_height.saturating_sub(reserved + FILES_FIXED_ROWS).max(floor)
}

/// What FILES says after the work left the working tree, or nothing when it never was there.
///
/// An empty diff against `HEAD` has two causes that look identical in a snapshot and mean opposite
/// things: nothing has happened yet, or it was committed. Saying nothing about the second is how a
/// reader loses a review they were about to open — the counts and the markers all disappear at
/// once, with nothing left saying there was anything to see.
///
/// The turn's own baseline outlives the commit (luxe retains the before and after content on disk,
/// not a Git range), so the way in is still open; only the sign of it was missing.
fn committed_notice(app: &App) -> Option<Vec<Line<'static>>> {
    if !app.worktree_emptied || app.review.last_turn.is_none() {
        return None;
    }
    Some(vec![
        Line::from(Span::styled(
            "committed — nothing in the working tree".to_string(),
            app.theme_styles.text.patch(app.theme_styles.tool_bg),
        )),
        muted_line("the last turn is still reviewable", app),
    ])
}

/// The FILES section: the project as a tree, with what changed marked in it.
///
/// It took CHANGES' place and kept its job. Folders on the way to a changed file start open, the
/// headline counts the change (against the last review, when there was one), and the two ways into
/// the change stay on the row under the tree. What is new is everything else: the rest of the
/// project is a click away rather than a terminal away. Every row is a window over the whole
/// tree, scrolling in a viewport of its own like the lists around it, and the scope switch on the
/// heading narrows it to the changes alone.
pub(super) fn file_lines(changes: Option<&DockChanges>, app: &App, row_cap: usize) -> DockSection {
    let items: &[DockChangeItem] = changes.map_or(&[], |changes| changes.files.as_slice());
    let seen = app.review.seen.as_ref();
    let (heading, scope) = files_heading_line(changes, app);
    let mut lines = vec![Line::default(), heading];
    let mut hits = vec![scope];
    if let Some(changes) = changes.filter(|_| seen.is_some()) {
        lines.push(change_summary_line(changes, app));
    }
    let committed = committed_notice(app);
    let reviewable = committed.is_some();
    lines.extend(committed.into_iter().flatten());
    let filter = app.files.filter.as_ref().map(|filter| filter_line(filter, items, app));
    // the filter's own row comes out of the tree's, so the section keeps its height
    let row_cap = row_cap.saturating_sub(usize::from(filter.is_some()));
    lines.extend(filter);

    let rows = app.files.rows(items);
    let matched = app.files.hits(items);
    let cap = row_cap.max(1);
    let open = app.changes.as_ref().map(|view| view.path.as_str());
    let focus = app.files.cursor.as_deref().or(open);
    let follow = focus
        .and_then(|path| rows.iter().position(|row| row.path() == Some(path)))
        .map_or(0, |at| (at + 1 + PLAN_LOOKAHEAD).saturating_sub(cap));
    let (offset, width) = list_window(app, DockList::Files, rows.len(), cap, follow);

    let start = lines.len();
    for row in rows.iter().skip(offset).take(cap) {
        if let Some(target) = tree_row_target(row) {
            hits.push(DockHit::row(lines.len(), target));
        }
        let mut line = tree_row_line(row, width, open, seen, app);
        if let Some(positions) = row.path().and_then(|path| matched.positions.get(path)) {
            light_matches(&mut line, row, positions, app);
        }
        lines.push(line);
    }
    if rows.is_empty() && app.files.changed_only {
        lines.push(muted_line("nothing has changed", app));
    }
    let end = lines.len();

    // While the keyboard is in the tree, the keys it answers to; otherwise the ways into the
    // change, and the way to move through the tree without the mouse
    if app.files.focused {
        let keys = match app.files.filter.as_ref() {
            Some(filter) if filter.typing => "↑ ↓ move · enter open · esc clear",
            Some(_) => "j k move · / edit · esc clear",
            None => "j k move · l open · / filter · esc back",
        };
        lines.push(muted_line(keys, app));
    }
    let mut hints = match (app.files.focused, changes.is_some(), reviewable) {
        (true, ..) => Vec::new(),
        (false, true, _) => change_hints(app),
        (false, false, true) => review_hint(app),
        (false, false, false) => Vec::new(),
    };
    if !app.files.focused {
        // First, so a rail too narrow for every hint gives it up before the ways into the change;
        // the preview's footer and the key card say it too
        hints.insert(0, browse_hint(app));
    }
    if !hints.is_empty() {
        let (row, spans) = hint_line(&hints, lines.len(), app);
        hits.extend(spans);
        lines.push(row);
    }
    DockSection {
        lines,
        hits,
        list: scrolls(DockList::Files, (start, end), offset, rows.len(), cap),
    }
}

/// `FILES 4 changed +335 −29            all ▾` — the heading, what changed, and what is showing.
///
/// The scope switch rides the heading's right edge, where a reader looks for a list's filter, and
/// answers for its own cells, so the rest of the heading stays a heading.
fn files_heading_line(changes: Option<&DockChanges>, app: &App) -> (Line<'static>, DockHit) {
    let styles = &app.theme_styles;
    let mut spans = heading_spans(Section::Files, app);
    if let Some(changes) = changes {
        let seen = app.review.seen.as_ref();
        let unreviewed = changes.unreviewed(seen);
        let (text, style) = match seen.is_some() {
            false => (format!(" {} changed", changes.total_files), styles.text),
            true if unreviewed == 0 => (" all reviewed".to_string(), styles.success),
            true => (format!(" {unreviewed} new since your review"), styles.text),
        };
        spans.push(Span::styled(text, style.patch(styles.tool_bg)));
        // Totals git could not count are left out rather than printed as `?`
        if let (None, Some(additions), Some(deletions)) =
            (seen, changes.additions, changes.deletions)
        {
            spans
                .push(Span::styled(format!(" +{additions}"), styles.success.patch(styles.tool_bg)));
            spans.push(Span::styled(format!(" −{deletions}"), styles.error.patch(styles.tool_bg)));
        }
    }
    let scope = match (app.files.changed_only, app.files.show_ignored) {
        (true, _) => "changed ▾",
        (false, false) => "all ▾",
        (false, true) => "all + ignored ▾",
    };
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    let width = row_width(app);
    let column = width.saturating_sub(scope.width()).max(used + 1);
    spans.push(Span::styled(" ".repeat(column - used), styles.muted.patch(styles.tool_bg)));
    let target = crate::mouse::MouseTarget::DockFilesScope;
    let ink = match app.pointer.hover.as_ref() == Some(&target) {
        true => styles.text.patch(styles.tool_bg).patch(styles.selected_bg),
        false => styles.muted.patch(styles.tool_bg),
    };
    spans.push(Span::styled(scope, ink));
    let hit = DockHit {
        line: 1,
        columns: Some((
            u16::try_from(column).unwrap_or(u16::MAX),
            u16::try_from(scope.width()).unwrap_or(u16::MAX),
        )),
        target,
    };
    (Line::from(spans), hit)
}

/// `/ dock▏                 12 of 340` — what the tree is filtered by, and how much it matched.
fn filter_line(filter: &TreeFilter, items: &[DockChangeItem], app: &App) -> Line<'static> {
    let styles = &app.theme_styles;
    let ink = |style: ratatui::style::Style| style.patch(styles.tool_bg);
    let mut spans = vec![
        Span::styled("/ ", ink(styles.accent)),
        Span::styled(filter.query.clone(), ink(styles.text.add_modifier(Modifier::BOLD))),
    ];
    if filter.typing {
        spans.push(Span::styled("▏", ink(styles.accent)));
    }
    let count = match app.files.filtering() {
        Some(_) => {
            let hits = app.files.hits(items);
            match hits.total {
                0 => "none".to_string(),
                total if total > crate::files::filter::SHOWN => {
                    format!("{} of {total}", crate::files::filter::SHOWN)
                }
                total => format!("{total}"),
            }
        }
        None => "type to filter".to_string(),
    };
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    let width = row_width(app);
    let pad = width.saturating_sub(used + count.width()).max(1);
    spans.push(Span::styled(" ".repeat(pad), ink(styles.muted)));
    spans.push(Span::styled(count, ink(styles.muted)));
    Line::from(spans)
}

/// Lights the characters of a row's name that the filter matched.
///
/// The positions are in the row's whole path and the name is its tail, so they shift by what comes
/// before it. A name elided to fit is left unlit, since its characters no longer sit where the
/// positions say.
fn light_matches(line: &mut Line<'static>, row: &TreeRow, positions: &[usize], app: &App) {
    let (Some(path), TreeRow::Folder { name, .. } | TreeRow::File { name, .. }) = (row.path(), row)
    else {
        return;
    };
    let Some(at) = line.spans.iter().position(|span| span.content == name.as_str()) else {
        return;
    };
    let offset = path.chars().count() - name.chars().count();
    let lit: std::collections::HashSet<usize> =
        positions.iter().filter_map(|position| position.checked_sub(offset)).collect();
    let base = line.spans[at].style;
    let matched = base.patch(app.theme_styles.accent).add_modifier(Modifier::BOLD);
    let mut pieces: Vec<Span<'static>> = Vec::new();
    for (index, character) in name.chars().enumerate() {
        let style = if lit.contains(&index) { matched } else { base };
        match pieces.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(character),
            _ => pieces.push(Span::styled(character.to_string(), style)),
        }
    }
    line.spans.splice(at..=at, pieces);
}

/// Cells between two hints on the same row.
const HINT_SEPARATOR: &str = " · ";

/// One way into the change, and the words that offer it.
pub(super) struct ChangeHint {
    /// `Ctrl+P peek`.
    pub(super) text: String,
    /// What a click on those words does.
    pub(super) target: crate::mouse::MouseTarget,
}

/// The two ways into the change: a glance at one file, and the review of all of them.
///
/// Both were always here — every file row above is already a peek away — but only the review said
/// so, which left the cheaper of the two answers a secret. Chords are read back from the live
/// keymap rather than typed in, so a remapped binding cannot leave the dock advertising a key that
/// does nothing; the slash command is the fallback when an action is unbound.
///
/// A chord and the verb it performs, which is the shape every other footer in the product uses
/// (`Space next`, `Esc close`, `Alt+W no wrap`), and on a forty-cell rail no word is free.
pub(super) fn change_hints(app: &App) -> Vec<ChangeHint> {
    use luxe_core::keymap::UiAction;
    let chord = |action, fallback: &str| {
        app.keymap
            .chords_for(action)
            .and_then(|chords| chords.first())
            .map_or_else(|| fallback.to_string(), ToString::to_string)
    };
    vec![
        ChangeHint {
            text: format!("{} preview", chord(UiAction::ToggleChanges, "/preview")),
            target: crate::mouse::MouseTarget::DockPeekChanges,
        },
        ChangeHint {
            text: format!("{} review", chord(UiAction::ReviewOpen, "/diff")),
            target: crate::mouse::MouseTarget::DockReviewChanges,
        },
    ]
}

/// `Ctrl+T browse` — the way into the tree from the keyboard.
fn browse_hint(app: &App) -> ChangeHint {
    let chord = app
        .keymap
        .chords_for(luxe_core::keymap::UiAction::FocusFiles)
        .and_then(|chords| chords.first())
        .map_or_else(|| "ctrl+t".to_string(), ToString::to_string);
    ChangeHint {
        text: format!("{chord} browse"),
        target: crate::mouse::MouseTarget::DockFocusFiles,
    }
}

/// The review hint on its own, for a change that has already left the working tree.
fn review_hint(app: &App) -> Vec<ChangeHint> {
    change_hints(app)
        .into_iter()
        .filter(|hint| hint.target == crate::mouse::MouseTarget::DockReviewChanges)
        .collect()
}

/// `Ctrl+P peek · Ctrl+G review` — the hints on one row, with the columns each one owns.
///
/// Hints are dropped from the front until the row fits the rail, so the row never wraps: a wrapped
/// hint puts its second half where the click regions no longer point, and the one that survives is
/// the review — the only one of the two a reader cannot reach by clicking a file name above it.
///
/// Each hint highlights under the pointer on its own, the way the Changes pane's footer does, so a
/// row offering two things says which of them is about to happen.
pub(super) fn hint_line(
    hints: &[ChangeHint],
    line: usize,
    app: &App,
) -> (Line<'static>, Vec<DockHit>) {
    let styles = &app.theme_styles;
    let width = row_width(app);
    let rendered = |hints: &[ChangeHint]| {
        hints.iter().map(|hint| hint.text.chars().count()).sum::<usize>()
            + HINT_SEPARATOR.chars().count() * hints.len().saturating_sub(1)
    };
    let mut shown = hints;
    while shown.len() > 1 && rendered(shown) > width {
        shown = &shown[1..];
    }

    let mut spans = Vec::new();
    let mut regions = Vec::new();
    let mut column = 0usize;
    for (index, hint) in shown.iter().enumerate() {
        if index > 0 {
            // The rail's own separator, not the footer's dimmer one: every neighboring row here
            // is muted, and a fainter dot between two of them simply reads as a gap.
            spans.push(Span::styled(HINT_SEPARATOR, styles.muted.patch(styles.tool_bg)));
            column += HINT_SEPARATOR.chars().count();
        }
        let cells = hint.text.chars().count();
        let style = if app.pointer.hover.as_ref() == Some(&hint.target) {
            styles.text.patch(styles.tool_bg).patch(styles.selected_bg)
        } else {
            styles.muted.patch(styles.tool_bg)
        };
        spans.push(Span::styled(hint.text.clone(), style));
        regions.push(DockHit {
            line,
            columns: Some((
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(cells).unwrap_or(u16::MAX),
            )),
            target: hint.target.clone(),
        });
        column += cells;
    }
    (Line::from(spans), regions)
}
