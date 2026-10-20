//! Session picker — searchable resume flow for the start screen and `/sessions`.
//!
//! The CLI prepares a bounded catalog, at startup for the start screen and on demand mid-session;
//! this module only filters, renders, and returns the selected session ID. Disk access and resume
//! logic stay outside the TUI.

use crate::dashboard::{PickerKind, PreviewRole, PreviewTurn, SessionSummary};
use crate::glyphs::Icon;
use crate::picker::PickerNav;
use crate::theme::styles::ThemeStyles;
use crate::widgets::{centered_rect, truncate_end as truncate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// A searchable session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// Durable session identifier
    pub id: String,
    /// Human title derived from the first user message
    pub name: String,
    /// Relative modification time
    pub age: String,
    /// Bounded transcript text used for grep-style search
    pub search_text: String,
    /// Role-tagged transcript excerpt rendered (with markdown) in the preview pane
    pub preview: Vec<PreviewTurn>,
}

impl From<&SessionSummary> for SessionEntry {
    fn from(summary: &SessionSummary) -> Self {
        Self {
            id: summary.id.clone(),
            name: summary.name.clone(),
            age: summary.age.clone(),
            search_text: summary.search_text.clone(),
            preview: summary.preview.clone(),
        }
    }
}

/// What a key did in the session picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPickerKey {
    /// Enter accepted this session
    Selected(String),
    /// The picker used the key, or swallowed it so the composer behind it never sees it
    Used,
    /// A chord the picker has no use for, left for the app's global bindings
    Ignored,
}

/// Pure state for the find/recent/grep session picker.
#[derive(Debug)]
pub struct SessionPickerState {
    kind: PickerKind,
    sessions: Vec<SessionEntry>,
    nav: PickerNav,
    /// The read of the catalog it is waiting for, while it waits
    reading: Option<u64>,
}

impl SessionPickerState {
    /// Creates a find-session picker.
    pub fn new(sessions: Vec<SessionEntry>) -> Self {
        Self::for_kind(sessions, PickerKind::FindSession)
    }

    /// Creates the picker variant requested by a dashboard action.
    pub fn for_kind(sessions: Vec<SessionEntry>, kind: PickerKind) -> Self {
        Self { kind, nav: PickerNav::new(sessions.len()), sessions, reading: None }
    }

    /// Creates a find-session picker for a catalog that is still being read. Args: `(request)`,
    /// the read it waits for.
    ///
    /// It opens at once and takes a search while it waits, so the reader is never held on a
    /// frame that is walking session logs.
    pub(crate) fn reading(request: u64) -> Self {
        Self { reading: Some(request), ..Self::new(Vec::new()) }
    }

    /// Whether the catalog it lists is still being read.
    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Whether `request` is the read it is waiting for.
    pub(crate) fn waits_for(&self, request: u64) -> bool {
        self.reading == Some(request)
    }

    /// Lists `sessions`, the catalog a [`Self::reading`] picker was waiting for, keeping whatever
    /// was typed meanwhile.
    pub(crate) fn fill(&mut self, sessions: Vec<SessionEntry>) {
        let query = self.query().to_string();
        *self = Self::for_kind(sessions, self.kind.clone());
        if !query.is_empty() {
            self.set_query(&query);
        }
    }

    /// Returns the current search query.
    pub fn query(&self) -> &str {
        self.nav.query()
    }

    /// The cursor's row among the visible matches.
    pub(crate) fn cursor(&self) -> usize {
        self.nav.selected()
    }

    /// Returns the number of visible matches.
    pub fn match_count(&self) -> usize {
        self.nav.match_count()
    }

    /// Replaces the search query and recomputes ranked matches.
    pub fn set_query(&mut self, query: &str) {
        let haystacks = searchable(&self.sessions);
        self.nav.set_query(query, &haystacks);
    }

    /// Filters by pasted text, the way typing it would.
    pub fn paste(&mut self, text: &str) {
        let haystacks = searchable(&self.sessions);
        self.nav.paste_query(text, &haystacks);
    }

