//! Theme picker — the overlay behind `/theme` and the palette's "Change theme".
//!
//! Every built-in, plus whatever the desktop resolved to, in one list. The point of it is the
//! **live preview**: moving the cursor repaints the whole app in that theme immediately, because a
//! palette is not something you can judge from its name. `Esc` puts back the one you arrived with;
//! `Enter` keeps it and writes it to the global config so it survives a relaunch.
//!
//! That preview is why the picker holds `opened_with`: a cancelled experiment has to leave no trace,
//! and the only way to guarantee that is to remember where you started rather than to re-derive it
//! from a config that has not been written yet.

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::Span,
    widgets::{Clear, List, ListItem},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Panel label, shared by the rendered title and the modal-width computation.
const TITLE: &str = "Theme";

/// Footer hint, shared by the rendered row and the modal-width computation.
const FOOTER: &str = " enter keep · ↑/↓ preview · esc cancel ";

/// Row suffix marking the theme the picker opened with.
const CURRENT_NOTE: &str = "  current";

/// What a keypress asked the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemePickerOutcome {
    /// Repaint in this theme without committing to it.
    Preview(String),
    /// Keep this theme, persist it, and close.
    Select(String),
    /// Close, putting back the theme the picker opened with.
    Cancel(String),
    /// Key consumed; nothing to repaint.
    Consumed,
}

/// Pure state for the theme picker overlay.
#[derive(Debug, Clone)]
pub struct ThemePickerState {
    themes: Vec<String>,
    selected: usize,
    /// The theme in force when the picker opened, restored on cancel.
    opened_with: String,
}

impl ThemePickerState {
    /// Open the picker over `themes`, with `active` preselected.
    pub fn open(themes: Vec<String>, active: &str) -> Self {
        let selected = themes.iter().position(|name| name == active).unwrap_or(0);
        Self { themes, selected, opened_with: active.to_string() }
    }

    /// The theme under the cursor.
    pub fn current(&self) -> &str {
        self.themes.get(self.selected).map_or(self.opened_with.as_str(), String::as_str)
    }

    /// The theme the picker opened with.
    pub fn opened_with(&self) -> &str {
        &self.opened_with
    }

    /// Rows on offer, for the renderer and for hit-testing.
    pub fn themes(&self) -> &[String] {
        &self.themes
    }

    /// Handle a key event.
    pub fn on_key(&mut self, key: &KeyEvent) -> ThemePickerOutcome {
        match key.code {
            KeyCode::Esc => ThemePickerOutcome::Cancel(self.opened_with.clone()),
            KeyCode::Enter => ThemePickerOutcome::Select(self.current().to_string()),
            KeyCode::Up | KeyCode::Char('k') => self.step(false),
            KeyCode::Down | KeyCode::Char('j') => self.step(true),
            KeyCode::Home => self.jump(0),
            KeyCode::End => self.jump(self.themes.len().saturating_sub(1)),
            _ => ThemePickerOutcome::Consumed,
        }
    }

    /// Move the cursor to `index`, previewing what lands under it.
    pub fn select(&mut self, index: usize) -> Option<String> {
        (index < self.themes.len() && index != self.selected).then(|| {
            self.selected = index;
            self.current().to_string()
        })
    }

    /// Wrapping step, so a list this short never dead-ends at either edge.
    fn step(&mut self, forward: bool) -> ThemePickerOutcome {
        if self.themes.is_empty() {
            return ThemePickerOutcome::Consumed;
        }
        let last = self.themes.len() - 1;
        self.selected = match (forward, self.selected) {
            (true, index) if index == last => 0,
            (true, index) => index + 1,
            (false, 0) => last,
            (false, index) => index - 1,
        };
        ThemePickerOutcome::Preview(self.current().to_string())
    }

    fn jump(&mut self, index: usize) -> ThemePickerOutcome {
        self.selected = index.min(self.themes.len().saturating_sub(1));
        ThemePickerOutcome::Preview(self.current().to_string())
    }
}

