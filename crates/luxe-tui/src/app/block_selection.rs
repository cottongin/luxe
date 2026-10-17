//! Block selection — picking a transcript block, expanding it, copying it.

use super::*;

impl App {
    /// Puts `text` on the clipboard and confirms it on screen.
    ///
    /// Every copy path goes through here: with the mouse captured, a drag doesn't produce a
    /// *terminal* selection, so the terminal's own copy key has nothing to grab and the user needs
    /// to see that the copy already happened.
    pub fn record_copy(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.status.flash_copied();
        self.outbox.copy = Some(text);
    }

    /// Move the selected block up by one. Wraps from first to last.
    pub fn select_block_up(&mut self) {
        conversation::select_up(self);
    }

    /// Move the selected block down by one. Wraps from last to first.
    pub fn select_block_down(&mut self) {
        conversation::select_down(self);
    }

    /// Expands or collapses one block's hidden detail (a mouse click on its row).
    ///
    /// Returns whether the block had anything to show, so a click on plain prose can stay a no-op.
    pub fn toggle_block_details_at(&mut self, index: usize) -> bool {
        conversation::toggle_details_at(self, index)
    }

    /// Records a left press at `(col, row)` and returns how much text it claims.
    pub(crate) fn press_chain(
        &mut self,
        col: u16,
        row: u16,
        now: std::time::Instant,
    ) -> crate::selection::SelectionSpan {
        let chain = crate::selection::PressChain::next(self.pointer.press_chain, (col, row), now);
        self.pointer.press_chain = Some(chain);
        chain.span
    }

    /// Toggles details for the currently selected structured tool item.
    pub fn toggle_selected_tool_details(&mut self) -> bool {
        conversation::toggle_selected_details(self)
    }

    /// Get the text of the selected block (or the last block if none selected).
    pub fn selected_block_text(&self) -> Option<String> {
        conversation::selected_text(self)
    }

    /// Copy the selected block's text. Returns true if a block was copied.
    pub fn copy_selected_block(&mut self) -> bool {
        conversation::copy_selected(self)
    }
}
