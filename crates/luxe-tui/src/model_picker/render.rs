//! Model picker drawing — the panel, its search line, the rows of whichever step is showing, and
//! the footer that says what `tab` and `←` do from here.

use super::meta::meta_suffix;
use super::providers::ProviderRow;
use super::{ModelEntry, ModelPickerState};
use crate::glyphs::{icon_label, Icon};
use crate::theme::styles::ThemeStyles;
use crate::widgets::truncate_end as truncate;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Panel label for the model step, and the start of its title when scoped to a provider.
const MODEL_TITLE: &str = "Select model";

/// Panel label for the provider step.
const PROVIDER_TITLE: &str = "Select provider";

impl ModelPickerState {
    /// The panel's glyph and title: the step showing, and which provider a scoped list is from.
    fn title(&self) -> (&'static str, String) {
        if self.providers.is_some() {
            return (Icon::Provider.glyph(), PROVIDER_TITLE.to_owned());
        }
        match self.scoped_provider() {
            Some(provider) => (Icon::Model.glyph(), format!("{MODEL_TITLE} \u{b7} {provider}")),
            None => (Icon::Model.glyph(), MODEL_TITLE.to_owned()),
        }
    }

    /// The keys along the bottom edge, for the step showing.
    fn hint(&self) -> String {
        if self.providers.is_some() {
            return "enter choose \u{b7} \u{2191}/\u{2193} move \u{b7} esc close".to_owned();
        }
        let widen = match (self.served_scope(), self.widened) {
            (Some(provider), true) => format!("tab only {provider} \u{b7} "),
            (Some(_), false) => "tab every provider \u{b7} ".to_owned(),
            (None, _) => String::new(),
        };
        let esc = if self.from_providers { "esc back" } else { "esc close" };
        format!("enter switch \u{b7} {widen}\u{2190} providers \u{b7} {esc}")
    }

