//! Thinking-level picker — the overlay behind `/thinking` and the palette's "Set thinking level".
//!
//! A short fixed list (Off … Ultra), gated by what the active model actually accepts: a level its
//! [`ModelInfo`] can't honor is drawn dim, labeled, skipped by the cursor, and deaf to a click, so
//! the list only ever offers choices that survive `ModelInfo::effective_thinking_level`. Selecting a
//! level sends `Action::SetThinkingLevel`.

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::provider::{ModelInfo, ThinkingLevel};
use ratatui::{
    layout::Rect,
    style::Style,
    text::Span,
    widgets::{Clear, List, ListItem},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Panel label, shared by the rendered title and the modal-width computation.
const TITLE: &str = "Thinking level";

/// Row suffix marking a level the active model can't honor. The frame's bottom title names the
/// model, so the row itself doesn't have to repeat it.
const UNSUPPORTED_NOTE: &str = "  not supported";

/// Row suffix marking the level this model starts every session on (`models.json`'s `thinking`).
/// Without it the setting is invisible: the level would just *be* different here for no stated
/// reason.
const DEFAULT_NOTE: &str = "  default";

/// Footer hint, shared by the rendered row and the modal-width computation.
const FOOTER: &str = " enter apply · ↑/↓ choose · esc close ";

/// Outcome of a thinking-picker keypress.
#[derive(Clone, Debug, PartialEq)]
pub enum ThinkingPickerOutcome {
    /// Apply this level and close.
    Select(ThinkingLevel),
    /// Close without changing.
    Close,
    /// Key consumed; picker stays open.
    Consumed,
}

/// Pure state for the thinking-level picker overlay.
#[derive(Debug, Clone)]
pub struct ThinkingPickerState {
    levels: Vec<ThinkingLevel>,
    selected: usize,
    /// Active model, used to annotate which levels are actually supported.
    model: Option<ModelInfo>,
}

impl ThinkingPickerState {
    /// Open the picker with `current` preselected, gated against `model` when known.
    ///
    /// The cursor never starts on a level the model can't honor: the engine would clamp it anyway,
    /// so the picker opens on the level a keypress would actually apply.
    pub fn open(current: ThinkingLevel, model: Option<ModelInfo>) -> Self {
        let levels = ThinkingLevel::ALL.to_vec();
        let effective = match &model {
            Some(model) => model.effective_thinking_level(current),
            None => current,
        };
        let selected = levels.iter().position(|level| *level == effective).unwrap_or(0);
        Self { levels, selected, model }
    }

    /// Handle a key event. Returns the outcome (select/close/consumed).
    pub fn on_key(&mut self, key: &KeyEvent) -> ThinkingPickerOutcome {
        match key.code {
            KeyCode::Esc => ThinkingPickerOutcome::Close,
            KeyCode::Enter => ThinkingPickerOutcome::Select(self.levels[self.selected]),
            KeyCode::Up | KeyCode::Char('k') => {
                self.step(false);
                ThinkingPickerOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.step(true);
                ThinkingPickerOutcome::Consumed
            }
            _ => ThinkingPickerOutcome::Consumed,
        }
    }

    /// The highlighted level.
    pub fn selected_level(&self) -> ThinkingLevel {
        self.levels[self.selected]
    }

    /// Moves the selection to `index` (a mouse click or hover on that row).
    ///
    /// Returns whether the row is selectable at all: an unsupported level leaves the selection
    /// where it was, so clicking one does nothing rather than silently applying a clamp.
    pub fn select(&mut self, index: usize) -> bool {
        match self.levels.get(index) {
            Some(level) if self.supports(*level) => {
                self.selected = index;
                true
            }
            _ => false,
        }
    }

    /// Whether the active model honors `level` (always true when no model is known).
    pub fn supports(&self, level: ThinkingLevel) -> bool {
        match &self.model {
            None => true,
            Some(model) => model.effective_thinking_level(level) == level,
        }
    }

    /// Moves the cursor one selectable row up or down, wrapping and skipping the levels this model
    /// can't honor. `Off` is always honorable, so the walk can't come up empty.
    fn step(&mut self, forward: bool) {
        let len = self.levels.len();
        for offset in 1..=len {
            let index = if forward {
                (self.selected + offset) % len
            } else {
                (self.selected + len - offset) % len
            };
            if self.supports(self.levels[index]) {
                self.selected = index;
                return;
            }
        }
    }

    /// Renders the picker as a compact centered modal.
    pub fn view(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        if area.width < 24 || area.height < 6 {
            return;
        }
        // Fit the widest level row (cursor + label + the unsupported note), the footer, and both
        // titles, plus borders and one right-pad cell — no fixed width with trailing dead space.
        let label_width = self.label_width();
        // Cursor column plus the frame the selected row is cut with.
        let level_width = 2
            + crate::widgets::SELECTION_FRAME
            + label_width
            + UnicodeWidthStr::width(UNSUPPORTED_NOTE).max(UnicodeWidthStr::width(DEFAULT_NOTE));
        let footer_width = UnicodeWidthStr::width(FOOTER);
        let title_width = crate::widgets::panel_title_width(Icon::Thinking.glyph(), TITLE);
        // The keys and the model share the footer row, a gap between them
        let model_width =
            self.model_note().map_or(0, |note| UnicodeWidthStr::width(note.as_str()) + 2);
        let natural = level_width.max(footer_width + model_width).max(title_width) as u16;
        let max_width = area.width.saturating_sub(4).max(24);
        let width = natural.saturating_add(3).clamp(24, max_width);
        let height = (self.levels.len() as u16 + 2 + crate::widgets::FOOTER_ROWS)
            .min(area.height.saturating_sub(2));
        let modal = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, modal);

        let block = crate::widgets::titled_panel(Icon::Thinking.glyph(), TITLE, theme);
        // Name the model beside the keys so a dimmed row reads as "not supported *by this
        // model*" without every row having to say so.
        let note = self.model_note();
        let list =
            crate::widgets::render_footed(frame, block, modal, FOOTER, note.as_deref(), theme);

        // a short terminal shows the slice holding the selection, not the first rows of the list
        let window = crate::picker::Window::new(
            self.levels.len(),
            None,
            self.selected,
            usize::from(list.height),
        );
        let shown: Vec<usize> = window
            .slots()
            .filter_map(|slot| match slot {
                crate::picker::Slot::Row(index) => Some(index),
                crate::picker::Slot::Rule => None,
            })
            .collect();
        let items: Vec<ListItem> = shown
            .iter()
            .map(|&index| {
                let level = &self.levels[index];
                let selected = index == self.selected;
                let background = if selected { theme.selected_bg } else { Style::default() };
                let supported = self.supports(*level);
                // Only a selectable row gets the cursor and the bright label; a disabled one stays
                // de-emphasized, since weight would read as "available".
                let cursor = crate::glyphs::Mark::Cursor.gutter(selected);
                let label_style =
                    if supported { theme.label_on(selected) } else { theme.secondary_on(false) };
                let mut spans = vec![
                    Span::styled(cursor, theme.accent_on(selected)),
                    // Pad every label so the notes line up in one column.
                    Span::styled(format!("{:<label_width$}", pretty_level(*level)), label_style),
                ];
                if !supported {
                    spans.push(Span::styled(UNSUPPORTED_NOTE, theme.muted.patch(background)));
                } else if self.is_configured_default(*level) {
                    spans.push(Span::styled(DEFAULT_NOTE, theme.muted.patch(background)));
                }
                ListItem::new(crate::widgets::selection_row(
                    spans,
                    usize::from(list.width),
                    selected,
                    theme,
                ))
            })
            .collect();
        {
            // each drawn row clicks through to the level it shows, wherever the window starts
            let mut regions = hit.borrow_mut();
            for (row, &index) in shown.iter().enumerate() {
                let row_y = list.y.saturating_add(row as u16);
                regions.push(
                    Rect::new(list.x, row_y, list.width, 1),
                    crate::mouse::MouseTarget::ThinkingRow(index),
                );
            }
        }
        frame.render_widget(List::new(items), list);
    }

    /// The footer's note naming the model the annotations refer to.
    fn model_note(&self) -> Option<String> {
        self.model.as_ref().map(|model| model.display().to_string())
    }

    /// Whether `level` is where this model's config says a session starts.
    fn is_configured_default(&self, level: ThinkingLevel) -> bool {
        self.model.as_ref().and_then(|model| model.default_thinking) == Some(level)
    }

    /// Cells the widest level label needs.
    fn label_width(&self) -> usize {
        self.levels
            .iter()
            .map(|level| UnicodeWidthStr::width(pretty_level(*level)))
            .max()
            .unwrap_or(0)
    }
}

