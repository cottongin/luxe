//! Overlay lists — what the wheel and the scrollbar do to a modal list that follows its cursor.
//!
//! Every such list draws the window around its cursor rather than keeping a scroll offset of its
//! own, so both gestures move the cursor: a notch walks it a row, the way the arrows do, and a press
//! or drag on the bar puts it at that point of the list. A view that drifted off its cursor would
//! leave Enter choosing a row nobody can see.

use super::App;
use crate::configuration::ConfigOutcome;
use crate::mouse::OverlayList;

impl App {
    /// Where `list`'s cursor is and how many rows it has, while it is open.
    pub(crate) fn overlay_cursor(&self, list: OverlayList) -> Option<(usize, usize)> {
        let overlays = &self.overlays;
        match list {
            OverlayList::Models => {
                overlays.model_picker.as_ref().map(|picker| (picker.cursor(), picker.match_count()))
            }
            OverlayList::Providers => overlays
                .auth_wizard
                .as_ref()
                .and_then(|wizard| Some((wizard.selected_row(), wizard.provider_rows()?))),
            OverlayList::Palette => {
                overlays.palette.as_ref().map(|palette| (palette.selected(), palette.match_count()))
            }
            OverlayList::Themes => {
                overlays.theme_picker.as_ref().map(|picker| (picker.cursor(), picker.len()))
            }
            OverlayList::Sessions => overlays
                .session_picker
                .as_ref()
                .map(|picker| (picker.cursor(), picker.match_count())),
            OverlayList::Slash => overlays
                .slash_flyout
                .as_ref()
                .map(|flyout| (flyout.cursor(), flyout.filtered_items().len())),
            OverlayList::Mention => overlays
                .mention
                .as_ref()
                .map(|mention| (mention.cursor(), mention.filtered_items().len())),
            OverlayList::Configuration => {
                overlays.configuration.as_ref().map(|it| it.list_cursor())
            }
        }
    }

    /// Puts `list`'s cursor on `row`, previewing a theme the way arrowing onto it does.
    fn put_overlay_cursor(&mut self, list: OverlayList, row: usize) {
        let overlays = &mut self.overlays;
        match list {
            OverlayList::Models => overlays.model_picker.as_mut().map(|picker| picker.select(row)),
            OverlayList::Providers => {
                overlays.auth_wizard.as_mut().map(|wizard| wizard.select_provider(row))
            }
            OverlayList::Palette => overlays.palette.as_mut().map(|palette| palette.select(row)),
            OverlayList::Themes => {
                let preview = overlays.theme_picker.as_mut().and_then(|picker| picker.select(row));
                preview.map(|theme| self.preview_theme(&theme))
            }
            OverlayList::Sessions => {
                overlays.session_picker.as_mut().map(|picker| picker.select(row))
            }
            OverlayList::Slash => overlays.slash_flyout.as_mut().map(|flyout| flyout.select(row)),
            OverlayList::Mention => overlays.mention.as_mut().map(|mention| mention.select(row)),
            OverlayList::Configuration => {
                let moved = overlays.configuration.as_mut().map(|surface| surface.select(row));
                // Repaints in the setup theme under the cursor, as a key onto it does
                moved.map(|()| self.apply_configuration_outcome(ConfigOutcome::Consumed))
            }
        };
    }

    /// Walks `list`'s cursor one row, stopping at either end rather than wrapping as the arrows
    /// do: a wheel spun past the last row would otherwise fling the reader back to the top.
    pub(crate) fn walk_overlay_list(&mut self, list: OverlayList, up: bool) {
        let Some((cursor, rows)) = self.overlay_cursor(list) else { return };
        let row = match up {
            true => cursor.saturating_sub(1),
            false => (cursor + 1).min(rows.saturating_sub(1)),
        };
        if row != cursor {
            self.put_overlay_cursor(list, row);
        }
    }

    /// Puts `list`'s cursor at the point of the list that `row` of a `height`-tall bar stands for.
    pub(crate) fn drag_overlay_list(&mut self, list: OverlayList, row: u16, height: u16) {
        let Some((_, rows)) = self.overlay_cursor(list) else { return };
        let target = crate::widgets::drag_offset(row, height, rows.saturating_sub(1));
        self.put_overlay_cursor(list, target);
    }
}

#[cfg(test)]
mod tests;
