//! Context Dock rendering — wide workspace panel presentation.
//!
//! The panel consumes the snapshot-derived section model and theme styles supplied by the app.
//! It never reads project state while drawing.

mod anchors;
mod chrome;
mod elsewhere;
mod files;
mod plan;
mod telemetry;
mod tree;

use crate::app::App;
use crate::workspace_dock::{DockPresentation, DockSectionModel};
use crate::workspace_layout::{gutter_inset, CONTEXT_DOCK_WIDTH};
use ratatui::{
    layout::Rect,
    text::Line,
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};

use crate::glyphs::{icon_label, Icon};
use anchors::{DockHit, DockHitAnchors, DockListAnchor};
pub(crate) use chrome::status_mark;
use chrome::{muted_line, section_heading_line, text_line, Section};
pub(crate) use elsewhere::external_place_folded;
use elsewhere::{external_lines, external_row_cap};
use files::{file_lines, files_row_cap};
use plan::plan_lines;
use telemetry::footer_lines;
#[cfg(test)]
use {
    crate::files::TreeRow,
    crate::glyphs::icon_cells,
    crate::theme::styles::Depth,
    crate::workspace_dock::{
        DockChangeRow, DockChanges, DockExternal, DockExternalGroup, DockPlan, DockPlanItem,
    },
    anchors::DockSection,
    chrome::section_heading_with,
    elsewhere::{external_context, external_rows, ExternalRow},
    files::{change_hints, hint_line, ChangeHint, FILES_FIXED_ROWS, FILE_ROWS_MIN},
    luxe_core::{config::StatuslineStyle, TodoSummaryStatus},
    plan::{plan_follow, plan_row_grants, subject_rows, todo_lines, PLAN_LOOKAHEAD},
    plan::{PLAN_ROWS_MAX, PLAN_WRAP_ROWS},
    ratatui::style::Modifier,
    telemetry::{telemetry_label_width, telemetry_lines, telemetry_value_width, token_value},
    telemetry::{TELEMETRY_LABEL_WIDTH, WIDEST_TELEMETRY_LABEL},
    tree::tree_row_line,
};

/// Renders the fixed-width Context panel from the latest accepted workspace snapshot.
///
/// The dock reads top-to-bottom like the main column: a pinned PROJECT header, a scrollable middle
/// (PLAN, FILES, ELSEWHERE, DEBUG, REVIEW), and a bottom-pinned footer carrying runtime status
/// next to the composer. `show_telemetry` (set when the statusline is hidden) relocates MODEL and
/// SESSION into that footer so the bar can hide without losing them.
pub(super) fn render(f: &mut Frame, area: Rect, app: &App, show_telemetry: bool) {
    let styles = &app.theme_styles;
    // Own every cell before painting: a `Block` only restyles what is already there, so as an
    // overlay the panel let the conversation's glyphs show through wherever a dock row was shorter
    // than the line underneath it — recolored as dock text, which read as garbled dock content.
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(styles.tool_bg), area);

    let presentation = app.dock.presentation(app.workspace_layout);
    // Every row below is measured to the rail this frame draws, which the reader may have dragged
    app.dock_rail.set(rail_width(app, area));
    // The whole panel is a scroll surface: a wheel notch anywhere over the dock scrolls its
    // middle region (the dock has no keyboard focus — it's informational).
    app.hit.borrow_mut().push_scroll(area, crate::mouse::ScrollRegionId::Dock);
    let DockRegions { top_lines, footer, anchors, placement } =
        dock_regions(app, area, show_telemetry, presentation);
    let DockHitAnchors { hits, footer_rows, lists } = anchors;

    let text_style = styles.text.patch(styles.tool_bg);
    // Inset the content off the separator (the bg Block above still fills the full pane)
    let content = gutter_inset(area, f.area());

    // Publish the clickable rows at their wrapped Y, only where scrolled into the visible top
    // area. The wrap is measured once for the whole column, so a row's position is a lookup; done
    // before `top_lines` is moved into the paragraph.
    let starts = crate::widgets::wrapped_row_starts(&top_lines, content.width, false);
    let placed = Placed { placement, rows: top_lines.len(), starts: &starts };
    publish_middle_hits(app, &placed, hits, content);
    let lanes = publish_list_lanes(app, &placed, &lists, area, content);

    let top_height = placement.top_height;
    f.render_widget(
        Paragraph::new(top_lines).style(text_style).wrap(Wrap { trim: false }),
        Rect { y: area.y, height: top_height, ..content },
    );
    // After the paragraph, because the bar sits on top of the column its rows gave up for it.
    for (lane, list) in lanes {
        let window = list.window.unwrap_or(usize::from(lane.height));
        crate::widgets::scrollbar(f, lane, list.offset, list.total, window, styles);
    }
    let footer_height = area.height.saturating_sub(top_height);
    if footer_height > 0 {
        let footer_area = Rect { y: area.y + top_height, height: footer_height, ..content };
        publish_footer_hits(app, &footer, &footer_rows, footer_area);
        f.render_widget(
            Paragraph::new(footer).style(text_style).wrap(Wrap { trim: false }),
            footer_area,
        );
    }
}

