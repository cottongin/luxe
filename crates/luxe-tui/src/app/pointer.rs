//! Pointer state — what the mouse is in the middle of, held across the events of one gesture.

/// A press, drag or hover in progress, and the selection it is making.
#[derive(Default)]
pub struct PointerState {
    /// Active transcript text selection (mouse drag), in rendered-line coordinates
    pub selection: Option<crate::app::ConversationSelection>,
    /// Surface a mouse drag is currently selecting on, if a button is down
    pub selection_drag: Option<crate::mouse::TextSurface>,
    /// The last left press, so the next one can tell a chained multi-click from a fresh one
    pub(crate) press_chain: Option<crate::selection::PressChain>,
    /// What the press over a text surface hit, held until the release proves the gesture was a
    /// click rather than the start of a drag.
    pub(crate) pending_click: Option<crate::mouse::MouseTarget>,
    /// Asks the next render to grow the transcript selection to the word under it — only the frame
    /// that laid the transcript out knows where a word ends.
    pub(crate) expand_word: bool,
    /// Asks the next render to extract and copy the transcript selection (only that frame knows
    /// the rendered text).
    pub copy_requested: bool,
    /// Scrollbar track being dragged: the surface it steers and the `(y, height)` it was grabbed
    /// on.
    ///
    /// Held across frames so the pointer keeps controlling the scroll position even after it
    /// wanders off the one-cell lane, which is how every other scrollbar behaves.
    pub scrollbar_drag: Option<(crate::mouse::ScrollRegionId, u16, u16)>,
    /// The divider being dragged, held across frames like [`Self::scrollbar_drag`] so the pointer
    /// keeps steering it once it leaves the one-cell rule
    pub divider_drag: Option<crate::mouse::DividerHandle>,
    /// The last press on a divider, for telling a double-click (reset the pane) from a drag
    pub(crate) divider_pressed: Option<(crate::workspace_layout::Pane, std::time::Instant)>,
    /// Whether a drag is picking lines in the Changes pane's File view
    pub peek_dragging: bool,
    /// Clickable target under the pointer, so hovering highlights an affordance the way keyboard
    /// focus does.
    pub hover: Option<crate::mouse::MouseTarget>,
}
