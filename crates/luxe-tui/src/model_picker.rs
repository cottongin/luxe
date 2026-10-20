//! Model picker — the searchable overlay behind `/model` and the palette's "Change model".
//!
//! Pure state + rendering. The CLI assembles the [`ModelEntry`] catalog (configured models from
//! `models.json`, enriched/extended by a cached models.dev catalog); this module only filters,
//! renders, and returns the chosen [`ModelInfo`]. Selecting one sends `Action::SetModel`.

use crate::glyphs::Icon;
use crate::picker::PickerNav;
use crate::theme::styles::ThemeStyles;
use crate::widgets::truncate_end as truncate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::provider::{ModelInfo, ModelRef};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

mod meta;
use meta::meta_suffix;

/// Panel label, shared by the rendered title and the modal-width computation.
const TITLE: &str = "Select model";

/// The model catalog + active model the TUI runner installs at startup.
#[derive(Debug, Clone, Default)]
pub struct ModelMenu {
    /// Browsable models (configured + models.dev extras).
    pub catalog: Vec<ModelEntry>,
    /// Full info for the model the engine starts on, when known.
    pub active: Option<ModelInfo>,
}

/// Builds the menu on demand.
///
/// The CLI owns config and catalog files, so it supplies this instead of a fixed snapshot; the
/// picker calls it each time it opens, which is what lets an edited `models.json` or a
/// freshly-refreshed models.dev cache show up without relaunching.
pub type ModelMenuSource = std::sync::Arc<dyn Fn() -> ModelMenu + Send + Sync>;

/// Builds the menu again, having asked anything that can be asked first.
///
/// [`ModelMenuSource`] reads files and must not block a frame; this one is allowed to make a
/// request, so the UI spawns it and applies the answer when it lands. What it exists for is a
/// local server: its model list is a fact about this machine a moment ago, and a model loaded in
/// LM Studio after the last connect would otherwise never appear.
pub type ModelMenuProbe = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelMenu> + Send>>
        + Send
        + Sync,
>;

/// Both ways the runner can rebuild the menu, as one thing to hand over.
///
/// They are never useful apart — the probe is the same menu with a question asked first — and
/// passing them separately is two more arguments on a function that already has enough.
#[derive(Clone)]
pub struct ModelSources {
    /// Rebuilds from what is on disk. Cheap enough to run while a frame is being laid out.
    pub build: ModelMenuSource,
    /// Rebuilds it again off the frame, having asked a local server what it serves.
    pub probe: Option<ModelMenuProbe>,
}

impl ModelSources {
    /// Sources that read what is on disk, with nothing to ask.
    pub fn reading(build: ModelMenuSource) -> Self {
        Self { build, probe: None }
    }
}

/// One selectable model row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelEntry {
    /// Full model info sent to the engine on selection.
    pub info: ModelInfo,
    /// Display name (models.dev name when known, else the model id).
    pub label: String,
    /// Whether this model is declared in the user's config (vs. a models.dev browse extra).
    pub configured: bool,
    /// Whether this is a catalog guess for an endpoint that can say what it serves, and did not.
    ///
    /// An Azure resource lists its deployments; until it has, the catalog's models are Azure's
    /// offer rather than the resource's, and the row says so instead of passing for one.
    pub unverified: bool,
}

impl ModelEntry {
    /// `provider/id` — the stable key shown as a subtitle and searched against.
    pub fn qualified_id(&self) -> String {
        format!("{}/{}", self.info.r#ref.provider, self.info.r#ref.id)
    }
}

/// Outcome of a model-picker keypress.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelPickerOutcome {
    /// Switch to this model and close the picker.
    Select(Box<ModelInfo>),
    /// Close without switching.
    Close,
    /// Key consumed; picker stays open.
    Consumed,
}

/// Pure state machine for the model picker overlay.
#[derive(Debug, Clone)]
pub struct ModelPickerState {
    items: Vec<ModelEntry>,
    nav: PickerNav,
    active: Option<ModelRef>,
    note: Vec<String>,
}