/// The dock's lines this frame, with the middle already scrolled, and where it sits.
struct DockRegions {
    /// The header, then the middle from its scroll offset on
    top_lines: Vec<Line<'static>>,
    footer: Vec<Line<'static>>,
    anchors: DockHitAnchors,
    placement: MiddlePlacement,
}

/// Where the scrolled middle landed: enough to turn a middle-line index into screen rows.
#[derive(Clone, Copy)]
struct MiddlePlacement {
    /// Top of the panel
    y: u16,
    /// Rows above the footer
    top_height: u16,
    header_len: usize,
    /// Middle lines scrolled away above the top
    offset: usize,
}

/// Builds the dock's header, middle and footer, pins the footer to the bottom, and scrolls the
/// middle under the header.
fn dock_regions(
    app: &App,
    area: Rect,
    show_telemetry: bool,
    presentation: DockPresentation,
) -> DockRegions {
    let (header, middle, footer, anchors) = app.workspace.as_ref().map_or_else(
        || loading_regions(app, show_telemetry, presentation),
        |snapshot| {
            region_lines(
                &DockSectionModel::from_snapshot(snapshot),
                app,
                area.height as usize,
                show_telemetry,
                presentation,
            )
        },
    );
    let header_len = header.len();

    // Footer hugs the bottom; keep at least one row for the header on absurdly short docks.
    let footer_height =
        u16::try_from(footer.len()).unwrap_or(u16::MAX).min(area.height.saturating_sub(1));
    let top_height = area.height.saturating_sub(footer_height);

    // The middle scrolls under a pinned header/footer; the largest useful offset leaves the last
    // middle rows filling the top area. Publish it so wheel scrolling clamps without re-deriving
    // the layout, then clamp this frame's offset to it.
    let visible_middle = usize::from(top_height).saturating_sub(header_len);
    let scroll_max = middle.len().saturating_sub(visible_middle.max(1));
    app.dock_scroll_max.set(scroll_max);
    let offset = app.dock.scroll.min(scroll_max);
    let mut top_lines = header;
    top_lines.extend(middle.into_iter().skip(offset));
    let placement = MiddlePlacement { y: area.y, top_height, header_len, offset };
    DockRegions { top_lines, footer, anchors, placement }
}

/// [`MiddlePlacement`] with the top lines' wrapped row starts.
struct Placed<'a> {
    placement: MiddlePlacement,
    rows: usize,
    starts: &'a [u16],
}

impl Placed<'_> {
    /// A middle-line index as the screen rows it occupies, or `None` where it is scrolled away.
    fn placed(&self, mid_index: usize) -> Option<(u16, u16)> {
        let MiddlePlacement { y: top, top_height, header_len, offset } = self.placement;
        let bottom = top + top_height;
        let top_index = header_len + mid_index.checked_sub(offset)?;
        if top_index >= self.rows {
            return None;
        }
        let before = self.starts[top_index];
        let y = top.saturating_add(before);
        (y < bottom)
            .then(|| (y, self.starts[top_index + 1].saturating_sub(before).max(1).min(bottom - y)))
    }
}

/// Publishes what a pointer can act on in the scrolled middle, at the rows each landed on.
fn publish_middle_hits(app: &App, placed: &Placed<'_>, hits: Vec<DockHit>, content: Rect) {
    for hit in hits {
        let Some((y, row_height)) = placed.placed(hit.line) else {
            continue;
        };
        // A column-scoped hit describes one line's worth of text, so a row that wrapped would put
        // its second affordance somewhere these columns no longer name. The hint row is built to
        // fit for exactly this reason; if it ever does not, the row keeps its first meaning rather
        // than acquiring a wrong one.
        let region = match hit.columns {
            Some(_) if row_height > 1 => None,
            Some((offset, width)) => (offset < content.width)
                .then(|| Rect::new(content.x + offset, y, width.min(content.width - offset), 1)),
            None => Some(Rect::new(content.x, y, content.width, row_height)),
        };
        if let Some(region) = region {
            app.hit.borrow_mut().push(region, hit.target);
        }
    }
}

