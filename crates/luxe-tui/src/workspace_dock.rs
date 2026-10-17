//! Workspace dock state — UI-local visibility and scroll state.
//!
//! The Context Dock is a purely **informational** panel: it summarizes the project, plan and
//! changes at a glance. It has no section selection or keyboard navigation — every
//! action it surfaces (open the model picker, review changes) is a mouse click or a slash command /
//! palette entry. The only interaction it owns is scrolling when its content outgrows the panel.
//! This module resolves user preference and temporary project-local overrides before rendering; it
//! knows nothing about snapshots or terminal widgets.

use luxe_core::config::{DockMode, DockPosition};

mod sections;
pub use sections::{
    change_rows, DockChangeGroup, DockChangeItem, DockChangeRow, DockChanges, DockExternal,
    DockExternalGroup, DockIdentity, DockPlan, DockPlanItem, DockSectionModel,
};

use crate::workspace_layout::WorkspaceLayout;

/// How far one wheel notch / scroll step moves the dock's content.
const SCROLL_STEP: usize = 3;

/// One of the dock's lists, each of which scrolls inside a viewport of its own.
///
/// The panel scroll moves *between* sections; these move within one, so a sixty-file change or a
/// forty-step plan stays reachable without pushing its neighbors off the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockList {
    /// The session's checklist.
    Plan,
    /// The project's files, with what changed marked in them.
    Files,
    /// What the session changed outside the project.
    Elsewhere,
}

impl DockList {
    /// Every list, in the order the rail draws them.
    pub const ALL: [Self; 3] = [Self::Plan, Self::Files, Self::Elsewhere];

    /// This list's slot in the per-list measurements the renderer publishes.
    pub const fn slot(self) -> usize {
        match self {
            Self::Plan => 0,
            Self::Files => 1,
            Self::Elsewhere => 2,
        }
    }
}

/// What the last layout measured about one dock list, so a gesture can move it without
/// re-deriving the layout.
///
/// Published by the renderer each frame (interior-mutable: render is `&App`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DockListMetrics {
    /// Largest useful offset for the current list and viewport.
    pub max: usize,
    /// Offset the list sits at while nobody has taken it over — the top for most, the window
    /// around the work in hand for PLAN.
    pub follow: usize,
}

/// A temporary project-local visibility choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockRuntimeOverride {
    /// Follow the persisted configuration.
    None,
    /// Keep Context open across responsive layout changes.
    Shown,
    /// Keep Context closed across responsive layout changes.
    Hidden,
}

/// The visual form Context uses for the resolved workspace layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPresentation {
    /// Context is not visible.
    Hidden,
    /// Context shares the workspace beside the conversation.
    SideBySide,
    /// Context overlays the unchanged conversation.
    Overlay,
    /// Context replaces the conversation surface.
    Replacement,
}

/// Presentation-only Context Dock state for the current project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockState {
    /// Persisted user preference from layered UI config.
    pub configured: DockMode,
    /// Which side the full-height dock occupies.
    pub position: DockPosition,
    /// Temporary project-local visibility choice.
    pub runtime_override: DockRuntimeOverride,
    /// Offset into the scrollable middle region (mouse wheel over the panel).
    pub scroll: usize,
    /// Where the reader has taken PLAN, or `None` while it follows the work in hand.
    plan_scroll: Option<usize>,
    /// Where the reader has taken FILES, or `None` while it follows the file in hand.
    files_scroll: Option<usize>,
    /// Where the reader has taken ELSEWHERE, or `None` while it sits at the top.
    elsewhere_scroll: Option<usize>,
}

impl DockState {
    /// Creates a fresh dock state from the resolved persisted preference.
    ///
    /// The side defaults to the left; callers apply the configured [`DockPosition`] afterward.
    pub fn new(configured: DockMode) -> Self {
        Self {
            configured,
            position: DockPosition::Left,
            runtime_override: DockRuntimeOverride::None,
            scroll: 0,
            plan_scroll: None,
            files_scroll: None,
            elsewhere_scroll: None,
        }
    }

    /// Resolves Context visibility without inspecting project data or render state.
    pub fn presentation(&self, layout: WorkspaceLayout) -> DockPresentation {
        let shown = match self.runtime_override {
            DockRuntimeOverride::Shown => true,
            DockRuntimeOverride::Hidden => false,
            DockRuntimeOverride::None => match self.configured {
                DockMode::Shown => true,
                DockMode::Hidden => false,
                DockMode::Auto | DockMode::Unknown => layout == WorkspaceLayout::Wide,
            },
        };
        if !shown {
            return DockPresentation::Hidden;
        }

        match layout {
            WorkspaceLayout::Wide => DockPresentation::SideBySide,
            WorkspaceLayout::Standard => DockPresentation::Overlay,
            WorkspaceLayout::Compact => DockPresentation::Replacement,
        }
    }

