//! Workspace layout — deterministic terminal-width classification.
//!
//! The renderer consumes these measurements before it lays out conversation and Context Dock
//! surfaces, and `gutter_inset` answers the one question left over: how far from the edges it
//! meets each of those surfaces draws. Keeping this math pure prevents resize behavior from
//! depending on snapshot data.

use ratatui::layout::Rect;

/// The terminal width at which the Context Dock may render beside the conversation.
pub const WIDE_LAYOUT_MIN_WIDTH: u16 = 136;
/// The smallest terminal width that keeps the conversation as the primary surface.
pub const STANDARD_LAYOUT_MIN_WIDTH: u16 = 80;
/// Width of the left rail shared by the Context Dock (chat) and the Guided Review guide
/// pane, so the vertical divider never jumps when moving between chat and review. Sized for
/// the richer review guide (chapter titles, file rows + stats, graph insight); the dock
/// inherits it and spends the extra room on its otherwise-truncated change paths.
pub const LEFT_RAIL_WIDTH: u16 = 42;
/// The fixed width of the side-by-side or overlay Context Dock (the shared left rail).
pub const CONTEXT_DOCK_WIDTH: u16 = LEFT_RAIL_WIDTH;
/// The divider between side-by-side conversation and dock surfaces.
pub const CONTEXT_DOCK_SEPARATOR_WIDTH: u16 = 1;

/// Cells a surface keeps between itself and a rule luxe drew beside it.
const INNER_GUTTER: u16 = 1;

/// `rect`, inset by a column on each side that faces something luxe drew, and flush on each side
/// that sits on an edge of `frame`.
///
/// The two edges are different questions. A rule luxe owns needs a column of air or text runs
/// into it. The terminal's edge does not: air there is a column the reader doesn't get, and a
/// host with chrome of its own (herdr draws its tab row flush to the pane's left edge) would
/// disagree with luxe about where the content starts. Every surface that touches an edge insets
/// through here, so the answer lives in one place.
///
/// Degenerate widths collapse the rect rather than wrapping, matching
/// [`ratatui::layout::Rect::inner`].
pub(crate) fn gutter_inset(rect: Rect, frame: Rect) -> Rect {
    let left = if rect.x <= frame.x { 0 } else { INNER_GUTTER };
    let right = if rect.right() >= frame.right() { 0 } else { INNER_GUTTER };
    let spent = left + right;
    if rect.width < spent {
        return Rect::ZERO;
    }
    Rect { x: rect.x.saturating_add(left), width: rect.width - spent, ..rect }
}

/// A pane the reader can widen or narrow by dragging the divider beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The Context Dock beside the conversation
    Dock,
    /// The guide beside a Guided Review's diff
    ReviewGuide,
    /// A peeked diff beside the conversation
    Peek,
}

/// The widths the reader dragged the panes to, and where they are kept.
///
/// In memory by default, which is what a test or an embedder gets: nothing it drags reaches the
/// reader's state file. The TUI points it at the runtime state when it launches, the same as the
/// commands it remembers.
#[derive(Clone, Debug, Default)]
pub struct PaneLayout {
    widths: luxe_core::runtime_state::PaneWidths,
    store: Option<std::path::PathBuf>,
}

impl PaneLayout {
    /// Reads what `path` remembers, and keeps writing it there.
    pub fn persisted(path: std::path::PathBuf) -> Self {
        let widths = luxe_core::runtime_state::RuntimeState::load_from(&path).panes;
        Self { widths, store: Some(path) }
    }

    /// The width the reader gave `pane`, if they gave it one.
    pub fn width(&self, pane: Pane) -> Option<u16> {
        match pane {
            Pane::Dock => self.widths.dock,
            Pane::ReviewGuide => self.widths.review_guide,
            Pane::Peek => self.widths.peek,
        }
    }