impl ModelPickerState {
    /// Open the picker over a catalog, marking `active` as the current model.
    pub fn open(items: Vec<ModelEntry>, active: Option<ModelRef>) -> Self {
        let mut nav = PickerNav::new(items.len());
        // Start on the active model when it's in the list, so the current choice is preselected.
        // Nothing is filtered yet, so an item's index is also its row.
        if let Some(row) =
            active.as_ref().and_then(|it| items.iter().position(|entry| &entry.info.r#ref == it))
        {
            nav.select(row);
        }
        Self { items, nav, active, note: Vec::new() }
    }

    /// Opens with something to say first — what a login landed, and which model it left you on.
    ///
    /// A successful sign-in walks straight into this picker rather than pausing on a screen that
    /// only reports success, so the report rides along here. Each line is drawn as written and
    /// truncated to the panel, which is what keeps the measured height honest.
    pub fn with_note(mut self, note: Vec<String>) -> Self {
        self.note = note;
        self
    }

    /// What the picker says above its list.
    #[cfg(test)]
    pub(crate) fn note(&self) -> &[String] {
        &self.note
    }

    /// Rows the note costs: its lines plus the blank one under them.
    fn note_rows(&self) -> u16 {
        match self.note.is_empty() {
            true => 0,
            false => self.note.len() as u16 + 1,
        }
    }

    /// Handle a key event. Returns the outcome (select/close/consumed).
    pub fn on_key(&mut self, key: &KeyEvent) -> ModelPickerOutcome {
        match key.code {
            KeyCode::Esc => ModelPickerOutcome::Close,
            KeyCode::Enter => match self.selected_entry() {
                Some(entry) => ModelPickerOutcome::Select(Box::new(entry.info.clone())),
                None => ModelPickerOutcome::Consumed,
            },
            KeyCode::Up => {
                self.move_up();
                ModelPickerOutcome::Consumed
            }
            KeyCode::Down => {
                self.move_down();
                ModelPickerOutcome::Consumed
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_up();
                ModelPickerOutcome::Consumed
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_down();
                ModelPickerOutcome::Consumed
            }
            _ => {
                self.nav.edit_query(key, || searchable(&self.items));
                // Everything else is swallowed: the picker is modal, so a stray key must not
                // reach the composer behind it.
                ModelPickerOutcome::Consumed
            }
        }
    }

    /// Moves the selection to `filtered_index` (a mouse click on that row); clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        self.nav.select(filtered_index);
    }

    /// Filters by pasted text, the way typing it would.
    pub fn paste(&mut self, text: &str) {
        self.nav.paste_query(text, &searchable(&self.items));
    }

    /// Swap in a freshly-built catalog without disturbing the reader.
    ///
    /// A list that arrives while someone is typing at it must not move the cursor off what they
    /// had chosen, so the row is re-found by id and the query survives. A model that has gone
    /// away takes the cursor to the top, which is the only honest answer left.
    pub fn refresh(&mut self, items: Vec<ModelEntry>) {
        let chosen = self.selected_entry().map(ModelEntry::qualified_id);
        let query = self.nav.query().to_string();
        self.items = items;
        self.nav = PickerNav::new(self.items.len());
        self.nav.set_query(&query, &searchable(&self.items));
        if let Some(chosen) = chosen {
            if let Some(row) = self
                .nav
                .filtered_indices()
                .iter()
                .position(|index| self.items[*index].qualified_id() == chosen)
            {
                self.nav.select(row);
            }
        }
    }

    /// The currently highlighted entry, if any row matches.
    pub fn selected_entry(&self) -> Option<&ModelEntry> {
        self.items.get(self.nav.selected_item_index()?)
    }

    /// The cursor's row among the visible matches.
    pub(crate) fn cursor(&self) -> usize {
        self.nav.selected()
    }

    /// Number of visible matches.
    pub fn match_count(&self) -> usize {
        self.nav.match_count()
    }

    /// The current search query.
    pub fn query(&self) -> &str {
        self.nav.query()
    }

    fn move_up(&mut self) {
        self.nav.move_up();
    }

    fn move_down(&mut self) {
        self.nav.move_down();
    }

    /// Terminal-cell width for the picker modal: fit the widest model row (cursor + active
    /// marker + label + qualified id + metadata), the footer, and the border title, then add
    /// borders + one right-pad cell. Computed over ALL items so the width is query-stable.
    ///
    /// The row's own frame is part of the fit rather than a constant beside it: a modal sized to
    /// the words alone clips the closing cap off its widest row the moment the frame changes.
    fn content_modal_width(&self, available: u16) -> u16 {
        let row_width = self
            .items
            .iter()
            .map(|entry| {
                crate::widgets::SELECTION_FRAME
                    + 2
                    + 2
                    + UnicodeWidthStr::width(entry.label.as_str())
                    + 2
                    + UnicodeWidthStr::width(entry.qualified_id().as_str())
                    + UnicodeWidthStr::width(meta_suffix(entry).as_str())
            })
            .max()
            .unwrap_or(0);
        let total = self.items.len();
        let footer_width = UnicodeWidthStr::width(
            format!(" {total}/{total} · enter switch · ↑/↓ choose · esc close ").as_str(),
        );
        let title_width = crate::widgets::panel_title_width(Icon::Model.glyph(), TITLE);
        // The lead space each note line is drawn with comes out of the same three cells the
        // borders take below, so the text's own width is the whole of what it asks for.
        let note_width =
            self.note.iter().map(|line| UnicodeWidthStr::width(line.as_str())).max().unwrap_or(0);
        let natural = row_width.max(footer_width).max(title_width).max(note_width) as u16;
        let max_width = available.saturating_sub(4).max(30);
        natural.saturating_add(3).clamp(30, max_width)
    }

    /// Rows the picker wants in `area`: its list, its chrome, and a floor so it never reads as a
    /// scrap of a list.
    pub(crate) fn desired_height(&self, area: Rect) -> u16 {
        // Borders, the search field and its rule, the rows, the footer
        let desired = self.nav.filtered_indices().len().min(14) as u16
            + 4
            + crate::widgets::FOOTER_ROWS
            + self.note_rows();
        desired.max(10 + self.note_rows()).min(area.height.saturating_sub(2))
    }

    /// Renders the picker as a centered modal over the conversation.
    pub fn view(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        if area.width < 30 || area.height < 8 {
            return;
        }
        // Fit the widest model row (stable across queries) + one right-pad cell, rather than a
        // fixed-width box with trailing dead space.
        let width = self.content_modal_width(area.width);
        let height = self.desired_height(area);
        self.view_in(frame, crate::widgets::centered_rect(width, height, area), theme, hit);
    }

    /// Renders the picker's panel at exactly `panel`, for a surface that has already placed it.
    ///
    /// The Configuration surface hands a finished login straight to this, in the region the wizard
    /// was just in — "now pick a model" is the last step of setting one up, not a new screen.
    pub(crate) fn view_in(
        &self,
        frame: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        frame.render_widget(Clear, panel);

        let block = crate::widgets::titled_panel(Icon::Model.glyph(), TITLE, theme);
        let inner = block.inner(panel);
        frame.render_widget(block, panel);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(self.note_rows()),
                Constraint::Length(2),
                Constraint::Min(2),
                Constraint::Length(crate::widgets::FOOTER_ROWS),
            ])
            .split(inner);