    /// Moves the selection up, wrapping from the first result to the last.
    pub fn move_up(&mut self) {
        self.nav.move_up();
    }

    /// Moves the selection down, wrapping from the last result to the first.
    pub fn move_down(&mut self) {
        self.nav.move_down();
    }

    /// Moves the highlight to filtered-list row `index` (mouse hover or click), clamped.
    pub fn select(&mut self, index: usize) {
        self.nav.select(index);
    }

    /// The session id at filtered-list row `index` (a mouse click on that row).
    pub fn session_id_at(&self, index: usize) -> Option<String> {
        let entry = self.nav.item_index_at(index)?;
        self.sessions.get(entry).map(|session| session.id.clone())
    }

    /// The session under the cursor.
    pub fn selected_session(&self) -> Option<&SessionEntry> {
        self.sessions.get(self.nav.selected_item_index()?)
    }

    /// Handles picker-local input: the session Enter accepts, a key the picker used, or one it
    /// left for the app (a chord, which may be a global binding).
    pub fn on_key(&mut self, event: &KeyEvent) -> SessionPickerKey {
        match event.code {
            KeyCode::Up if event.modifiers.is_empty() => self.nav.move_up(),
            KeyCode::Down if event.modifiers.is_empty() => self.nav.move_down(),
            KeyCode::Enter if event.modifiers.is_empty() => {
                return self.selected_session().map_or(SessionPickerKey::Used, |session| {
                    SessionPickerKey::Selected(session.id.clone())
                });
            }
            _ => {
                let edited = self.nav.edit_query(event, || searchable(&self.sessions));
                let chord = event.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                if !edited && chord {
                    return SessionPickerKey::Ignored;
                }
            }
        }
        SessionPickerKey::Used
    }

