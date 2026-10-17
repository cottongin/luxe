//! Dividers — the rules between resizable panes, drawn and published for the mouse in one place.
//!
//! A divider lights when the pointer is on it and while it is being dragged, the way a multiplexer
//! shows which border it is about to move. Lit is the accent border; at rest it is the same muted
//! rule every other divider is.

use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::App;
use crate::mouse::{DividerHandle, MouseTarget};

/// Draws the rule in `rect` on its `border` side and publishes it as a handle on `handle`'s pane.
///
/// Args: `(frame, rule, border, hit area, handle, app)`. The hit area is the whole column the rule
/// stands in, which may be taller than the rule itself.
pub(super) fn render(
    f: &mut Frame,
    rule: Rect,
    border: Borders,
    hit: Rect,
    handle: DividerHandle,
    app: &App,
) {
    let style = match is_lit(app, handle) {
        true => app.theme_styles.border_accent,
        false => app.theme_styles.border_muted,
    };
    f.render_widget(Block::default().borders(border).border_style(style), rule);
    app.hit.borrow_mut().push(hit, MouseTarget::Divider(handle));
}

/// Whether `handle`'s divider is under the pointer or in the middle of a drag.
fn is_lit(app: &App, handle: DividerHandle) -> bool {
    let hovered =
        matches!(&app.pointer.hover, Some(MouseTarget::Divider(over)) if over.pane == handle.pane);
    let dragged = app.pointer.divider_drag.is_some_and(|dragging| dragging.pane == handle.pane);
    hovered || dragged
}
