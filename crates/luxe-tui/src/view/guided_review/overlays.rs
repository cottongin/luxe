//! Guided Review overlays — findings drawer, finish/confirm dialogs, finding details,
//! draft composer, outline, help, the source picker, and a pull request's whole description.
//!
//! Two overlay classes (design-tui-polish-and-mouse §A3, D5):
//! - **Modal** (source picker, outline, help, finding details, draft composer, finish review):
//!   a centered popup rendered over a **dimmed backdrop** (the caller in `super::render` mutes
//!   the surface first) so it reads as focused, not as clipping the text beneath it.
//! - **Drawer** (the findings panel): a non-modal right-edge companion with **no backdrop** —
//!   the surface stays bright so the reviewer keeps the diff in view while triaging findings.

mod description;
mod evidence;
mod finding;
mod finish;
mod help;

use crate::app::App;
pub(super) use description::render_pull_request_description;
pub(super) use evidence::render_finding_fix;
pub(super) use finding::{
    anchor_summary, chapter_kind_label, finding_state_glyph, finding_state_label,
    render_finding_details, render_finding_draft,
};
#[cfg(test)]
use finish::finish_carries;
pub(super) use finish::{render_commit_preview, render_finish_review};
pub(super) use help::{render_help, render_source_picker};
use ratatui::widgets::{Block, Padding};

/// A review overlay panel: the shared [`crate::widgets::titled_panel`] chrome (rounded accent
/// frame, accent glyph, bold label) plus a 1-col left pad so text isn't flush against the frame.
pub(super) fn overlay_block<'a>(app: &App, glyph: &str, label: &str) -> Block<'a> {
    crate::widgets::titled_panel(glyph, label, &app.theme_styles).padding(Padding::left(1))
}

#[cfg(test)]
mod tests;