    /// What a model row says after its label: who serves it while the list mixes providers, and
    /// its id when a configured name stands in for it.
    fn subtitle(&self, entry: &ModelEntry) -> String {
        let id = &entry.info.r#ref.id;
        let named = entry.label != *id;
        match (self.scoped_provider().is_none(), named) {
            (true, true) => format!("  {}", entry.qualified_id()),
            (true, false) => format!("  {}", entry.info.r#ref.provider),
            (false, true) => format!("  {id}"),
            (false, false) => String::new(),
        }
    }

    /// Terminal-cell width for the picker modal: fit the widest row of either step, the footer,
    /// the title and the note, then add borders and one right-pad cell. Measured over every row so
    /// the width holds still while a query narrows the list or the reader changes step.
    ///
    /// The row's own frame is part of the fit rather than a constant beside it: a modal sized to
    /// the words alone clips the closing cap off its widest row the moment the frame changes.
    pub(super) fn content_modal_width(&self, available: u16) -> u16 {
        let chrome = crate::widgets::SELECTION_FRAME + 2 + 2;
        let model_rows = self.all.iter().map(|entry| {
            let words = entry.label.width() + entry.qualified_id().width() + 2;
            chrome + words + meta_suffix(entry).width()
        });
        let provider_rows = self.provider_step_rows().into_iter().map(|row| chrome + row.width());
        let row_width = model_rows.chain(provider_rows).max().unwrap_or(0);
        let total = self.all.len();
        let footer_width = format!(" {total}/{total} \u{b7} {} ", self.hint()).width();
        let (glyph, title) = self.title();
        let title_width = crate::widgets::panel_title_width(glyph, &title);
        // The lead space each note line is drawn with comes out of the same three cells the
        // borders take below, so the text's own width is the whole of what it asks for.
        let note_width = self.note.iter().map(|line| line.width()).max().unwrap_or(0);
        let natural = row_width.max(footer_width).max(title_width).max(note_width) as u16;
        let max_width = available.saturating_sub(4).max(30);
        natural.saturating_add(3).clamp(30, max_width)
    }

    /// Every provider row as drawn, for measuring; the step need not be showing.
    fn provider_step_rows(&self) -> Vec<String> {
        let step = super::providers::ProviderStep::new(&self.all, None);
        step.rows.iter().map(provider_text).collect()
    }

    /// Rows the picker wants in `area`: its list, its chrome, and a floor so it never reads as a
    /// scrap of a list.
    pub(crate) fn desired_height(&self, area: Rect) -> u16 {
        // Borders, the search field and its rule, the rows, the footer
        let desired =
            self.match_count().min(14) as u16 + 4 + crate::widgets::FOOTER_ROWS + self.note_rows();
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
        // Fit the widest row (stable across queries) + one right-pad cell, rather than a
        // fixed-width box with trailing dead space.
        let width = self.content_modal_width(area.width);
        let height = self.desired_height(area);
        self.view_in(frame, crate::widgets::centered_rect(width, height, area), theme, hit);
    }

    /// Renders the picker's panel at exactly `panel`, for a surface that has already placed it.
    ///
    /// The Configuration surface hands a finished login straight to this, in the region the wizard
    /// was just in: "now pick a model" is the last step of setting one up, not a new screen.
    pub(crate) fn view_in(
        &self,
        frame: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        frame.render_widget(Clear, panel);
        let (glyph, title) = self.title();
        let block = crate::widgets::titled_panel(glyph, &title, theme);
        let chunks = self.sections(block.inner(panel));
        frame.render_widget(block, panel);
        self.render_note(frame, chunks[0], theme);
        self.render_search(frame, chunks[1], theme);
        self.render_list(frame, chunks[2], theme, hit);
        let count = self.position_label();
        crate::widgets::render_footer(frame, chunks[3], &self.hint(), Some(&count), theme);
    }

    /// The panel's inside, top to bottom: the note, the search line, the list and the footer.
    fn sections(&self, inner: Rect) -> std::rc::Rc<[Rect]> {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(self.note_rows()),
                Constraint::Length(2),
                Constraint::Min(2),
                Constraint::Length(crate::widgets::FOOTER_ROWS),
            ])
            .split(inner)
    }

    /// `3/12`: the cursor's row among the matches, for the footer.
    fn position_label(&self) -> String {
        let total = self.match_count();
        format!("{}/{total}", self.cursor().saturating_add(1).min(total))
    }

    /// What a login landed, above the list, when there is something to say.
    fn render_note(&self, frame: &mut Frame, area: Rect, theme: &ThemeStyles) {
        if self.note.is_empty() {
            return;
        }
        let room = usize::from(area.width).saturating_sub(1);
        let lines: Vec<Line> = self
            .note
            .iter()
            .map(|line| Line::from(Span::styled(format!(" {}", truncate(line, room)), theme.muted)))
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// The search line, with the terminal's cursor at the end of the query.
    fn render_search(&self, frame: &mut Frame, area: Rect, theme: &ThemeStyles) {
        let search = Paragraph::new(Line::from(vec![
            Span::styled(" Search: ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(self.query().to_string(), theme.text),
        ]))
        .block(Block::default().borders(Borders::BOTTOM).border_style(theme.border_muted));
        frame.render_widget(search, area);
        let cursor_x = area
            .x
            .saturating_add(9)
            .saturating_add(self.query().width() as u16)
            .min(area.right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, area.y));
    }

    /// The rows of the step showing, or a line saying nothing matches.
    fn render_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        if self.match_count() == 0 {
            return self.render_empty(frame, area, theme);
        }
        let window = self.window(area.height as usize);
        let width = crate::widgets::list_rows_width(area.width, self.match_count(), area.height);
        self.register_rows(area, width, window, theme, frame, hit);
        let available = width.saturating_sub(2 + crate::widgets::SELECTION_FRAME as u16) as usize;
        let items: Vec<ListItem> = (window.0..window.1)
            .map(|row| {
                let selected = row == self.cursor();
                let spans = self.row_spans(row, selected, available, theme);
                let line = crate::widgets::selection_row(spans, width.into(), selected, theme);
                ListItem::new(line)
            })
            .collect();
        frame.render_widget(List::new(items), Rect { width, ..area });
    }

    /// The line in place of the list when the query matches nothing.
    fn render_empty(&self, frame: &mut Frame, area: Rect, theme: &ThemeStyles) {
        let empty = if self.providers.is_some() { "providers" } else { "models" };
        let message = Paragraph::new(format!(" No matching {empty}")).style(theme.muted);
        frame.render_widget(message, area);
    }

    /// Row `row` of the step showing, as spans.
    fn row_spans(
        &self,
        row: usize,
        selected: bool,
        available: usize,
        theme: &ThemeStyles,
    ) -> Vec<Span<'static>> {
        match &self.providers {
            Some(step) => self.provider_spans(step, row, selected, available, theme),
            None => self.model_spans(row, selected, available, theme),
        }
    }

    /// The `(first, end)` rows of the filtered list that fit `capacity`, around the cursor.
    fn window(&self, capacity: usize) -> (usize, usize) {
        let total = self.match_count();
        let start =
            (self.cursor() + 1).saturating_sub(capacity).min(total.saturating_sub(capacity));
        (start, (start + capacity).min(total))
    }

    /// Publishes a click target per visible row, and the scrollbar beside them.
    fn register_rows(
        &self,
        area: Rect,
        width: u16,
        (start, end): (usize, usize),
        theme: &ThemeStyles,
        frame: &mut Frame,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        let mut regions = hit.borrow_mut();
        for row in start..end {
            let row_rect = Rect::new(area.x, area.y.saturating_add((row - start) as u16), width, 1);
            regions.push(row_rect, crate::mouse::MouseTarget::ModelRow(row));
        }
        let (list, position) = (crate::mouse::OverlayList::Models, (start, self.match_count()));
        crate::widgets::list_scrollbar(frame, &mut regions, area, list, position, theme);
    }

    /// One model row: cursor, the in-use dot, label, who serves it or its id, and what it is.
    fn model_spans(
        &self,
        row: usize,
        selected: bool,
        available: usize,
        theme: &ThemeStyles,
    ) -> Vec<Span<'static>> {
        let Some(entry) = self.nav.item_index_at(row).and_then(|index| self.items.get(index))
        else {
            return Vec::new();
        };
        let is_active = self.active.as_ref() == Some(&entry.info.r#ref);
        let label = truncate(&entry.label, available.saturating_sub(MARKER_WIDTH + 3));
        // The subtitle, then what it is, in whatever the label leaves: a row that ran past the
        // frame was cut by it mid-word, and took the selection's closing cap with it
        let room = available.saturating_sub(MARKER_WIDTH).saturating_sub(label.width());
        let subtitle = truncate(&self.subtitle(entry), room);
        let meta = truncate(&meta_suffix(entry), room.saturating_sub(subtitle.width()));
        let mut spans = lead_spans(selected, is_active, theme);
        spans.push(Span::styled(label, theme.label_on(selected)));
        spans.push(Span::styled(subtitle, theme.muted.patch(row_background(selected, theme))));
        if !meta.is_empty() {
            spans.push(Span::styled(meta, theme.secondary_on(selected)));
        }
        spans
    }

    /// One provider row: cursor, the in-use dot, its mark and id, and how many models it serves.
    fn provider_spans(
        &self,
        step: &super::providers::ProviderStep,
        row: usize,
        selected: bool,
        available: usize,
        theme: &ThemeStyles,
    ) -> Vec<Span<'static>> {
        let Some(provider) = step.nav.item_index_at(row).and_then(|index| step.rows.get(index))
        else {
            return Vec::new();
        };
        let is_active = self.active.as_ref().is_some_and(|active| active.provider == provider.id);
        let name = icon_label(crate::widgets::provider_icon(&provider.id).glyph(), &provider.id);
        let name = truncate(&name, available.saturating_sub(MARKER_WIDTH));
        let count = truncate(&model_count(provider), available.saturating_sub(name.width() + 2));
        let mut spans = lead_spans(selected, is_active, theme);
        spans.push(Span::styled(name, theme.label_on(selected)));
        spans.push(Span::styled(count, theme.muted.patch(row_background(selected, theme))));
        spans
    }
}