        if !self.note.is_empty() {
            let room = usize::from(chunks[0].width).saturating_sub(1);
            let lines: Vec<Line> = self
                .note
                .iter()
                .map(|line| {
                    Line::from(Span::styled(format!(" {}", truncate(line, room)), theme.muted))
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), chunks[0]);
        }

        let search = Paragraph::new(Line::from(vec![
            Span::styled(" Search: ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(self.query().to_string(), theme.text),
        ]))
        .block(Block::default().borders(Borders::BOTTOM).border_style(theme.border_muted));
        frame.render_widget(search, chunks[1]);
        let cursor_x = chunks[1]
            .x
            .saturating_add(9)
            .saturating_add(UnicodeWidthStr::width(self.query()) as u16)
            .min(chunks[1].right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, chunks[1].y));

        if self.nav.filtered_indices().is_empty() {
            frame
                .render_widget(Paragraph::new(" No matching models").style(theme.muted), chunks[2]);
        } else {
            self.render_rows(frame, chunks[2], theme, hit);
        }

        let count = format!(
            "{}/{}",
            self.nav.selected().saturating_add(1).min(self.nav.filtered_indices().len()),
            self.nav.filtered_indices().len()
        );
        crate::widgets::render_footer(
            frame,
            chunks[3],
            "enter switch · ↑/↓ choose · esc close",
            Some(&count),
            theme,
        );
    }

    fn render_rows(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        let capacity = area.height as usize;
        let start = self
            .nav
            .selected()
            .saturating_add(1)
            .saturating_sub(capacity)
            .min(self.nav.filtered_indices().len().saturating_sub(capacity));
        let total = self.nav.filtered_indices().len();
        let end = start.saturating_add(capacity).min(total);
        let width = crate::widgets::list_rows_width(area.width, total, area.height);
        let available = width.saturating_sub(2 + crate::widgets::SELECTION_FRAME as u16) as usize;
        {
            let mut regions = hit.borrow_mut();
            for offset in 0..end.saturating_sub(start) {
                let row_y = area.y.saturating_add(offset as u16);
                regions.push(
                    Rect::new(area.x, row_y, width, 1),
                    crate::mouse::MouseTarget::ModelRow(start + offset),
                );
            }
            let list = crate::mouse::OverlayList::Models;
            crate::widgets::list_scrollbar(frame, &mut regions, area, list, (start, total), theme);
        }

        let items: Vec<ListItem> = self.nav.filtered_indices()[start..end]
            .iter()
            .enumerate()
            .map(|(offset, index)| {
                let entry = &self.items[*index];
                let selected = start + offset == self.nav.selected();
                let is_active = self.active.as_ref() == Some(&entry.info.r#ref);
                let background = if selected { theme.selected_bg } else { Style::default() };

                let cursor = crate::glyphs::Mark::Cursor.gutter(selected);
                let marker = if is_active { "● " } else { "  " };
                let meta_text = meta_suffix(entry);

                let label = truncate(&entry.label, available.saturating_sub(marker.len() + 3));
                // The id, then what it is, in whatever the label leaves: a row that ran past the
                // frame was cut by it mid-word, and took the selection's closing cap with it
                let room = available
                    .saturating_sub(UnicodeWidthStr::width(marker))
                    .saturating_sub(UnicodeWidthStr::width(label.as_str()));
                let id = truncate(&format!("  {}", entry.qualified_id()), room);
                let meta_text =
                    truncate(&meta_text, room.saturating_sub(UnicodeWidthStr::width(id.as_str())));
                let mut spans = vec![
                    Span::styled(cursor, theme.accent_on(selected)),
                    Span::styled(
                        marker,
                        if is_active {
                            theme.success.patch(background)
                        } else {
                            theme.muted.patch(background)
                        },
                    ),
                    Span::styled(label, theme.label_on(selected)),
                    Span::styled(id, theme.muted.patch(background)),
                ];
                if !meta_text.is_empty() {
                    spans.push(Span::styled(meta_text, theme.secondary_on(selected)));
                }
                ListItem::new(crate::widgets::selection_row(
                    spans,
                    usize::from(width),
                    selected,
                    theme,
                ))
            })
            .collect();
        frame.render_widget(List::new(items), Rect { width, ..area });
    }
}

/// What a reader can type at a model row: its provider-qualified id and its display label.
fn searchable(items: &[ModelEntry]) -> Vec<String> {
    items.iter().map(|entry| format!("{} {}", entry.qualified_id(), entry.label)).collect()
}

#[cfg(test)]
mod tests;
