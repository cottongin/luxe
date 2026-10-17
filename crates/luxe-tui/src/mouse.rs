//! Mouse hit-testing — per-frame regions the render pass publishes for `route_mouse`.
//!
//! ratatui is immediate-mode: the view computes rects every frame but retains nothing. To make the
//! mouse meaningful, the render pass records the on-screen rectangle of each scrollable, clickable
//! and selectable region into [`HitRegions`], which `route_mouse` then hit-tests against. The
//! registry lives behind a `RefCell` on `App` so the many `&App` render helpers can publish without
//! threading a `&mut` through the whole view layer.

use ratatui::layout::Rect;

use crate::selection::TextPoint;

/// A surface whose rendered text can be selected with the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSurface {
    /// The chat conversation transcript.
    Conversation,
    /// The prompt composer.
    Composer,
    /// The Changes pane's code: the File view, or one side of the diff.
    Peek(PeekSide),
}

/// Which text of the Changes pane a selection runs through.
///
/// A side-by-side diff draws two files on one row, so a drag down the right column has to stay in
/// the new file rather than pick up the deletions the left column shows beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekSide {
    /// Every row: the File view, or the unified diff
    Whole,
    /// The old file, the left column of a side-by-side diff
    Old,
    /// The new file, the right column of a side-by-side diff
    New,
}

/// A rendered transcript line's stable identity.
///
/// A selection recorded as a bare line index dies the moment the transcript reflows under it — a
/// card opening, a token arriving — which is every interesting moment. Pinning each end to the item
/// it landed in lets the next frame find it again wherever it moved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPin {
    /// Transcript item the line belongs to.
    pub item: crate::app::ConversationItemId,
    /// Which of that item's rendered lines it is.
    pub visual_line_within_item: usize,
}

/// A published text row: which surface drew it, which line it is, and where its first column sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRow {
    /// Surface that published the row.
    pub surface: TextSurface,
    /// Line index within that surface's rendered lines.
    pub line: usize,
    /// Screen x of the row's first cell.
    pub x: u16,
    /// Column of the whole line the row's first cell shows: nonzero for the second and later rows
    /// of a wrapped line, so a hit lands on the line's own column rather than the row's.
    pub first_column: usize,
    /// Stable identity of the row, for a surface that reflows under a live selection. The composer
    /// has none: its selection is buffer offsets, which reflow cannot move.
    pub pin: Option<TextPin>,
}

/// A cursor position resolved onto a text surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextHit {
    /// Surface the position belongs to.
    pub surface: TextSurface,
    /// Line + column within that surface.
    pub point: TextPoint,
    /// Stable identity of the hit line, when its surface publishes one.
    pub pin: Option<TextPin>,
}

/// A scrollable surface, resolved from the cursor position to route a wheel notch precisely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollRegionId {
    /// The chat conversation transcript.
    Conversation,
    /// The Guided Review diff body (scrolls source lines).
    ReviewDiff,
    /// The Guided Review guide pane (scrolls the file list).
    ReviewGuide,
    /// The Context Dock panel (scrolls its middle section list).
    Dock,
    /// One of the Context Dock's lists, which scrolls inside its own bounded viewport.
    DockList(crate::workspace_dock::DockList),
    /// The Changes pane's diff body (scrolls source rows).
    Changes,
    /// The project-context list (walks its rows).
    ContextFiles,
    /// The Guided Review feedback window's reading pane.
    ReviewFeedback,
    /// The pull request opening's description.
    PullRequestBody,
}

