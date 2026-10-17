//! backdrop — what a modal does to the surface behind it.

use ratatui::{layout::Rect, Frame};

use crate::theme::styles::ThemeStyles;

/// Dims the surface behind a modal overlay without erasing its highlighted rows.
///
/// A blanket `set_style(dim)` patches every cell's foreground, and nearly every bundled theme maps
/// `selectedBg` to the same color as `dim` — so the selected file row and diff line turned into
/// unreadable blocks. Cells already carrying the selection background step up to `muted` instead,
/// the same escape `ThemeStyles::secondary_on` makes for text on a highlighted row.
///
/// Backgrounds have to give too, and only the *slab* ones. A style patch touches the foreground
/// and leaves `bg` where it was, so a section heading kept its undiluted accent fill while the
/// triangle caps either side of it — which are foreground glyphs — faded with everything else. The
/// heading then read as the brightest thing on a backgrounded surface, which is the opposite of
/// what a backdrop is for. Diff tints and tool-card fills are *evidence* rather than chrome, so
/// they stay: a reader glancing past the modal should still see which lines were added.
pub fn dim_backdrop(frame: &mut Frame, area: Rect, styles: &ThemeStyles) {
    let selected_bg = styles.selected_bg.bg;
    let chrome = styles.slab.ramp();
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell_mut((x, y)) else {
                continue;
            };
            let highlighted = selected_bg.is_some_and(|background| cell.bg == background);
            if chrome.contains(&cell.bg) {
                // Back to the panel it sits on, so the label dims into its own surface exactly as
                // its caps do.
                cell.set_bg(styles.slab.canvas);
            }
            cell.set_style(if highlighted { styles.muted } else { styles.dim });
        }
    }
}