    /// Renders a centered modal over the dashboard.
    pub fn view(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        if area.width < 28 || area.height < 8 {
            frame.render_widget(
                Paragraph::new("Resize to browse sessions").style(theme.muted),
                area,
            );
            return;
        }

        // Side-by-side list + preview once the terminal is wide enough for the dock (same
        // breakpoint); otherwise a taller stacked layout that spends free vertical space on a
        // richer preview pane.
        let wide =
            area.width >= crate::workspace_layout::WIDE_LAYOUT_MIN_WIDTH && area.height >= 16;
        let width = if wide {
            area.width.saturating_sub(6).clamp(120, 180)
        } else {
            area.width.saturating_sub(4).clamp(28, 96)
        };
        let height = area.height.saturating_sub(2).clamp(if wide { 16 } else { 10 }, 40);
        let modal = centered_rect(width, height, area);
        frame.render_widget(Clear, modal);

        let block = crate::widgets::titled_panel(Icon::Session.glyph(), self.title(), theme);
        let inner = block.inner(modal);
        frame.render_widget(block, modal);

        // Footer pins to the bottom across both layouts; the body fills the rest.
        let shell = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(crate::widgets::FOOTER_ROWS)])
            .split(inner);
        if wide {
            self.render_wide_body(frame, shell[0], theme, hit);
        } else {
            self.render_stacked_body(frame, shell[0], theme, hit);
        }
        crate::widgets::render_footer(
            frame,
            shell[1],
            "enter resume · ctrl+u clear · ↑/↓ choose · esc back",
            None,
            theme,
        );
    }

    /// Renders the search field + row-count line into `area`, placing the text cursor after the
    /// query. Shared by both layouts.
    fn render_search_header(&self, frame: &mut Frame, area: Rect, theme: &ThemeStyles) {
        let header = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area);
        let query_line = Line::from(vec![
            Span::styled(" Search: ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(self.query().to_string(), theme.text),
        ]);
        frame.render_widget(Paragraph::new(query_line), header[0]);
        let cursor_x = header[0]
            .x
            .saturating_add(9)
            .saturating_add(UnicodeWidthStr::width(self.query()) as u16)
            .min(header[0].right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, header[0].y));
        let count = if self.is_reading() {
            String::new()
        } else if self.query().is_empty() {
            format!(" {} sessions", self.nav.filtered_indices().len())
        } else {
            format!(" {} matches", self.nav.filtered_indices().len())
        };
        frame.render_widget(Paragraph::new(count).style(theme.muted), header[1]);
    }

    /// Narrow layout: search/count, the results list sized to its content, then a multi-line
    /// preview pane (separated by a thin rule) that fills the remaining vertical space.
    fn render_stacked_body(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        // The list takes what it needs (up to ~half the body) so the preview keeps a rich share.
        let body = area.height.saturating_sub(2); // search + count
        let preview_floor = 5u16;
        let divider = 1u16;
        let max_list = body.saturating_sub(preview_floor + divider).max(1);
        let list_height = (self.nav.filtered_indices().len() as u16).clamp(1, max_list);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(list_height),
                Constraint::Length(divider),
                Constraint::Min(preview_floor),
            ])
            .split(area);
        self.render_search_header(frame, chunks[0], theme);
        self.render_results(frame, chunks[1], theme, hit);
        // A minimal horizontal rule separates the list from the preview.
        frame.render_widget(
            Block::default().borders(Borders::TOP).border_style(theme.border_muted),
            chunks[2],
        );
        self.render_preview_pane(frame, chunks[3], theme);
    }

    /// Wide layout: results list on the left, a full-height preview pane on the right, divided by
    /// a single vertical rule (matching the dock/main-content separator).
    fn render_wide_body(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        let list_width = (area.width / 2).clamp(40, area.width.saturating_sub(32));
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(list_width),
                Constraint::Length(1),
                Constraint::Min(20),
            ])
            .split(area);
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(columns[0]);
        self.render_search_header(frame, left[0], theme);
        self.render_results(frame, left[1], theme, hit);
        // Vertical divider between the list and the preview.
        frame.render_widget(
            Block::default().borders(Borders::LEFT).border_style(theme.border_muted),
            columns[1],
        );
        // Inset the preview one cell off the divider for breathing room.
        let preview = Rect {
            x: columns[2].x.saturating_add(1),
            width: columns[2].width.saturating_sub(1),
            ..columns[2]
        };
        self.render_preview_pane(frame, preview, theme);
    }

    fn render_results(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    ) {
        if self.nav.filtered_indices().is_empty() {
            let said = match (self.is_reading(), self.query().is_empty()) {
                (true, _) => " Reading sessions…",
                (false, true) => " No sessions to open yet",
                (false, false) => " No sessions match this search",
            };
            frame.render_widget(Paragraph::new(said).style(theme.muted), area);
            return;
        }

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
        // Two cells for the cursor column, and the frame the selected row is cut with.
        let available_width =
            width.saturating_sub(4 + crate::widgets::SELECTION_FRAME as u16) as usize;
        {
            let mut regions = hit.borrow_mut();
            for offset in 0..end.saturating_sub(start) {
                let row_y = area.y.saturating_add(offset as u16);
                regions.push(
                    Rect::new(area.x, row_y, width, 1),
                    crate::mouse::MouseTarget::SessionRow(start + offset),
                );
            }
            let list = crate::mouse::OverlayList::Sessions;
            crate::widgets::list_scrollbar(frame, &mut regions, area, list, (start, total), theme);
        }

        let items: Vec<ListItem> = self.nav.filtered_indices()[start..end]
            .iter()
            .enumerate()
            .map(|(offset, index)| {
                let session = &self.sessions[*index];
                let selected = start + offset == self.nav.selected();
                let age_width = UnicodeWidthStr::width(session.age.as_str());
                let title_width = available_width.saturating_sub(age_width + 2).max(4);
                let title = truncate(&session.name, title_width);
                let gap = " ".repeat(
                    available_width
                        .saturating_sub(UnicodeWidthStr::width(title.as_str()) + age_width)
                        .max(1),
                );
                let background = if selected { theme.selected_bg } else { Style::default() };
                let cursor = crate::glyphs::Mark::Cursor.gutter(selected);
                ListItem::new(crate::widgets::selection_row(
                    vec![
                        Span::styled(cursor, theme.accent_on(selected)),
                        Span::styled(title, theme.label_on(selected)),
                        Span::styled(gap, background),
                        Span::styled(session.age.clone(), theme.muted.patch(background)),
                    ],
                    usize::from(width),
                    selected,
                    theme,
                ))
            })
            .collect();
        frame.render_widget(List::new(items), Rect { width, ..area });
    }

    /// Renders a multi-line preview of the selected session: a heading, the session title + age,
    /// then the role-tagged transcript excerpt (markdown-rendered) filling the available height.
    /// Used by both layouts.
    fn render_preview_pane(&self, frame: &mut Frame, area: Rect, theme: &ThemeStyles) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let mut lines =
            vec![Line::from(Span::styled("Preview", theme.accent.add_modifier(Modifier::BOLD)))];
        if let Some(session) = self.selected_session() {
            lines.push(Line::from(Span::styled(session.name.clone(), theme.text)));
            lines.push(Line::from(Span::styled(session.age.clone(), theme.dim)));
            lines.push(Line::default());
            if session.preview.is_empty() {
                // No structured transcript captured — fall back to the flattened search text.
                let transcript =
                    session.search_text.split_whitespace().collect::<Vec<_>>().join(" ");
                if transcript.is_empty() {
                    lines.push(Line::from(Span::styled("No message preview", theme.muted)));
                } else {
                    lines.push(Line::from(Span::styled(transcript, theme.muted)));
                }
            } else {
                append_preview_turns(&mut lines, &session.preview, theme);
            }
        } else {
            lines.push(Line::from(Span::styled("No session selected", theme.muted)));
        }
        // `trim: false` keeps markdown indentation (list bullets, code) intact when wrapping.
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }

    fn title(&self) -> &'static str {
        match self.kind {
            PickerKind::FindSession => "Find session",
            PickerKind::RecentSessions => "Recent sessions",
            PickerKind::GrepSessions => "Grep sessions",
        }
    }
}