/// A clickable target, resolved from the cursor position to an action on left-click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MouseTarget {
    /// A row of the start screen's list, by its index among the rows that can be chosen.
    DashboardItem(usize),
    /// A Guided Review findings-inspector row (selects that finding).
    ReviewFindingRow(luxe_core::guided_review::FindingId),
    /// A command-palette row, by its index into the filtered list.
    PaletteRow(usize),
    /// A slash-command flyout row, by its index into the filtered list.
    SlashRow(usize),
    /// A permission-prompt option row, by its option index.
    PermissionRow(usize),
    /// A question-modal row (option or the free-text row), by its index.
    QuestionRow(usize),
    /// A mention (`@`) flyout row, by its index into the filtered list.
    MentionRow(usize),
    /// A model-picker row, by its index into the filtered list.
    ModelRow(usize),
    /// A login-wizard provider row, by its index into the filtered list.
    AuthProviderRow(usize),
    /// A login-wizard method row, by its index.
    AuthMethodRow(usize),
    /// A Configuration row — a way in, or a file to open — by its index in whichever is showing.
    ConfigRow(usize),
    /// A settings page in the list down the left, by its index.
    ConfigPage(usize),
    /// A project-context row, by its index into the list the surface is showing.
    ContextFileRow(usize),
    /// A thinking-level-picker row, by its index.
    ThinkingRow(usize),
    /// A theme-picker row, by its index.
    ThemeRow(usize),
    /// A session-picker row, by its index into the filtered list.
    SessionRow(usize),
    /// A Guided Review footer shortcut (the always-visible action bar).
    ReviewFooter(ReviewFooterAction),
    /// A Guided Review unified-diff row (moves the cursor/anchor to that absolute row index).
    ReviewDiffRow(usize),
    /// A collapsed run of mechanical changes (opens or re-collapses that file's churn).
    ReviewFoldedRun(luxe_core::guided_review::FileId),
    /// A guide file row (points the review at that file).
    ReviewGuideFile(luxe_core::guided_review::FileId),
    /// A guide chapter row (points the review at that chapter's first stop).
    ReviewGuideChapter(luxe_core::guided_review::ChapterId),
    /// The Context Dock's "preview" hint (opens the Changes pane on the file the list leads with).
    DockPeekChanges,
    /// The Context Dock "Review changes" affordance (opens Guided Review).
    DockReviewChanges,
    /// A Context Dock file row under ELSEWHERE, carrying the absolute path of a file the session
    /// changed outside the project.
    DockChangeFile(String),
    /// A Context Dock ELSEWHERE place heading; folds or unfolds the files under it.
    DockExternalPlace(String),
    /// A FILES file row, by project-relative path (shows it in the Changes pane).
    DockFile(String),
    /// A FILES folder row, by project-relative path (opens or closes it).
    DockFolder(String),
    /// The FILES scope switch: every file, or only what changed.
    DockFilesScope,
    /// The composer's title, which names the mode and switches to the next one
    ComposerMode,
    /// The FILES hint that puts the keyboard in the tree.
    DockFocusFiles,
    /// One line of a file in the Changes pane's File view, by its 1-based number. A drag across
    /// these picks lines to ask about.
    PeekLine(u32),
    /// What to do with the lines picked in the File view.
    PeekSelection(PeekSelectionAction),
    /// A row of an open context menu, by index.
    ContextMenuItem(usize),
    /// The Context Dock footer's model-name row (opens the model picker).
    DockModelRow,
    /// The statusline's changes module (opens the Changes pane).
    StatuslineChanges,
    /// The Context Dock footer's mode row, which switches to the next mode
    DockModeRow,
    /// A Changes-pane footer shortcut.
    ChangesFooter(ChangesFooterAction),
    /// A conversation transcript block (selects it for copy; a second click clears it).
    ConversationBlock(usize),
    /// A code block's "copy" affordance; carries the block's source text to put on the clipboard.
    CodeBlockCopy(String),
    /// An image's "open" affordance, offered when the terminal cannot draw it inline.
    OpenImage(crate::graphics::ImageKey),
    /// A row of an inline image. A double-click on one opens it full-window.
    ConversationImage(crate::graphics::ImageKey),
    /// A scrollbar track. Carries the surface it steers and the track's own geometry, so a press
    /// or drag maps the pointer row to a scroll position without the router knowing how any of it
    /// was laid out.
    Scrollbar { region: ScrollRegionId, y: u16, height: u16 },
    /// The divider beside a resizable pane: drag it to resize the pane, double-click it to put the
    /// pane back to its own width.
    Divider(DividerHandle),
}

/// What a drag on a divider needs to turn a pointer column into a pane width, published with it.
///
/// The router knows nothing about how any surface was laid out, the same deal a scrollbar track
/// gets: the renderer that drew the divider says where the pane's far edge is and how far it may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DividerHandle {
    /// The pane the divider resizes
    pub pane: crate::workspace_layout::Pane,
    /// The column of the pane's far edge: its left edge when it lies left of the divider, one past
    /// its right edge otherwise
    pub anchor: u16,
    /// Whether the pane lies left of the divider
    pub pane_on_left: bool,
    /// The narrowest the pane may be dragged
    pub min: u16,
    /// The widest the pane may be dragged
    pub max: u16,
}

