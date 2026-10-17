//! Dock anchors — clickable rows as section-relative indices, rebased onto the panel's middle,
//! and where a scrolling list's window sits.

use super::{row_width, SCROLLBAR_LANE};
use crate::app::App;
use crate::workspace_dock::{DockList, DockListMetrics};
use ratatui::text::Line;

/// One clickable region of the dock, as a `middle`-line index and the cells it occupies.
pub(super) struct DockHit {
    /// Index into the scrollable middle region.
    pub(super) line: usize,
    /// Cells from the content's left edge, or the whole row when absent.
    ///
    /// A row carrying two affordances (peek and review share one line) needs each to answer for
    /// its own words; everything else claims the row it is on.
    pub(super) columns: Option<(u16, u16)>,
    /// What a click there means.
    pub(super) target: crate::mouse::MouseTarget,
}

impl DockHit {
    /// A hit covering a whole row.
    pub(super) fn row(line: usize, target: crate::mouse::MouseTarget) -> Self {
        Self { line, columns: None, target }
    }
}

/// Clickable dock anchors as `middle`-line indices, published as hit regions by `render`.
#[derive(Default)]
pub(super) struct DockHitAnchors {
    /// Everything a pointer can act on in the scrollable middle.
    pub(super) hits: Vec<DockHit>,
    /// The footer's clickable rows, when telemetry is here: the model (opens the picker) and the
    /// mode (switches it).
    pub(super) footer_rows: Vec<(usize, crate::mouse::MouseTarget)>,
    /// The lists scrolling inside viewports of their own, each with a lane to draw.
    pub(super) lists: Vec<DockListAnchor>,
}

/// One scrolling list, as `render` needs it to place a lane and say what steers it.
///
/// Only present while the list has somewhere to go: a lane on a list that fits would claim there
/// is more, and the wheel over it belongs to the panel.
pub(super) struct DockListAnchor {
    /// Which list, so a wheel notch or a dragged thumb reaches the right one.
    pub(super) list: DockList,
    /// Half-open span of lines the list's rows occupy.
    pub(super) rows: (usize, usize),
    /// Where the window sits in the whole list.
    pub(super) offset: usize,
    /// Rows in the whole list.
    pub(super) total: usize,
    /// Rows of the list the window shows, where a row can wrap and that is fewer than the lines
    /// the window spans. `None` when every row is one line.
    pub(super) window: Option<usize>,
}

/// A built dock section: its lines, what a pointer can act on in them, and the list inside it that
/// scrolls on its own.
///
/// Anchors are section-relative until [`DockSection::land`] rebases them onto the middle region.
#[derive(Default)]
pub(super) struct DockSection {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) hits: Vec<DockHit>,
    pub(super) list: Option<DockListAnchor>,
}

impl DockSection {
    /// Appends the section to `middle`, rebasing its anchors onto where it landed.
    pub(super) fn land(self, middle: &mut Vec<Line<'static>>, anchors: &mut DockHitAnchors) {
        let base = middle.len();
        anchors
            .hits
            .extend(self.hits.into_iter().map(|hit| DockHit { line: base + hit.line, ..hit }));
        anchors.lists.extend(
            self.list.map(|list| DockListAnchor {
                rows: (base + list.rows.0, base + list.rows.1),
                ..list
            }),
        );
        middle.extend(self.lines);
    }
}

/// Where a dock list is drawn from this frame, and the cells its rows have.
///
/// Publishing what the layout measured is what lets a wheel notch and a dragged thumb move the
/// list without re-deriving any of it. `follow` is where the list sits while nobody has taken it
/// over — the top for most, the window around the work in hand for PLAN.
pub(super) fn list_window(
    app: &App,
    list: DockList,
    total: usize,
    cap: usize,
    follow: usize,
) -> (usize, usize) {
    let metrics = DockListMetrics { max: total.saturating_sub(cap), follow };
    app.set_dock_list_metrics(list, metrics);
    let lane = if total > cap { SCROLLBAR_LANE } else { 0 };
    (app.dock.list_scroll(list, metrics), row_width(app).saturating_sub(lane))
}

/// The anchor a list hands `render`, or nothing when it has nowhere to go.
pub(super) fn scrolls(
    list: DockList,
    rows: (usize, usize),
    offset: usize,
    total: usize,
    cap: usize,
) -> Option<DockListAnchor> {
    (total > cap && rows.1 > rows.0).then_some(DockListAnchor {
        list,
        rows,
        offset,
        total,
        window: None,
    })
}
