//! Marks — the signs the dock puts beside a todo and a changed file.
//!
//! Public so the phone app's renderer draws the same ones; the dock's colors stay with the dock,
//! and the app's stylesheet colors them by status.

use crate::glyphs::{GlyphTier, Mark};
use luxe_core::workspace::FileChangeStatus;
use luxe_core::TodoSummaryStatus;

/// The mark beside a todo in the PLAN checklist, at `tier`.
pub fn todo(status: TodoSummaryStatus, tier: GlyphTier) -> &'static str {
    match status {
        TodoSummaryStatus::Completed => Mark::Yes.at(tier),
        TodoSummaryStatus::InProgress => Mark::Underway.at(tier),
        TodoSummaryStatus::Blocked => Mark::Halted.at(tier),
        TodoSummaryStatus::Pending => "○",
    }
}

/// The letter Git's status reads as beside a changed file.
pub const fn change(status: FileChangeStatus) -> &'static str {
    match status {
        FileChangeStatus::Added => "A",
        FileChangeStatus::Modified => "M",
        FileChangeStatus::Deleted => "D",
        FileChangeStatus::Renamed => "R",
        FileChangeStatus::Untracked => "?",
    }
}