impl DividerHandle {
    /// The pane's width with the divider at screen column `col`, held to its bounds.
    pub fn width_at(&self, col: u16) -> u16 {
        let width = match self.pane_on_left {
            true => col.saturating_sub(self.anchor),
            false => self.anchor.saturating_sub(col.saturating_add(1)),
        };
        width.clamp(self.min, self.max.max(self.min))
    }
}

impl MouseTarget {
    /// Whether this target sits inside a selectable text surface, so a press on it may still turn
    /// out to be the start of a drag and its action has to wait for the release.
    ///
    /// Chrome acts on the press, the way a button should. Anything drawn *over* text — an overlay
    /// row above the transcript — does too, because a press there was never going to select text.
    pub fn acts_on_release(&self) -> bool {
        matches!(
            self,
            Self::ConversationBlock(_)
                | Self::CodeBlockCopy(_)
                | Self::OpenImage(_)
                | Self::ConversationImage(_)
        )
    }
}

/// A clickable Changes-pane footer shortcut. Mapped to its `Msg` in `route_mouse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangesFooterAction {
    /// Show the previous changed file.
    FilePrev,
    /// Show the next changed file.
    FileNext,
    /// Soft-wrap long source lines, or stop.
    ToggleWordWrap,
    /// Draw the diff side by side, or unified.
    ToggleSplit,
    /// Open the full Guided Review on this file.
    OpenReview,
    /// Open the file in the reader's editor.
    OpenInEditor,
    /// Return the column to the transcript.
    Close,
    /// Show a changed file's diff, or its text.
    ToggleView,
    /// Put the keyboard in the pane.
    Focus,
    /// A key the focused pane answers to, named for the reader; a click on it does nothing.
    Hint,
}

/// What the action row under a File-view selection offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekSelectionAction {
    /// Put a mention of the lines in the composer
    Ask,
    /// Put the lines on the clipboard
    Copy,
    /// Open the file in the reader's editor, at the first picked line
    OpenInEditor,
    /// Let the selection go
    Clear,
}

/// A clickable Guided Review footer shortcut. Mapped to its `Msg` in `route_mouse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewFooterAction {
    HunkNext,
    ToggleNarration,
    ToggleNoise,
    ToggleNoiseDetails,
    ToggleSplit,
    ToggleWordWrap,
    ToggleGuide,
    ChapterNext,
    ChapterPrev,
    AskQuestion,
    AddComment,
    Feedback,
    Finish,
    ToggleSource,
    ToggleHelp,
    Exit,
}

/// Per-frame mouse hit-test registry. Cleared at the top of each render, then filled by the
/// surfaces that draw scrollable/clickable/selectable regions and read by `route_mouse`.
#[derive(Default, Debug)]
pub struct HitRegions {
    scrolls: Vec<(Rect, ScrollRegionId)>,
    clicks: Vec<(Rect, MouseTarget)>,
    texts: Vec<(Rect, TextRow)>,
}

impl HitRegions {
    /// Drops every region so the next frame starts clean.
    pub fn clear(&mut self) {
        self.scrolls.clear();
        self.clicks.clear();
        self.texts.clear();
    }

    /// Records `rect` as the hit area for a scrollable `region`.
    pub fn push_scroll(&mut self, rect: Rect, region: ScrollRegionId) {
        self.scrolls.push((rect, region));
    }

    /// Records `rect` as the hit area for a clickable `target`.
    pub fn push(&mut self, rect: Rect, target: MouseTarget) {
        self.clicks.push((rect, target));
    }

    /// The scrollable region under `(col, row)`, if any. Later pushes win, so a region drawn on
    /// top of another (e.g. an overlay) takes precedence.
    pub fn scroll_at(&self, col: u16, row: u16) -> Option<ScrollRegionId> {
        self.scrolls
            .iter()
            .rev()
            .find(|(rect, _)| rect_contains(rect, col, row))
            .map(|(_, region)| *region)
    }

    /// Every published click target, for tests that need to assert a surface offers one.
    pub fn clicks(&self) -> impl Iterator<Item = (Rect, &MouseTarget)> {
        self.clicks.iter().map(|(rect, target)| (*rect, target))
    }

    /// The clickable target under `(col, row)`, if any (topmost / last-pushed wins).
    pub fn at(&self, col: u16, row: u16) -> Option<&MouseTarget> {
        self.clicks.iter().rev().find(|(rect, _)| rect_contains(rect, col, row)).map(|(_, t)| t)
    }

