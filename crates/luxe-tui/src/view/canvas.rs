//! Canvas — how wide the conversation's text runs, and how a peeked diff shares the column.

use crate::app::App;
use crate::workspace_layout;
use luxe_core::config::{ConversationAlignment, DockPosition};
use ratatui::layout::{Constraint, Direction, Layout};

pub(super) const PROSE_MAX_WIDTH: u16 = 120;
pub(super) const WIDE_BLOCK_MAX_WIDTH: u16 = 140;
pub(super) const SAFE_GUTTER_WIDTH: u16 = 2;
const SCROLLBAR_LANE_WIDTH: u16 = 1;

/// Horizontal padding (cells) inside a backgrounded block, so text never sits flush against either
/// edge of the band. [`conversation_canvas_widths_with_limits`] reserves it on both sides;
/// the transcript's `banded_line` paints the left one.
pub(super) const BLOCK_INSET: u16 = 1;

/// Conversation canvas measurements after reserving stable outer space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationCanvasWidths {
    /// Width left for content after gutters and the scrollbar lane.
    pub available_width: u16,
    /// Readable maximum for prose blocks.
    pub prose_width: u16,
    /// Larger maximum for code, tables, and tool details.
    pub wide_width: u16,
    /// Prose x offset within the conversation area.
    pub prose_x: u16,
    /// Wide-block x offset within the conversation area.
    pub wide_x: u16,
}

/// Calculates readable prose and wider block canvases with the default left alignment.
pub fn conversation_canvas_widths(area_width: u16) -> ConversationCanvasWidths {
    conversation_canvas_widths_with_limits(
        area_width,
        PROSE_MAX_WIDTH,
        WIDE_BLOCK_MAX_WIDTH,
        ConversationAlignment::Left,
    )
}

/// Calculates conversation canvases using resolved user preferences.
///
/// `alignment` controls how the (optionally width-capped) content sits in the available space:
/// `Left` anchors it to a small gutter, `Center` splits the slack evenly, and `Full` drops the
/// prose cap so content fills the whole width.
pub fn conversation_canvas_widths_with_limits(
    area_width: u16,
    prose_max_width: u16,
    wide_block_max_width: u16,
    alignment: ConversationAlignment,
) -> ConversationCanvasWidths {
    let reserved_width = SAFE_GUTTER_WIDTH + SCROLLBAR_LANE_WIDTH;
    let available_width = area_width.saturating_sub(reserved_width).max(1);
    // A banded block (a user message, a tool card) is painted the full `available_width` with its
    // text inset by [`BLOCK_INSET`], so content has to wrap that much narrower on *both* sides —
    // otherwise a line that fills the canvas ends up flush against the band's right edge.
    let content_width = available_width.saturating_sub(2 * BLOCK_INSET).max(1);
    let (prose_width, wide_width) = match alignment {
        ConversationAlignment::Full => (content_width, content_width),
        _ => (content_width.min(prose_max_width), content_width.min(wide_block_max_width)),
    };
    let (prose_x, wide_x) = match alignment {
        ConversationAlignment::Center => (
            SAFE_GUTTER_WIDTH / 2 + (content_width - prose_width) / 2,
            SAFE_GUTTER_WIDTH / 2 + (content_width - wide_width) / 2,
        ),
        _ => (SAFE_GUTTER_WIDTH / 2, SAFE_GUTTER_WIDTH / 2),
    };

    ConversationCanvasWidths { available_width, prose_width, wide_width, prose_x, wide_x }
}

/// Cells the conversation keeps beside a peeked diff before the diff takes the column alone.
///
/// Below this the transcript is a column of two-word lines, which is worse than not showing it:
/// the point of keeping it is that the exchange stays readable while you look at the code.
const MIN_PEEK_TRANSCRIPT_COLUMNS: u16 = 56;

/// Cells a peeked diff needs before it will share the column with the conversation.
///
/// A function of the width and nothing else — deliberately not of the layout the diff happens to be
/// drawing. Sizing the columns from that would make `Ctrl+B` summon and dismiss a transcript pane,
/// and a key that toggles a diff must toggle a diff.
const MIN_PEEK_DIFF_COLUMNS: u16 = 72;

/// Code cells a unified peek gives a line before the rest of the row goes to the conversation.
///
/// Most code is written to 80–120 columns, and a unified diff never draws wider than its longest
/// line. Every cell past that was a cell of empty diff and one fewer for the transcript beside it.
const PEEK_UNIFIED_CODE_COLUMNS: u16 = 120;

/// Code cells each side of a side-by-side peek gets, by the same reasoning, a little tighter since
/// there are two of them.
const PEEK_SPLIT_CODE_COLUMNS: u16 = 100;

/// Line-number digits the cap budgets for: a file of up to 9,999 lines gets its full measure.
const PEEK_GUTTER_DIGITS: u16 = 4;

/// The pane's own cells around the diff body: an inset on each side and the scrollbar.
const PEEK_PANE_CHROME: u16 = 3;