    /// Temporarily shows Context for the current project.
    pub fn show(&mut self) {
        self.runtime_override = DockRuntimeOverride::Shown;
    }

    /// Temporarily hides Context for the current project.
    pub fn hide(&mut self) {
        self.runtime_override = DockRuntimeOverride::Hidden;
        self.rewind();
    }

    /// Clears the temporary project-local choice and returns to automatic behavior.
    pub fn reset_auto(&mut self) {
        self.runtime_override = DockRuntimeOverride::None;
        self.rewind();
    }

    /// Puts every scroll back where a freshly opened panel would have it.
    fn rewind(&mut self) {
        self.scroll = 0;
        self.plan_scroll = None;
        self.files_scroll = None;
        self.elsewhere_scroll = None;
    }

    /// Toggles Context without making auto mode resize-sensitive again.
    pub fn toggle(&mut self, layout: WorkspaceLayout) {
        if self.presentation(layout) == DockPresentation::Hidden {
            self.show();
        } else {
            self.hide();
        }
    }

    /// Swaps the dock between the left and right edges.
    pub fn toggle_position(&mut self) {
        self.position = match self.position {
            DockPosition::Right => DockPosition::Left,
            DockPosition::Left | DockPosition::Unknown => DockPosition::Right,
        };
    }

    /// Scrolls the dock content by one step (a wheel notch), clamped at the top by `max`.
    ///
    /// `max` is the largest useful offset for the current content/viewport; the renderer owns the
    /// exact bound, so callers pass it from the last layout.
    pub fn scroll_by(&mut self, up: bool, max: usize) {
        self.scroll = if up {
            self.scroll.saturating_sub(SCROLL_STEP)
        } else {
            (self.scroll + SCROLL_STEP).min(max)
        };
    }

    /// Where `list` is drawn from this frame, given what the last layout measured.
    pub fn list_scroll(&self, list: DockList, metrics: DockListMetrics) -> usize {
        self.taken(list).unwrap_or(metrics.follow).min(metrics.max)
    }

    /// Scrolls `list` by one step, clamped at both ends.
    ///
    /// Landing back on the follow position hands the list back rather than pinning it there: PLAN
    /// resumes tracking the work in hand, and a list scrolled back to its top is simply at its top.
    pub fn scroll_list_by(&mut self, list: DockList, up: bool, metrics: DockListMetrics) {
        let from = self.list_scroll(list, metrics);
        let to = if up {
            from.saturating_sub(SCROLL_STEP)
        } else {
            (from + SCROLL_STEP).min(metrics.max)
        };
        self.take(list, to, metrics);
    }

    /// Jumps `list` to the position a thumb dragged to `row` of a `height`-tall track points at.
    pub fn drag_list(&mut self, list: DockList, row: u16, height: u16, metrics: DockListMetrics) {
        self.take(list, crate::widgets::drag_offset(row, height, metrics.max), metrics);
    }

    /// Records where the reader put `list`, or hands it back when that is where it was going
    /// anyway.
    fn take(&mut self, list: DockList, offset: usize, metrics: DockListMetrics) {
        let taken = (offset != metrics.follow).then_some(offset);
        match list {
            DockList::Plan => self.plan_scroll = taken,
            DockList::Files => self.files_scroll = taken,
            DockList::Elsewhere => self.elsewhere_scroll = taken,
        }
    }

    /// Hands `list` back to following, so the row the keyboard moved to is the one in view.
    pub fn follow_list(&mut self, list: DockList) {
        match list {
            DockList::Plan => self.plan_scroll = None,
            DockList::Files => self.files_scroll = None,
            DockList::Elsewhere => self.elsewhere_scroll = None,
        }
    }

    /// Where the reader put `list`, if they have.
    const fn taken(&self, list: DockList) -> Option<usize> {
        match list {
            DockList::Plan => self.plan_scroll,
            DockList::Files => self.files_scroll,
            DockList::Elsewhere => self.elsewhere_scroll,
        }
    }
}

impl Default for DockState {
    fn default() -> Self {
        Self::new(DockMode::Auto)
    }
}

#[cfg(test)]
mod tests;