/// Renders the preview transcript into `lines`: each turn gets a role header (a colored bar +
/// label matching the conversation's role coloring) followed by its markdown-rendered body, with
/// a blank line between turns. Tool turns render as a single glyphed one-liner.
fn append_preview_turns(
    lines: &mut Vec<Line<'static>>,
    turns: &[PreviewTurn],
    theme: &ThemeStyles,
) {
    for (index, turn) in turns.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        match turn.role {
            PreviewRole::User | PreviewRole::Assistant => {
                let (label, color) = match turn.role {
                    PreviewRole::User => ("You", theme.accent),
                    _ => ("Assistant", theme.success),
                };
                lines.push(Line::from(vec![
                    Span::styled("▌ ", color),
                    Span::styled(label, color.add_modifier(Modifier::BOLD)),
                ]));
                lines.extend(crate::markdown::render_markdown(&turn.text, theme));
            }
            PreviewRole::Tool => {
                lines.push(Line::from(vec![
                    Span::styled(Icon::ToolGeneric.prefix(), theme.warning),
                    Span::styled(turn.text.clone(), theme.warning),
                ]));
            }
        }
    }
}

/// What a reader can type at a session row: its title plus the searchable body text.
fn searchable(sessions: &[SessionEntry]) -> Vec<String> {
    sessions.iter().map(|session| format!("{} {}", session.name, session.search_text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn fresh() -> SessionPickerState {
        SessionPickerState::new(vec![
            SessionEntry {
                id: "a".into(),
                name: "Fix palette navigation".into(),
                age: "2h ago".into(),
                search_text: "arrow keys command search".into(),
                preview: vec![
                    PreviewTurn {
                        role: PreviewRole::User,
                        text: "arrow keys command search".into(),
                    },
                    PreviewTurn {
                        role: PreviewRole::Assistant,
                        text: "Done — **rebound** the keys.".into(),
                    },
                ],
            },
            SessionEntry {
                id: "b".into(),
                name: "Refine dashboard".into(),
                age: "1d ago".into(),
                search_text: "session picker modal".into(),
                preview: Vec::new(),
            },
            SessionEntry {
                id: "c".into(),
                name: "Provider work".into(),
                age: "2d ago".into(),
                search_text: "stream completion usage".into(),
                preview: Vec::new(),
            },
        ])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn filter_searches_titles_and_transcript_text() {
        let mut picker = fresh();
        picker.set_query("modal");
        assert_eq!(picker.match_count(), 1);
        assert_eq!(picker.selected_session().map(|s| s.id.as_str()), Some("b"));
    }

    #[test]
    fn empty_query_shows_all() {
        let mut picker = fresh();
        picker.set_query("dashboard");
        picker.set_query("");
        assert_eq!(picker.match_count(), 3);
    }

    #[test]
    fn arrow_navigation_moves_both_directions() {
        let mut picker = fresh();
        picker.on_key(&key(KeyCode::Down));
        assert_eq!(picker.selected_session().map(|s| s.id.as_str()), Some("b"));
        picker.on_key(&key(KeyCode::Up));
        assert_eq!(picker.selected_session().map(|s| s.id.as_str()), Some("a"));
        picker.on_key(&key(KeyCode::Up));
        assert_eq!(picker.selected_session().map(|s| s.id.as_str()), Some("c"));
    }

    #[test]
    fn printable_j_and_k_filter_instead_of_navigating() {
        let mut picker = fresh();
        picker.on_key(&key(KeyCode::Char('j')));
        picker.on_key(&key(KeyCode::Char('k')));
        assert_eq!(picker.query(), "jk");
    }

    #[test]
    fn enter_returns_selected_session() {
        let mut picker = fresh();
        assert_eq!(picker.on_key(&key(KeyCode::Enter)), SessionPickerKey::Selected("a".into()));
    }

    #[test]
    fn backspace_updates_query() {
        let mut picker = fresh();
        picker.on_key(&key(KeyCode::Char('p')));
        picker.on_key(&key(KeyCode::Backspace));
        assert_eq!(picker.query(), "");
        assert_eq!(picker.match_count(), 3);
    }

    fn dark_theme() -> ThemeStyles {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("dark").expect("dark theme resolves");
        ThemeStyles::from_resolved(&resolved, "dark")
    }

    fn render_at(width: u16, height: u16) -> String {
        use ratatui::{backend::TestBackend, Terminal};
        let picker = fresh();
        let theme = dark_theme();
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal
            .draw(|frame| picker.view(frame, frame.area(), &theme, &hit))
            .expect("render session picker");
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn narrow_layout_renders_a_multiline_preview_pane() {
        // Below the side-by-side breakpoint: the preview stacks under the list and shows the
        // selected session's transcript, filling the vertical space rather than one line.
        let text = render_at(90, 24);
        assert!(text.contains("Preview"), "preview heading present");
        assert!(text.contains("Fix palette navigation"), "selected session title in preview");
        assert!(text.contains("arrow keys command search"), "transcript excerpt in preview");
    }

    #[test]
    fn preview_renders_role_headers_and_markdown() {
        // The structured preview shows a role header per turn and renders markdown rather than
        // dumping raw syntax: the assistant turn's `**rebound**` becomes bold `rebound` with no
        // literal asterisks in the buffer.
        let text = render_at(90, 24);
        assert!(text.contains("You"), "user role header present");
        assert!(text.contains("Assistant"), "assistant role header present");
        assert!(text.contains("rebound"), "assistant markdown body rendered");
        assert!(!text.contains("**"), "markdown emphasis syntax must not render literally");
    }

    #[test]
    fn wide_layout_places_the_preview_beside_the_list() {
        // At/above the side-by-side breakpoint the preview shares a row with the list, divided by
        // a vertical rule; both the list and the preview render on the same top rows.
        let width = crate::workspace_layout::WIDE_LAYOUT_MIN_WIDTH + 20;
        let text = render_at(width, 30);
        assert!(text.contains("Preview"), "preview heading present");
        assert!(text.contains("arrow keys command search"), "transcript excerpt in preview");
        assert!(text.contains('│'), "a vertical divider separates list and preview");
    }
}