    /// Sizes `pane` to `width`, or back to its own default with `None`.
    pub fn set(&mut self, pane: Pane, width: Option<u16>) {
        let slot = match pane {
            Pane::Dock => &mut self.widths.dock,
            Pane::ReviewGuide => &mut self.widths.review_guide,
            Pane::Peek => &mut self.widths.peek,
        };
        *slot = width;
    }

    /// Writes the widths to the state file, when there is one.
    ///
    /// Once a drag is over rather than on every step of it: a drag moves the divider dozens of times
    /// a second, and only where it stops is worth remembering.
    pub fn save(&self) {
        if let Some(path) = &self.store {
            luxe_core::runtime_state::RuntimeState::remember_panes_at(path, self.widths);
        }
    }
}

/// The narrowest the Context Dock is dragged to: its telemetry rows still seat a value beside their
/// label column.
pub const DOCK_MIN_WIDTH: u16 = 34;
/// The widest the Context Dock is dragged to. Past this a row is a sentence, not a label, and the
/// conversation is the thing being read.
pub const DOCK_MAX_WIDTH: u16 = 80;
/// Cells the conversation keeps beside a side-by-side dock, however wide it is dragged.
pub const MIN_CONVERSATION_BESIDE_DOCK: u16 = 80;
/// The narrowest the review's guide is dragged to; chapter titles still read at this measure.
pub const GUIDE_MIN_WIDTH: u16 = 32;
/// Cells the review's diff keeps beside the guide, however wide the guide is dragged.
pub const MIN_DIFF_BESIDE_GUIDE: u16 = 72;

/// The dock's width beside a conversation in a `terminal_width`-wide terminal: the reader's when
/// they dragged one, else the rail's, within what keeps the conversation readable.
pub fn side_by_side_dock_width(terminal_width: u16, preferred: Option<u16>) -> u16 {
    let most = terminal_width
        .saturating_sub(CONTEXT_DOCK_SEPARATOR_WIDTH + MIN_CONVERSATION_BESIDE_DOCK)
        .clamp(DOCK_MIN_WIDTH, DOCK_MAX_WIDTH);
    preferred.unwrap_or(CONTEXT_DOCK_WIDTH).clamp(DOCK_MIN_WIDTH, most)
}

/// The range a dragged dock width is held to in a `terminal_width`-wide terminal.
pub fn dock_width_bounds(terminal_width: u16) -> (u16, u16) {
    (DOCK_MIN_WIDTH, side_by_side_dock_width(terminal_width, Some(DOCK_MAX_WIDTH)))
}

/// The range a dragged guide width is held to in a `terminal_width`-wide review.
pub fn guide_width_bounds(terminal_width: u16) -> (u16, u16) {
    let most = terminal_width
        .saturating_sub(CONTEXT_DOCK_SEPARATOR_WIDTH + MIN_DIFF_BESIDE_GUIDE)
        .max(GUIDE_MIN_WIDTH);
    (GUIDE_MIN_WIDTH, most)
}

/// Cells the guide pane takes: the reader's width when they dragged one, held to what leaves the
/// diff its room, else [`guide_pane_width`].
pub fn guide_width(terminal: u16, preferred: Option<u16>) -> u16 {
    match preferred {
        Some(width) => {
            let (least, most) = guide_width_bounds(terminal);
            width.clamp(least, most)
        }
        None => guide_pane_width(terminal),
    }
}

/// Cells the guide pane takes, given the whole terminal.
///
/// Sized for prose rather than for the dock. Chapter rationale is the most important text in the
/// surface, and wrapping it in 38 cells beside a 120-cell diff would be exactly backwards, since
/// prose wants a 60-to-75 character measure and code wants room. A third of the terminal, floored
/// at the dock's width so the divider does not jump on a terminal with nothing to spare, and capped
/// before the measure gets too long to track a line across.
pub fn guide_pane_width(terminal: u16) -> u16 {
    (terminal / 3).clamp(LEFT_RAIL_WIDTH, 76)
}

/// Describes how the workspace uses the available terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLayout {
    /// Context replaces the conversation when explicitly opened.
    Compact,
    /// Context overlays the conversation when explicitly opened.
    Standard,
    /// Context may share space with the conversation.
    Wide,
}