/// The widest a peeked diff is worth drawing, in each layout: its code measure plus everything a
/// row spends around it.
fn peek_diff_cap(split: bool) -> u16 {
    match split {
        // Old and new numbers, then the sign and its spacing
        false => PEEK_UNIFIED_CODE_COLUMNS + PEEK_GUTTER_DIGITS * 2 + 3 + PEEK_PANE_CHROME,
        // Per side a number and a sign, then the rule between the two
        true => {
            (PEEK_SPLIT_CODE_COLUMNS + PEEK_GUTTER_DIGITS + 2) * 2
                + u16::try_from(super::diff_lines::SPLIT_DIVIDER_CELLS).unwrap_or(3)
                + PEEK_PANE_CHROME
        }
    }
}

/// Cells a peeked diff takes in a main column of `width`, or `None` when it takes the whole column.
///
/// Whether the two share is a function of the width alone — deliberately not of the layout the
/// diff draws in, so `Ctrl+B` toggles a diff and never summons or dismisses the transcript. How
/// wide the diff is once they share depends on the layout: wide enough for code at a normal
/// measure (`peek_diff_cap`), and the conversation takes the rest. Capping the conversation and
/// giving the diff everything left would put a 180-cell unified diff with a hundred cells of
/// nothing on every row on a wide terminal.
///
/// Args: `(width, split)` — whether the diff will draw side by side.
pub fn peek_diff_width(width: u16, split: bool) -> Option<u16> {
    if width < MIN_PEEK_DIFF_COLUMNS + 1 + MIN_PEEK_TRANSCRIPT_COLUMNS {
        return None;
    }
    let most = width - (MIN_PEEK_TRANSCRIPT_COLUMNS + 1);
    Some(most.min(peek_diff_cap(split).max(MIN_PEEK_DIFF_COLUMNS)))
}

/// Cells the peeked diff takes in a main column of `width` with the app as it is, or `None` when
/// it takes the whole column: [`peek_diff_width`], told which layout the pane is about to draw.
///
/// Public because a test that reads the diff's own rows has to slice the same columns the renderer
/// drew, and a second copy of this arithmetic in the test file is a second thing to get wrong.
pub fn peek_diff_column(app: &App, width: u16) -> Option<u16> {
    let view = app.changes.as_ref()?;
    // The layout is decided at the widest the diff could be: a split that fits there fits at the
    // split cap too, which is sized to seat one
    let most = peek_diff_width(width, true)?;
    // A width the reader dragged the rule to is theirs, held to what still seats both columns
    if let Some(preferred) = app.panes.width(workspace_layout::Pane::Peek) {
        let (least, widest) = peek_width_bounds(width)?;
        return Some(preferred.clamp(least, widest));
    }
    // Inside its inset, which is the width the pane itself measures from
    let split = super::changes::splits_at(app, view, most.saturating_sub(2));
    peek_diff_width(width, split)
}

/// The range a dragged peek may span in a main column of `width`, or `None` when the diff takes
/// the whole column and there is no rule to drag.
pub fn peek_width_bounds(width: u16) -> Option<(u16, u16)> {
    peek_diff_width(width, false)?;
    Some((MIN_PEEK_DIFF_COLUMNS, width - (MIN_PEEK_TRANSCRIPT_COLUMNS + 1)))
}

/// Splits the main column into `(diff, chat)` when a peeked file can share it, else `(None, all)`.
///
/// The diff gets a column of its own, full height, and the conversation keeps its whole stack —
/// transcript, activity, tray, composer — in the other one. A prompt spanning both would say the
/// text goes to both, and a diff stopping short of the composer's rows would waste them.
///
/// The diff sits next to the dock that lists the files, whichever side the dock is on, because that
/// is the association a reader is using: click a name, see the file.
///
/// Three panes are safe *here* and not in the review for one reason: the composer owns the keyboard
/// throughout and the diff owns no keys of its own, so there is no focus for a reader to track. The
/// review's verbs are bare letters, and a third pane there would make every one of them a question
/// about which surface is listening.
pub(super) fn peek_columns(
    app: &App,
    stack: ratatui::layout::Rect,
) -> (Option<ratatui::layout::Rect>, ratatui::layout::Rect) {
    let Some(width) = peek_diff_column(app, stack.width) else {
        return (None, stack);
    };
    let chat = Constraint::Length(stack.width.saturating_sub(width + 1));
    let diff = Constraint::Length(width);
    let rule = Constraint::Length(1);
    let columns = Layout::default().direction(Direction::Horizontal);
    let (diff, chat) = match app.dock.position {
        DockPosition::Right => {
            let cols = columns.constraints([chat, rule, diff]).split(stack);
            (cols[2], cols[0])
        }
        DockPosition::Left | DockPosition::Unknown => {
            let cols = columns.constraints([diff, rule, chat]).split(stack);
            (cols[0], cols[2])
        }
    };
    (Some(diff), chat)
}