/// Publishes each scrolling list's wheel region and lane, and returns the lanes to draw.
///
/// A scrolling list owns the wheel over its own rows and carries a lane at the far edge of the
/// content column; everywhere else in the panel still scrolls the panel. Published after the
/// panel-wide region, so a list wins where they meet.
fn publish_list_lanes<'a>(
    app: &App,
    placed: &Placed<'_>,
    lists: &'a [DockListAnchor],
    area: Rect,
    content: Rect,
) -> Vec<(Rect, &'a DockListAnchor)> {
    lists
        .iter()
        .filter(|_| content.width > 0)
        .filter_map(|list| {
            let (start, end) = list.rows;
            let ((top, _), (y, height)) = (
                (start..end).find_map(|row| placed.placed(row))?,
                (start..end).rev().find_map(|row| placed.placed(row))?,
            );
            let rows = y.saturating_add(height).saturating_sub(top);
            let region = crate::mouse::ScrollRegionId::DockList(list.list);
            let mut hit = app.hit.borrow_mut();
            hit.push_scroll(Rect::new(area.x, top, area.width, rows), region);
            let lane = Rect::new(lane_column(content, app), top, 1, rows);
            hit.push(lane, crate::mouse::MouseTarget::Scrollbar { region, y: top, height: rows });
            Some((lane, list))
        })
        .collect()
}

/// Publishes the footer's clickable rows at their wrapped Y within `footer_area`.
///
/// The footer renders from index 0 (no scroll), so a row's wrapped Y is the height of everything
/// above it.
fn publish_footer_hits(
    app: &App,
    footer: &[Line<'static>],
    rows: &[(usize, crate::mouse::MouseTarget)],
    footer_area: Rect,
) {
    let starts = crate::widgets::wrapped_row_starts(footer, footer_area.width, false);
    let footer_y = footer_area.y;
    let footer_bottom = footer_area.bottom();
    for (row, target) in rows.iter().filter(|(row, _)| *row < footer.len()) {
        let row = *row;
        let before = starts[row];
        let y = footer_y.saturating_add(before);
        if y < footer_bottom {
            let row_height = starts[row + 1].saturating_sub(before).max(1).min(footer_bottom - y);
            app.hit
                .borrow_mut()
                .push(Rect::new(footer_area.x, y, footer_area.width, row_height), target.clone());
        }
    }
}

/// Screen column the scrollbar lane occupies: the last one a full-width row would reach.
///
/// Tied to the row width rather than to the pane, because a dock wider than the rail (the compact
/// layout, where it is the whole terminal) still draws rail-wide rows, and a bar out at the pane's
/// own edge would sit in empty space nowhere near the list it belongs to.
fn lane_column(content: Rect, app: &App) -> u16 {
    let width = u16::try_from(row_width(app)).unwrap_or(u16::MAX).min(content.width);
    content.x.saturating_add(width.saturating_sub(1))
}

/// The rail a dock drawn in `area` measures its rows to: the width the reader dragged it to, or
/// the default, and never wider than the room it was given.
///
/// Beside the conversation and as an overlay, `area` *is* the rail. Where the dock takes the whole
/// terminal the rows keep the rail's measure rather than stretching across it.
fn rail_width(app: &App, area: Rect) -> u16 {
    let preferred = app
        .panes
        .width(crate::workspace_layout::Pane::Dock)
        .unwrap_or(CONTEXT_DOCK_WIDTH)
        .clamp(crate::workspace_layout::DOCK_MIN_WIDTH, crate::workspace_layout::DOCK_MAX_WIDTH);
    preferred.min(area.width)
}

/// Cells a dock row has to itself: the rail this frame draws, less the air on each side of it.
///
/// Measured from the rail rather than the pane, so a row reads the same width wherever the dock is
/// drawn — in a pane whose host already frames it, the spare column goes unused rather than
/// reflowing the rail.
fn row_width(app: &App) -> usize {
    usize::from(app.dock_rail.get()).saturating_sub(2)
}

fn loading_regions(
    app: &App,
    show_telemetry: bool,
    presentation: DockPresentation,
) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<Line<'static>>, DockHitAnchors) {
    let header = vec![
        section_heading_line(Section::Project, app),
        muted_line("Waiting for workspace state…", app),
    ];
    let (footer, _) = footer_lines(app, show_telemetry, presentation);
    (header, Vec::new(), footer, DockHitAnchors::default())
}