/// Precomputed widths for a workspace render pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceLayoutMetrics {
    /// The responsive layout category.
    pub layout: WorkspaceLayout,
    /// The complete terminal width supplied to the resolver.
    pub terminal_width: u16,
    /// Width reserved for the Context Dock when it is visible.
    pub dock_width: u16,
    /// Width reserved for the side-by-side divider.
    pub separator_width: u16,
    /// Width left for the conversation in a side-by-side layout.
    pub conversation_width: u16,
}

/// Resolves workspace dimensions with saturating math for every terminal width, at the default
/// dock width.
pub fn resolve_workspace_layout(terminal_width: u16) -> WorkspaceLayoutMetrics {
    resolve_workspace_layout_with(terminal_width, None)
}

/// [`resolve_workspace_layout`] with the dock at the width the reader dragged it to, if they did.
///
/// The layout tier does not move with it: whether the dock sits beside the conversation is a
/// question about the terminal, and a dock dragged wide narrows the conversation rather than
/// pushing the dock over it.
pub fn resolve_workspace_layout_with(
    terminal_width: u16,
    preferred_dock: Option<u16>,
) -> WorkspaceLayoutMetrics {
    let layout = if terminal_width >= WIDE_LAYOUT_MIN_WIDTH {
        WorkspaceLayout::Wide
    } else if terminal_width >= STANDARD_LAYOUT_MIN_WIDTH {
        WorkspaceLayout::Standard
    } else {
        WorkspaceLayout::Compact
    };

    let (dock_width, separator_width) = match layout {
        WorkspaceLayout::Wide => {
            (side_by_side_dock_width(terminal_width, preferred_dock), CONTEXT_DOCK_SEPARATOR_WIDTH)
        }
        WorkspaceLayout::Standard => (
            preferred_dock
                .unwrap_or(CONTEXT_DOCK_WIDTH)
                .clamp(DOCK_MIN_WIDTH, DOCK_MAX_WIDTH)
                .min(terminal_width),
            0,
        ),
        WorkspaceLayout::Compact => (terminal_width, 0),
    };
    let conversation_width = match layout {
        WorkspaceLayout::Wide => terminal_width.saturating_sub(dock_width + separator_width),
        WorkspaceLayout::Standard | WorkspaceLayout::Compact => terminal_width,
    };

    WorkspaceLayoutMetrics {
        layout,
        terminal_width,
        dock_width,
        separator_width,
        conversation_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dragged dock keeps its width within what still leaves the conversation readable, and the
    /// default is the rail it always was.
    #[test]
    fn a_dragged_dock_is_held_to_what_the_terminal_can_seat() {
        assert_eq!(side_by_side_dock_width(200, None), CONTEXT_DOCK_WIDTH);
        assert_eq!(side_by_side_dock_width(200, Some(60)), 60);
        assert_eq!(side_by_side_dock_width(200, Some(10)), DOCK_MIN_WIDTH);
        assert_eq!(side_by_side_dock_width(400, Some(200)), DOCK_MAX_WIDTH);
        // The narrowest side-by-side terminal still gives the conversation its eighty
        let width = WIDE_LAYOUT_MIN_WIDTH;
        let dock = side_by_side_dock_width(width, Some(DOCK_MAX_WIDTH));
        assert_eq!(width - dock - CONTEXT_DOCK_SEPARATOR_WIDTH, MIN_CONVERSATION_BESIDE_DOCK);
        let metrics = resolve_workspace_layout_with(width, Some(DOCK_MAX_WIDTH));
        assert_eq!(metrics.layout, WorkspaceLayout::Wide, "dragging never changes the tier");
        assert_eq!(metrics.conversation_width, MIN_CONVERSATION_BESIDE_DOCK);
    }

    /// A dragged guide leaves the diff its room; without a drag it is the terminal's share.
    #[test]
    fn a_dragged_guide_leaves_the_diff_its_room() {
        assert_eq!(guide_width(180, None), guide_pane_width(180));
        assert_eq!(guide_width(180, Some(50)), 50);
        assert_eq!(guide_width(180, Some(10)), GUIDE_MIN_WIDTH);
        assert_eq!(guide_width(180, Some(170)), 180 - 1 - MIN_DIFF_BESIDE_GUIDE);
    }

    /// A dragged width is remembered only where the TUI keeps one.
    #[test]
    fn pane_widths_reach_the_state_file_only_when_one_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let mut kept = PaneLayout::persisted(path.clone());
        kept.set(Pane::Peek, Some(90));
        kept.save();
        assert_eq!(PaneLayout::persisted(path.clone()).width(Pane::Peek), Some(90));

        let mut in_memory = PaneLayout::default();
        in_memory.set(Pane::Dock, Some(50));
        in_memory.save();
        assert_eq!(PaneLayout::persisted(path).width(Pane::Dock), None);
    }

    #[test]
    fn classifies_locked_width_boundaries() {
        assert_eq!(resolve_workspace_layout(79).layout, WorkspaceLayout::Compact);
        assert_eq!(resolve_workspace_layout(80).layout, WorkspaceLayout::Standard);
        assert_eq!(resolve_workspace_layout(135).layout, WorkspaceLayout::Standard);
        assert_eq!(resolve_workspace_layout(136).layout, WorkspaceLayout::Wide);
    }

    #[test]
    fn wide_layout_reserves_fixed_dock_and_divider() {
        assert_eq!(
            resolve_workspace_layout(136),
            WorkspaceLayoutMetrics {
                layout: WorkspaceLayout::Wide,
                terminal_width: 136,
                dock_width: 42,
                separator_width: 1,
                conversation_width: 93,
            }
        );
    }

    #[test]
    fn overlay_and_replacement_leave_conversation_unreflowed() {
        let standard = resolve_workspace_layout(120);
        assert_eq!(standard.dock_width, CONTEXT_DOCK_WIDTH);
        assert_eq!(standard.separator_width, 0);
        assert_eq!(standard.conversation_width, 120);

        let compact = resolve_workspace_layout(78);
        assert_eq!(compact.dock_width, 78);
        assert_eq!(compact.separator_width, 0);
        assert_eq!(compact.conversation_width, 78);
    }

    /// The terminal's edge is drawn flush; a side facing something luxe drew keeps its air.
    #[test]
    fn a_gutter_tells_the_terminal_edge_from_one_luxe_drew() {
        let frame = Rect::new(0, 0, 100, 40);
        let left_rail = Rect::new(0, 0, 42, 40);
        let main = Rect::new(43, 0, 57, 40);

        assert_eq!(gutter_inset(left_rail, frame), Rect::new(0, 0, 41, 40));
        assert_eq!(gutter_inset(main, frame), Rect::new(44, 0, 56, 40));
        assert_eq!(gutter_inset(frame, frame), frame, "the whole frame is the whole frame");
    }

    /// A rect with nothing to give collapses rather than wrapping around the frame.
    #[test]
    fn a_gutter_cannot_inset_more_than_there_is() {
        let frame = Rect::new(0, 0, 100, 40);
        assert_eq!(gutter_inset(Rect::new(50, 0, 1, 40), frame), Rect::ZERO);
        assert_eq!(gutter_inset(Rect::new(50, 0, 2, 40), frame), Rect::new(51, 0, 0, 40));
        assert_eq!(gutter_inset(Rect::new(0, 0, 1, 40), frame), Rect::new(0, 0, 0, 40));
    }

    #[test]
    fn tiny_terminal_widths_do_not_underflow() {
        for width in [0, 1, 37, 38, 79] {
            let metrics = resolve_workspace_layout(width);
            assert_eq!(metrics.layout, WorkspaceLayout::Compact);
            assert_eq!(metrics.dock_width, width);
            assert_eq!(metrics.conversation_width, width);
            assert_eq!(metrics.separator_width, 0);
        }
    }
}