/// Title-cased label for a level (`xhigh` → `X-High`, `ultra` → `Ultra`).
fn pretty_level(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "Off",
        ThinkingLevel::Minimal => "Minimal",
        ThinkingLevel::Low => "Low",
        ThinkingLevel::Medium => "Medium",
        ThinkingLevel::High => "High",
        ThinkingLevel::XHigh => "X-High",
        ThinkingLevel::Ultra => "Ultra",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a terminal too short for every level, the selected one is drawn and clicks through to it.
    #[test]
    fn a_short_terminal_still_shows_the_selected_level() {
        use ratatui::{backend::TestBackend, Terminal};
        let state = ThinkingPickerState::open(ThinkingLevel::Ultra, None);
        let styles = crate::theme::styles::ThemeStyles::from_resolved(
            &luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord"),
            "nord",
        );
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).expect("terminal");
        terminal.draw(|frame| state.view(frame, frame.area(), &styles, &hit)).expect("drawn");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> =
            (0..8).map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect()).collect();
        let label = pretty_level(ThinkingLevel::Ultra);
        let y = rows.iter().position(|row| row.contains(label)).expect("the selection shows");
        let at = rows[y].find(label).unwrap();
        let x = unicode_width::UnicodeWidthStr::width(&rows[y][..at]) as u16;
        let index = state.levels.iter().position(|level| *level == ThinkingLevel::Ultra).unwrap();
        assert_eq!(
            hit.borrow().at(x, y as u16),
            Some(&crate::mouse::MouseTarget::ThinkingRow(index))
        );
    }
    use crossterm::event::KeyModifiers;
    use std::collections::HashMap;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn opens_on_the_current_level() {
        let picker = ThinkingPickerState::open(ThinkingLevel::High, None);
        assert_eq!(picker.selected_level(), ThinkingLevel::High);
    }

    #[test]
    fn arrows_wrap_and_enter_selects() {
        let mut picker = ThinkingPickerState::open(ThinkingLevel::Off, None);
        assert_eq!(picker.on_key(&key(KeyCode::Up)), ThinkingPickerOutcome::Consumed);
        assert_eq!(picker.selected_level(), ThinkingLevel::Ultra);
        let outcome = picker.on_key(&key(KeyCode::Enter));
        assert_eq!(outcome, ThinkingPickerOutcome::Select(ThinkingLevel::Ultra));
    }

    /// A model that reasons but tops out at High — the shape of every extended-thinking Claude.
    fn up_to_high() -> ModelInfo {
        ModelInfo {
            reasoning: true,
            thinking_level_map: HashMap::from([
                ("high".to_string(), Some("high".to_string())),
                ("xhigh".to_string(), None),
                ("ultra".to_string(), None),
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn unsupported_levels_are_detected_from_the_model() {
        let picker = ThinkingPickerState::open(ThinkingLevel::Off, Some(up_to_high()));
        assert!(picker.supports(ThinkingLevel::High));
        assert!(!picker.supports(ThinkingLevel::XHigh));
        assert!(!picker.supports(ThinkingLevel::Ultra));
    }

    /// A level the model can't honor is not a choice: the cursor jumps over it, a click on it is
    /// ignored, and Enter therefore can never apply one.
    #[test]
    fn unsupported_levels_are_not_selectable() {
        let mut picker = ThinkingPickerState::open(ThinkingLevel::High, Some(up_to_high()));
        assert_eq!(picker.selected_level(), ThinkingLevel::High);

        // Down from the top supported level wraps past X-High/Ultra to Off.
        picker.on_key(&key(KeyCode::Down));
        assert_eq!(picker.selected_level(), ThinkingLevel::Off);
        // Up from Off skips them in the other direction too.
        picker.on_key(&key(KeyCode::Up));
        assert_eq!(picker.selected_level(), ThinkingLevel::High);

        let ultra_row = ThinkingLevel::ALL.iter().position(|l| *l == ThinkingLevel::Ultra).unwrap();
        assert!(!picker.select(ultra_row), "an unsupported row refuses the selection");
        assert_eq!(picker.selected_level(), ThinkingLevel::High, "the selection stays put");
        let medium_row =
            ThinkingLevel::ALL.iter().position(|l| *l == ThinkingLevel::Medium).unwrap();
        assert!(picker.select(medium_row));
        assert_eq!(picker.selected_level(), ThinkingLevel::Medium);
    }

    /// Opening on a level the model can't honor would offer a choice the engine then clamps, so the
    /// cursor starts where the engine would land.
    #[test]
    fn opening_on_an_unsupported_level_snaps_to_the_effective_one() {
        let picker = ThinkingPickerState::open(ThinkingLevel::Ultra, Some(up_to_high()));
        assert_eq!(picker.selected_level(), ThinkingLevel::High);
    }

    /// A model with no reasoning at all leaves Off as the only choice.
    #[test]
    fn a_non_reasoning_model_offers_only_off() {
        let model = ModelInfo { reasoning: false, ..Default::default() };
        let mut picker = ThinkingPickerState::open(ThinkingLevel::High, Some(model));
        assert_eq!(picker.selected_level(), ThinkingLevel::Off);
        for _ in 0..3 {
            picker.on_key(&key(KeyCode::Down));
            assert_eq!(picker.selected_level(), ThinkingLevel::Off, "nothing else is selectable");
        }
        assert!(picker.supports(ThinkingLevel::Off));
        assert!(!picker.supports(ThinkingLevel::Low));
    }

    /// The row a model starts every session on says so, and only that row.
    ///
    /// Otherwise the setting is invisible where it matters most: the level would simply *be*
    /// different on this model than on the last one, with nothing on screen saying why.
    #[test]
    fn the_configured_starting_level_is_marked_on_its_row() {
        let model = ModelInfo { default_thinking: Some(ThinkingLevel::Medium), ..up_to_high() };
        let picker = ThinkingPickerState::open(ThinkingLevel::High, Some(model));
        assert!(picker.is_configured_default(ThinkingLevel::Medium));
        for level in ThinkingLevel::ALL.iter().filter(|l| **l != ThinkingLevel::Medium) {
            assert!(!picker.is_configured_default(*level), "{level:?} is not the configured one");
        }

        // A model that declares nothing marks nothing — including Off, which is where it starts.
        let picker = ThinkingPickerState::open(ThinkingLevel::High, Some(up_to_high()));
        assert!(ThinkingLevel::ALL.iter().all(|level| !picker.is_configured_default(*level)));
        let unknown = ThinkingPickerState::open(ThinkingLevel::High, None);
        assert!(ThinkingLevel::ALL.iter().all(|level| !unknown.is_configured_default(*level)));
    }

    #[test]
    fn esc_closes() {
        let mut picker = ThinkingPickerState::open(ThinkingLevel::Off, None);
        assert_eq!(picker.on_key(&key(KeyCode::Esc)), ThinkingPickerOutcome::Close);
    }
}