/// Cells a scrolling list gives up on its right: a blank column, then the bar.
///
/// The bar lands on the last column the inset leaves for content, so the column the panel already
/// keeps clear is what stands between it and the divider beside the conversation.
const SCROLLBAR_LANE: usize = 2;

/// Splits the dock into `(header, middle, footer)` plus the anchors `render` publishes.
///
/// The header pins project identity to the top, the middle carries the scrollable section list, and
/// the footer pins runtime status to the bottom near the composer.
fn region_lines(
    model: &DockSectionModel,
    app: &App,
    available_height: usize,
    show_telemetry: bool,
    presentation: DockPresentation,
) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<Line<'static>>, DockHitAnchors) {
    let mut header = vec![section_heading_line(Section::Project, app)];
    // The statusline's project module, in the dock: the same folder beside the name, as the branch,
    // pull request and model rows here already carry their modules' icons
    header.push(text_line(icon_label(Icon::Folder.glyph(), &model.identity.project_name), app));
    if let Some(branch) = model.identity.branch.as_deref() {
        let dirty = if model.identity.dirty { " *" } else { "" };
        header.push(muted_line(icon_label(Icon::Branch.glyph(), &format!("{branch}{dirty}")), app));
    }
    if let Some(pull_request) = model.identity.pull_request.as_deref() {
        header.push(muted_line(icon_label(Icon::PullRequest.glyph(), pull_request), app));
    }

    let (footer, footer_rows) = footer_lines(app, show_telemetry, presentation);

    // Build the fixed-size sections first so Changes can size itself to the leftover height: it
    // shares the scrollable middle with them and shouldn't push them off-screen.
    let plan = model.plan.as_ref().map(|plan| plan_lines(plan, app));
    let external =
        model.external.as_ref().map(|e| external_lines(e, app, external_row_cap(available_height)));
    let reserved = header.len()
        + footer.len()
        + plan.as_ref().map_or(0, |section| section.lines.len())
        + external.as_ref().map_or(0, |section| section.lines.len());

    let mut middle = Vec::new();
    let mut anchors = DockHitAnchors { footer_rows, ..DockHitAnchors::default() };
    if let Some(plan) = plan {
        plan.land(&mut middle, &mut anchors);
    }
    file_lines(model.changes.as_ref(), app, files_row_cap(available_height, reserved))
        .land(&mut middle, &mut anchors);
    // Directly under FILES: it answers the question that section just raised — is that all of it?
    if let Some(external) = external {
        external.land(&mut middle, &mut anchors);
    }
    (header, middle, footer, anchors)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod elision_tests {
    use super::anchors::list_window;
    use super::*;
    use crate::workspace_dock::{DockChangeItem, DockChanges, DockList};
    use luxe_core::config::Config;
    use luxe_core::keymap::KeyMap;

    /// A file whose name is wider than its row keeps its extension, and the row its width.
    ///
    /// The break this catches: the row's name was cut by counting characters, so a run of wide
    /// glyphs spilled past the rail, and the cell-aware end cut that followed dropped the `.rs`.
    #[test]
    fn a_wide_name_keeps_its_extension_inside_its_row() {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let config = Config { theme: Some("nord".into()), ..Config::default() };
        let app = App::new(tx, KeyMap::defaults(), config);
        let name = format!("src/{}.rs", "界".repeat(30));
        let changes = DockChanges {
            files: vec![DockChangeItem {
                path: name,
                status: luxe_core::workspace::FileChangeStatus::Modified,
                additions: Some(1),
                deletions: Some(0),
            }],
            total_files: 1,
            additions: Some(1),
            deletions: Some(0),
            remaining: None,
        };
        let section = file_lines(Some(&changes), &app, 8);
        let text = |line: &Line<'_>| line.spans.iter().map(|span| span.content.as_ref()).collect();
        let rows: Vec<String> = section.lines.iter().map(text).collect();
        let row = rows.iter().find(|row| row.contains('界')).expect("the file's row");
        assert!(row.contains(".rs"), "{row}");
        assert!(row.contains('…'), "{row}");
        let (_, width) = list_window(&app, DockList::Files, 1, 8, 0);
        let cells = unicode_width::UnicodeWidthStr::width(row.as_str());
        assert!(cells <= width, "{cells} cells in a {width}-cell row: {row}");
    }
}
