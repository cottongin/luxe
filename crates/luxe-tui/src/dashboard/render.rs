//! Dashboard rendering — the wordmark, the prompt's frame, and the list joined under it.
//!
//! The prompt itself is the session's composer, drawn by the view the same way a session draws it;
//! this module measures where it goes, draws everything around it, and then cuts the composer's
//! bottom edge into the divider that makes the prompt and the list one box.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::{selectable_in, DashboardState, Entry};
use crate::glyphs::{Icon, Mark};
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::{Depth, ThemeStyles};
use crate::widgets::{
    badge, key_hint_row, render_wordmark, selection_row, truncate_end, BannerClock, Wordmark,
    PANEL_BORDER_TYPE, SELECTION_FRAME,
};

/// Widest the prompt grows: a readable measure, not the terminal's width.
const MAX_WIDTH: u16 = 96;

/// Rows the list's frame spends besides its rows: a rule, the keys, the bottom edge — the top edge
/// is the composer's bottom, recut as a divider.
const LIST_CHROME: u16 = 3;

/// The composer's height with one line in it, its frame included: what the layout is measured
/// against, so a prompt that wraps grows downward rather than pushing everything up.
const SINGLE_LINE_INPUT: u16 = 3;

/// Rows of air between the wordmark and the prompt, and the fallback for a short terminal.
const GAP: u16 = 2;
const TIGHT_GAP: u16 = 1;