impl ThemePickerState {
    /// Render the picker centerd over `area`.
    ///
    /// Deliberately narrow. The whole app behind it *is* the preview, so the overlay's job is to
    /// stay out of the way of the thing being judged.
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
        let label_width = self.themes.iter().map(|name| name.width()).max().unwrap_or(0);
        // Cursor column plus the frame the selected row is cut with.
        let row_width = 2 + crate::widgets::SELECTION_FRAME + label_width + CURRENT_NOTE.width();
        let title_width = crate::widgets::panel_title_width(Icon::Palette.glyph(), TITLE);
        let natural = row_width.max(FOOTER.width()).max(title_width) as u16;
        let max_width = area.width.saturating_sub(4).max(24);
        let width = natural.saturating_add(3).clamp(24, max_width);
        let height = (self.themes.len() as u16 + 2 + crate::widgets::FOOTER_ROWS)
            .min(area.height.saturating_sub(2));
        let modal = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, modal);

        let block = crate::widgets::titled_panel(Icon::Palette.glyph(), TITLE, theme);
        let inner = block.inner(modal);
        frame.render_widget(block, modal);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(crate::widgets::FOOTER_ROWS)])
            .split(inner);

        // a short terminal shows the slice holding the selection, not the first rows of the list
        let window = crate::picker::Window::new(
            self.themes.len(),
            None,
            self.selected,
            usize::from(chunks[0].height),
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
                let name = &self.themes[index];
                let selected = index == self.selected;
                let background = if selected { theme.selected_bg } else { Style::default() };
                let cursor = crate::glyphs::Mark::Cursor.gutter(selected);
                let mut spans = vec![
                    Span::styled(cursor, theme.accent_on(selected)),
                    Span::styled(format!("{name:<label_width$}"), theme.label_on(selected)),
                ];
                // The one you arrived with, so a cancelled experiment has somewhere to point back to.
                if name == &self.opened_with {
                    spans.push(Span::styled(CURRENT_NOTE, theme.muted.patch(background)));
                }
                ListItem::new(crate::widgets::selection_row(
                    spans,
                    usize::from(chunks[0].width),
                    selected,
                    theme,
                ))
            })
            .collect();
        {
            // each drawn row clicks through to the theme it shows, wherever the window starts
            let mut regions = hit.borrow_mut();
            for (row, &index) in shown.iter().enumerate() {
                let row_y = chunks[0].y.saturating_add(row as u16);
                regions.push(
                    Rect::new(chunks[0].x, row_y, chunks[0].width, 1),
                    crate::mouse::MouseTarget::ThemeRow(index),
                );
            }
        }
        frame.render_widget(List::new(items), chunks[0]);
        crate::widgets::render_footer(frame, chunks[1], FOOTER, None, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a terminal too short for the whole list, the selected theme is drawn, and its row clicks
    /// through to it.
    ///
    /// The break this catches: the list drew from its first row whatever was selected, so a theme
    /// near the end could be chosen, previewed and kept while off screen.
    #[test]
    fn a_short_terminal_still_shows_the_selected_theme() {
        use ratatui::{backend::TestBackend, Terminal};
        let names: Vec<String> = (0..12).map(|index| format!("theme-{index:02}")).collect();
        let state = ThemePickerState::open(names, "theme-11");
        let styles = crate::theme::styles::ThemeStyles::from_resolved(
            &luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord"),
            "nord",
        );
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let mut terminal = Terminal::new(TestBackend::new(60, 9)).expect("terminal");
        terminal.draw(|frame| state.view(frame, frame.area(), &styles, &hit)).expect("drawn");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> =
            (0..9).map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect()).collect();
        let y = rows.iter().position(|row| row.contains("theme-11")).expect("the selection shows");
        let at = rows[y].find("theme-11").unwrap();
        let x = unicode_width::UnicodeWidthStr::width(&rows[y][..at]) as u16;
        assert_eq!(hit.borrow().at(x, y as u16), Some(&crate::mouse::MouseTarget::ThemeRow(11)));
        assert!(!rows.iter().any(|row| row.contains("theme-00")), "the list starts where it must");
        let mut state = state;
        let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
        let chosen = state.on_key(&enter);
        assert!(matches!(chosen, ThemePickerOutcome::Select(name) if name == "theme-11"));
    }

    /// A picker with no themes to window draws an empty list rather than panicking.
    #[test]
    fn an_empty_picker_draws() {
        use ratatui::{backend::TestBackend, Terminal};
        let styles = crate::theme::styles::ThemeStyles::from_resolved(
            &luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord"),
            "nord",
        );
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let empty = ThemePickerState::open(Vec::new(), "nord");
        let mut terminal = Terminal::new(TestBackend::new(60, 9)).expect("terminal");
        terminal.draw(|frame| empty.view(frame, frame.area(), &styles, &hit)).expect("no themes");
        assert!(hit.borrow().at(10, 2).is_none(), "and offers no row to click");
    }

    fn picker() -> ThemePickerState {
        ThemePickerState::open(
            ["luxe", "luxe-light", "nord", "gruvbox"].map(String::from).to_vec(),
            "nord",
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn it_opens_on_the_active_theme() {
        assert_eq!(picker().current(), "nord");
    }

    /// A theme nobody shipped — a custom file, or one removed since the config was written — must
    /// still open the picker rather than panic on a missing index.
    #[test]
    fn an_unknown_active_theme_opens_on_the_first_row() {
        let state = ThemePickerState::open(vec!["luxe".into()], "someone-elses");
        assert_eq!(state.current(), "luxe");
        assert_eq!(state.opened_with(), "someone-elses");
    }

    /// Every move previews, because a name is not something you can judge a palette by.
    #[test]
    fn moving_the_cursor_asks_for_a_preview() {
        let mut state = picker();
        assert_eq!(
            state.on_key(&key(KeyCode::Down)),
            ThemePickerOutcome::Preview("gruvbox".into())
        );
        assert_eq!(state.on_key(&key(KeyCode::Up)), ThemePickerOutcome::Preview("nord".into()));
        assert_eq!(state.on_key(&key(KeyCode::Home)), ThemePickerOutcome::Preview("luxe".into()));
    }

    /// A short list should not dead-end; wrapping costs nothing and saves a keypress.
    #[test]
    fn the_cursor_wraps_at_both_ends() {
        let mut state = picker();
        state.on_key(&key(KeyCode::End));
        assert_eq!(state.on_key(&key(KeyCode::Down)), ThemePickerOutcome::Preview("luxe".into()));
        assert_eq!(
            state.on_key(&key(KeyCode::Up)),
            ThemePickerOutcome::Preview("gruvbox".into()),
            "and back the other way"
        );
    }

    /// Cancelling names the theme to restore. Without it a cancelled experiment leaves the app in
    /// whatever the cursor last touched, which is the one thing a preview must never do.
    #[test]
    fn cancelling_names_the_theme_to_put_back() {
        let mut state = picker();
        state.on_key(&key(KeyCode::Down));
        state.on_key(&key(KeyCode::Down));
        assert_eq!(state.on_key(&key(KeyCode::Esc)), ThemePickerOutcome::Cancel("nord".into()));
    }

    #[test]
    fn enter_keeps_what_is_under_the_cursor() {
        let mut state = picker();
        state.on_key(&key(KeyCode::Down));
        assert_eq!(
            state.on_key(&key(KeyCode::Enter)),
            ThemePickerOutcome::Select("gruvbox".into())
        );
    }

    /// A click previews the row it landed on, and a click on the current row is not a change.
    #[test]
    fn clicking_a_row_previews_it_once() {
        let mut state = picker();
        assert_eq!(state.select(0), Some("luxe".to_string()));
        assert_eq!(state.select(0), None, "the same row again is not a repaint");
        assert_eq!(state.select(99), None, "and neither is a row that does not exist");
    }
}
