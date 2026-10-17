//! picker — shared list-nav state + renderer for the composer flyouts (`/` slash, `@` mention).
//!
//! Both flyouts are the same widget: a bordered list that grows up from the composer, scrolls a
//! fixed window, highlights the focused row, and publishes per-row click targets. Only four things
//! actually differ between them — how items are *filtered* (fuzzy rank vs. a ref provider), what a
//! *row* shows (label + optional detail), which *mouse target* a row maps to, and the title
//! glyph/label naming the surface. This module owns the common 90%; each flyout keeps its own item
//! type, `on_key`, and `Outcome`.
//!
//! [`PickerNav`] is the wider of the two abstractions here: the modal pickers (session, project,
//! model) draw entirely different chrome but navigate identically, so they share the state machine
//! without sharing the renderer.

mod layout;
mod nav;
mod rows;
mod view;

pub(crate) use layout::{render_pane, tab_line, wrapped_rows, Slot, Split, Window, PANE_GUTTER};
pub use nav::PickerNav;
pub use view::{render_picker, DetailFn, Edge, PickerRow, PickerView, TABBED_ROWS};

/// Max rows shown before the list scrolls (keeps the flyout from swallowing the transcript).
const MAX_VISIBLE_ROWS: usize = 8;

/// Rows a tabbed picker spends above its list: the tabs, then the rule under them.
///
/// The rule is the one the palette and the model picker draw under their search field, so a
/// header reads as a header on every list surface rather than as a first row that happens to be
/// in the accent.
const TAB_ROWS: u16 = 2;

#[cfg(test)]
mod tests;