/// A row's icon, its icon's style, its label and what trails it.
type RowParts = (&'static str, Style, Vec<Span<'static>>, Vec<Span<'static>>);

impl DashboardState {
    /// How wide the prompt is drawn in `area`.
    pub fn input_width(area: Rect) -> u16 {
        match area.width {
            width if width >= MAX_WIDTH + 8 => MAX_WIDTH,
            width => width.saturating_sub(4).max(20).min(width),
        }
    }

    /// Draw the wordmark, the list and the footer, and return where the composer goes.
    ///
    /// `input_rows` is the composer's height, its frame included, as the session measures it.
    /// `border` is the composer's border style, which the list's frame continues.
    #[allow(clippy::too_many_arguments)]
    pub fn view(
        &self,
        f: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
        clock: BannerClock,
        input_rows: u16,
        border: Style,
    ) -> Rect {
        let width = Self::input_width(area);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let entries = if self.list_shown() { self.entries() } else { Vec::new() };
        let list_rows = entries.len() as u16;
        let chrome = if self.list_shown() { LIST_CHROME } else { 0 };
        let note_rows = if self.footer_note.is_some() { 2 } else { 0 };
        let gap = if area.height >= 30 { GAP } else { TIGHT_GAP };

        // The layout is measured against the empty screen, not against what the draft matched, so
        // the prompt holds still while you type: results grow downward from it, and a draft that
        // folds the list away leaves the prompt where it was. A prompt that wraps grows downward
        // into the same room, for the same reason
        let one_line = input_rows.min(SINGLE_LINE_INPUT);
        let anchor = match self.style {
            luxe_core::config::DashboardStyle::Minimal => 0,
            _ => self.menu().len() as u16 + LIST_CHROME,
        };
        let need = |size: Wordmark, rows: u16| size.height() + gap + one_line + rows + note_rows;
        let sizes: &[Wordmark] = match crate::glyphs::tier() == crate::glyphs::GlyphTier::Ascii {
            true => &[Wordmark::Line],
            false => &[Wordmark::Full, Wordmark::Small, Wordmark::Line],
        };
        let fits =
            |size: &&Wordmark| size.width() <= area.width && need(**size, anchor) <= area.height;
        let (mark, gap) = match sizes.iter().find(fits) {
            Some(size) => (Some(*size), gap),
            None if need(Wordmark::Line, anchor.min(LIST_CHROME + 1)) <= area.height => {
                (Some(Wordmark::Line), gap)
            }
            None => (None, 0),
        };
        let mark_rows = mark.map_or(0, Wordmark::height);
        let total = (mark_rows + gap + one_line + anchor + note_rows).min(area.height);
        let top = area.y + area.height.saturating_sub(total) / 2;
        if let Some(size) = mark {
            render_wordmark(f, Rect { y: top, height: size.height(), ..area }, theme, size, clock);
        }
        let composer_y = top + mark_rows + gap;
        let composer = Rect::new(
            x,
            composer_y,
            width,
            input_rows.min(area.bottom().saturating_sub(composer_y)),
        );
        // Whatever is below the prompt, to the bottom of the screen, less the frame and the note
        let room = area.bottom().saturating_sub(composer.bottom() + chrome + note_rows);
        let shown = list_rows.min(room);

        let mut bottom = composer.bottom();
        if self.list_shown() {
            let frame =
                Rect::new(x, composer.bottom().saturating_sub(1), width, shown + chrome + 1);
            f.render_widget(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_type(PANEL_BORDER_TYPE)
                    .border_style(border),
                frame,
            );
            let inner = Rect::new(x + 1, composer.bottom(), width.saturating_sub(2), shown);
            self.render_list(f, inner, &entries, theme, hit);
            let rule = Rect { y: inner.bottom(), height: 1, ..inner };
            f.render_widget(
                Paragraph::new(Span::styled(
                    "─".repeat(usize::from(inner.width)),
                    theme.border_muted,
                )),
                rule,
            );
            f.render_widget(
                Paragraph::new(key_hint_row(&self.hint(&entries), theme)),
                Rect { y: rule.y + 1, ..rule },
            );
            bottom = frame.bottom();
        }
        if let Some(note) = &self.footer_note {
            f.render_widget(
                Paragraph::new(Span::styled(note.clone(), theme.muted))
                    .alignment(ratatui::layout::Alignment::Center),
                Rect { y: bottom + 1, height: 1, ..area },
            );
        }
        composer
    }

    /// Once the composer is drawn: cut its bottom edge into the divider over the list, or, with the
    /// list folded, say on that edge how to open it.
    pub fn finish(&self, f: &mut Frame, composer: Rect, theme: &ThemeStyles, border: Style) {
        if composer.width < 2 || composer.height == 0 {
            return;
        }
        // A flyout under the prompt is its own panel, and the edge it hangs from stays plain
        if self.flyout_open() {
            return;
        }
        let y = composer.bottom() - 1;
        let buffer = f.buffer_mut();
        if self.list_shown() {
            buffer[(composer.x, y)].set_symbol("├").set_style(border);
            buffer[(composer.right() - 1, y)].set_symbol("┤").set_style(border);
            for x in composer.x + 1..composer.right() - 1 {
                buffer[(x, y)].set_symbol("─").set_style(theme.border_muted);
            }
            return;
        }
        // Folded for a prompt, the edge says what Enter does; folded by the minimal style, what is
        // under it
        let label = match self.drafting_a_prompt() {
            true => Line::from(vec![
                Span::styled(" enter ", theme.accent),
                Span::styled("ask in a new session · ", theme.muted),
                Span::styled("esc ", theme.accent),
                Span::styled("clear ", theme.muted),
            ]),
            false => Line::from(vec![
                Span::styled(" ↓ ", theme.accent),
                Span::styled("sessions and commands ", theme.muted),
            ]),
        };
        let cells = label.width() as u16;
        if cells + 4 < composer.width {
            let at = Rect::new(composer.right() - cells - 2, y, cells, 1);
            f.render_widget(Paragraph::new(label), at);
        }
    }

    fn render_list(
        &self,
        f: &mut Frame,
        area: Rect,
        entries: &[Entry],
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let height = usize::from(area.height);
        if height == 0 {
            return;
        }
        // Keep the cursor on screen, and a heading with its first row when there is room
        let mut scroll = self.scroll.get().min(entries.len().saturating_sub(height));
        if self.selected < scroll {
            scroll = self.selected.saturating_sub(2);
        } else if self.selected >= scroll + height {
            scroll = self.selected + 1 - height;
        }
        self.scroll.set(scroll);

        let width = usize::from(area.width);
        let selectable = selectable_in(entries);
        let mut regions = hit.borrow_mut();
        let lines: Vec<Line<'static>> = entries
            .iter()
            .enumerate()
            .skip(scroll)
            .take(height)
            .map(|(index, entry)| {
                if let Some(position) = selectable.iter().position(|row| *row == index) {
                    let y = area.y + (index - scroll) as u16;
                    regions.push(
                        Rect::new(area.x, y, area.width, 1),
                        MouseTarget::DashboardItem(position),
                    );
                }
                self.entry_line(entry, width, index == self.selected, theme)
            })
            .collect();
        drop(regions);
        f.render_widget(Paragraph::new(lines), area);
    }

    fn entry_line(
        &self,
        entry: &Entry,
        width: usize,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Line<'static> {
        let parts = match entry {
            Entry::Heading(label, note) => {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(badge(label, Depth::Recessed, theme.slab.canvas, theme));
                if let Some(note) = note {
                    spans.push(Span::styled(format!(" {note}"), theme.muted));
                }
                return Line::from(spans);
            }
            Entry::Blank => return Line::default(),
            Entry::Recent(_) | Entry::Session(..) => {
                self.session_entry_parts(entry, selected, theme)
            }
            Entry::Slash(_) | Entry::Grep | Entry::ProjectContext | Entry::Command(..) => {
                self.search_entry_parts(entry, selected, theme)
            }
            _ => self.menu_entry_parts(entry, selected, theme),
        };
        let Some((icon, icon_style, label, trailing)) = parts else {
            return Line::default();
        };
        row(icon, icon_style, label, trailing, width, selected, theme)
    }

    /// A menu row's parts: what it opens, and the key or command beside it.
    fn menu_entry_parts(
        &self,
        entry: &Entry,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Option<RowParts> {
        let quiet = |text: &str| vec![Span::styled(text.to_string(), theme.muted)];
        let named = |text: &str| Span::styled(text.to_string(), label_style(selected, theme));
        Some(match entry {
            Entry::SetUp => (
                Icon::Config.glyph(),
                theme.muted,
                vec![
                    named("Set up a provider"),
                    Span::styled("  nothing is configured yet", theme.warning),
                ],
                quiet("/config"),
            ),
            Entry::Resume => {
                let title =
                    self.resume.as_ref().map(|resume| resume.title.clone()).unwrap_or_default();
                let age = self.resume.as_ref().map(|resume| resume.age.clone()).unwrap_or_default();
                (
                    Icon::Session.glyph(),
                    theme.muted,
                    vec![named("Resume"), Span::raw("  "), Span::styled(title, theme.text)],
                    quiet(&age),
                )
            }
            Entry::NewSession => {
                (Icon::NewSession.glyph(), theme.muted, vec![named("New session")], quiet("/new"))
            }
            Entry::Sessions => {
                (Icon::Find.glyph(), theme.muted, vec![named("Sessions…")], quiet("/sessions"))
            }
            Entry::Review => (
                Icon::Changes.glyph(),
                theme.muted,
                vec![
                    named("Review changes"),
                    Span::styled(format!("  {} files  ", self.briefing.changed_files), theme.muted),
                    Span::styled(format!("+{}", self.briefing.added), theme.success),
                    Span::styled(format!(" −{}", self.briefing.removed), theme.error),
                ],
                quiet("/diff"),
            ),
            Entry::Configuration => {
                (Icon::Config.glyph(), theme.muted, vec![named("Configuration")], quiet("/config"))
            }
            Entry::Quit => (Icon::Quit.glyph(), theme.muted, vec![named("Quit")], quiet("ctrl+c")),
            Entry::Ask => (
                Icon::Steering.glyph(),
                theme.muted,
                vec![Span::styled("Ask in a new session", label_style(true, theme))],
                quiet("enter"),
            ),
            _ => return None,
        })
    }

    /// A session row's parts, recent or matched, with its age; `None` once the session is gone.
    fn session_entry_parts(
        &self,
        entry: &Entry,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Option<RowParts> {
        let quiet = |text: &str| vec![Span::styled(text.to_string(), theme.muted)];
        Some(match entry {
            Entry::Recent(index) => {
                let session = self.recent.get(*index)?;
                (
                    Icon::SessionEntry.glyph(),
                    theme.accent_secondary,
                    vec![Span::styled(session.name.clone(), label_style(selected, theme))],
                    quiet(&session.age),
                )
            }
            Entry::Session(index, positions) => {
                let session = self.sessions.get(*index)?;
                (
                    Icon::SessionEntry.glyph(),
                    theme.accent_secondary,
                    lit(&session.name, positions, label_style(selected, theme), theme),
                    quiet(&session.age),
                )
            }
            _ => return None,
        })
    }

    /// A row a draft matched: a slash command, a search inside sessions, or a command.
    fn search_entry_parts(
        &self,
        entry: &Entry,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Option<RowParts> {
        let quiet = |text: &str| vec![Span::styled(text.to_string(), theme.muted)];
        let named = |text: &str| Span::styled(text.to_string(), label_style(selected, theme));
        Some(match entry {
            Entry::Slash(command) => {
                let mut label = vec![named(&format!("Run {}", command.slash_name().unwrap_or("")))];
                if let Some(args) = &command.args {
                    label.push(Span::styled(format!(" {args}"), theme.text));
                }
                label.push(Span::styled(format!("  {}", command.name), theme.muted));
                (Icon::Palette.glyph(), theme.muted, label, quiet("enter"))
            }
            Entry::Grep => (
                Icon::Grep.glyph(),
                theme.muted,
                vec![
                    named("Search inside every session for "),
                    Span::styled(
                        format!("“{}”", truncate_end(self.query.trim(), 32)),
                        theme.accent,
                    ),
                ],
                Vec::new(),
            ),
            Entry::ProjectContext => (
                Icon::ContextFile.glyph(),
                theme.muted,
                vec![
                    named("Project context"),
                    Span::styled("  instruction files, skills and MCP servers", theme.muted),
                ],
                Vec::new(),
            ),
            Entry::Command(index, positions) => {
                let command = self.command(*index)?;
                let mut label = lit(&command.name, positions, label_style(selected, theme), theme);
                label.push(Span::styled(format!("  {}", command.summary), theme.muted));
                (
                    Icon::Palette.glyph(),
                    theme.muted,
                    label,
                    quiet(command.slash_name().unwrap_or("")),
                )
            }
            _ => return None,
        })
    }

    /// The keys, for whatever the cursor is on, and the mode switch, naming the mode it switches
    /// to. What typing does is the prompt's placeholder to say.
    fn hint(&self, entries: &[Entry]) -> String {
        let verb = match entries.get(self.selected) {
            Some(Entry::Ask) => "ask",
            Some(Entry::SetUp) => "set up",
            Some(Entry::Slash(_) | Entry::Command(..)) => "run",
            Some(Entry::Session(..) | Entry::Recent(_) | Entry::Resume) => "resume",
            Some(Entry::Quit) => "quit",
            _ => "open",
        };
        let mut keys = vec![format!("enter {verb}"), "↑/↓ choose".to_string()];
        if let Some((chord, next)) = &self.mode_switch {
            keys.push(format!("{chord} {next} mode"));
        }
        keys.push(match self.query.trim().is_empty() {
            true => "ctrl+c quit".to_string(),
            false => "esc clear".to_string(),
        });
        keys.join(" · ")
    }
}

/// One row: cursor, icon, label, and one quiet fact at the margin.
///
/// The label is cut rather than the fact, which is the part that lines up down the list.
fn row(
    icon: &'static str,
    icon_style: Style,
    label: Vec<Span<'static>>,
    trailing: Vec<Span<'static>>,
    width: usize,
    selected: bool,
    theme: &ThemeStyles,
) -> Line<'static> {
    let room = width.saturating_sub(SELECTION_FRAME);
    let cursor = if selected { Mark::Cursor.glyph() } else { " " };
    let mut spans = vec![
        Span::styled(cursor, theme.accent.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(icon, if selected { theme.accent } else { icon_style }),
        Span::raw(crate::glyphs::icon_gap()),
    ];
    let trailing_width: usize = trailing.iter().map(|span| span.content.width()).sum();
    let fixed: usize =
        spans.iter().map(|span| span.content.width()).sum::<usize>() + trailing_width + 2;
    let mut used = 0;
    for span in label {
        let left = room.saturating_sub(fixed + used);
        if left == 0 {
            break;
        }
        let text = truncate_end(&span.content, left);
        used += text.width();
        spans.push(Span::styled(text, span.style));
    }
    let drawn: usize = spans.iter().map(|span| span.content.width()).sum();
    spans.push(Span::raw(" ".repeat(room.saturating_sub(drawn + trailing_width))));
    spans.extend(trailing);
    selection_row(spans, width, selected, theme)
}

/// `text` with the characters at `positions` lit in the accent.
fn lit(text: &str, positions: &[usize], base: Style, theme: &ThemeStyles) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let highlight = theme.accent.add_modifier(Modifier::BOLD);
    for (index, character) in text.chars().enumerate() {
        let style = if positions.contains(&index) { highlight } else { base };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(character),
            _ => spans.push(Span::styled(character.to_string(), style)),
        }
    }
    spans
}

fn label_style(selected: bool, theme: &ThemeStyles) -> Style {
    match selected {
        true => theme.accent.add_modifier(Modifier::BOLD),
        false => theme.text,
    }
}