/// Cells the in-use dot takes, with its gap, whether or not it is drawn.
const MARKER_WIDTH: usize = 2;

/// What every row starts with: the cursor's gutter, then the in-use dot or its blank.
fn lead_spans(selected: bool, is_active: bool, theme: &ThemeStyles) -> Vec<Span<'static>> {
    let marker = if is_active { "\u{25cf} " } else { "  " };
    let marker_style = active_style(is_active, theme).patch(row_background(selected, theme));
    vec![
        Span::styled(crate::glyphs::Mark::Cursor.gutter(selected), theme.accent_on(selected)),
        Span::styled(marker, marker_style),
    ]
}

/// The cursor row's background, for spans that would otherwise leave a gap in it.
fn row_background(selected: bool, theme: &ThemeStyles) -> Style {
    if selected {
        theme.selected_bg
    } else {
        Style::default()
    }
}

/// The in-use dot's ink: success on the row in use, muted (and blank) elsewhere.
fn active_style(is_active: bool, theme: &ThemeStyles) -> Style {
    if is_active {
        theme.success
    } else {
        theme.muted
    }
}

/// `  12 models`, after a provider's name.
fn model_count(provider: &ProviderRow) -> String {
    match provider.models {
        1 => "  1 model".to_owned(),
        count => format!("  {count} models"),
    }
}

/// A provider row's words as drawn, for measuring the panel.
fn provider_text(provider: &ProviderRow) -> String {
    let name = icon_label(crate::widgets::provider_icon(&provider.id).glyph(), &provider.id);
    format!("  {name}{}", model_count(provider))
}