    /// Records `rect` as one selectable text row.
    pub fn push_text(&mut self, rect: Rect, row: TextRow) {
        self.texts.push((rect, row));
    }

    /// The text position under `(col, row)`, if the cursor is over a selectable row.
    pub fn text_at(&self, col: u16, row: u16) -> Option<TextHit> {
        self.texts.iter().rev().find(|(rect, _)| rect_contains(rect, col, row)).map(|(_, text)| {
            TextHit {
                surface: text.surface,
                point: TextPoint::new(
                    text.line,
                    text.first_column + usize::from(col.saturating_sub(text.x)),
                ),
                pin: text.pin.clone(),
            }
        })
    }

    /// The vertical span `(top, bottom)` of `surface`'s published rows, so a drag can tell
    /// "pointer left the pane vertically" (scroll it) from "pointer is merely beside it".
    pub fn text_span_y(&self, surface: TextSurface) -> Option<(u16, u16)> {
        let mut span: Option<(u16, u16)> = None;
        for (rect, _) in self.texts.iter().filter(|(_, text)| text.surface == surface) {
            span = Some(match span {
                Some((top, bottom)) => (top.min(rect.y), bottom.max(rect.bottom())),
                None => (rect.y, rect.bottom()),
            });
        }
        span
    }

    /// The position on `surface` nearest to `(col, row)`, used while dragging past its edges so a
    /// selection keeps extending instead of freezing when the pointer leaves the pane.
    pub fn nearest_text(&self, surface: TextSurface, col: u16, row: u16) -> Option<TextHit> {
        let (rect, text) = self
            .texts
            .iter()
            .filter(|(_, text)| text.surface == surface)
            .min_by_key(|(rect, _)| vertical_distance(rect, row))?;
        let column = if col >= rect.right() {
            usize::from(rect.width)
        } else {
            usize::from(col.saturating_sub(text.x))
        };
        // Dragging below the surface should run to the end of the last row, not to its start.
        let column =
            if row >= rect.bottom() { usize::from(rect.width).max(column) } else { column };
        let point = TextPoint::new(text.line, text.first_column + column);
        Some(TextHit { surface, point, pin: text.pin.clone() })
    }
}

/// How far `row` sits outside `rect` vertically (0 when inside).
fn vertical_distance(rect: &Rect, row: u16) -> u16 {
    if row < rect.y {
        rect.y - row
    } else if row >= rect.bottom() {
        row - rect.bottom() + 1
    } else {
        0
    }
}

/// Whether `(col, row)` falls inside `rect` (half-open on the right/bottom, like ratatui areas).
fn rect_contains(rect: &Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.right() && row >= rect.y && row < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_at_finds_the_topmost_region_under_the_cursor() {
        let mut hit = HitRegions::default();
        hit.push_scroll(Rect::new(0, 0, 40, 20), ScrollRegionId::ReviewGuide);
        hit.push_scroll(Rect::new(40, 0, 60, 20), ScrollRegionId::ReviewDiff);

        assert_eq!(hit.scroll_at(10, 5), Some(ScrollRegionId::ReviewGuide));
        assert_eq!(hit.scroll_at(50, 5), Some(ScrollRegionId::ReviewDiff));
        // Outside every region.
        assert_eq!(hit.scroll_at(10, 40), None);
        // The boundary column belongs to the region that starts there (half-open rects).
        assert_eq!(hit.scroll_at(40, 5), Some(ScrollRegionId::ReviewDiff));
    }

    #[test]
    fn later_push_wins_on_overlap() {
        let mut hit = HitRegions::default();
        hit.push_scroll(Rect::new(0, 0, 100, 40), ScrollRegionId::Conversation);
        hit.push_scroll(Rect::new(10, 10, 20, 10), ScrollRegionId::ReviewDiff);
        assert_eq!(hit.scroll_at(15, 15), Some(ScrollRegionId::ReviewDiff));
        assert_eq!(hit.scroll_at(5, 5), Some(ScrollRegionId::Conversation));
    }

    #[test]
    fn clear_empties_the_registry() {
        let mut hit = HitRegions::default();
        hit.push_scroll(Rect::new(0, 0, 10, 10), ScrollRegionId::Conversation);
        hit.clear();
        assert_eq!(hit.scroll_at(1, 1), None);
    }
}
