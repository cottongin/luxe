//! Settings rendering — the pages down the left, the controls on the right, and what the one under
//! the cursor does underneath.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Padding, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::{Row, SettingsState};
use crate::glyphs::{Icon, Mark};
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::{Depth, ThemeStyles};
use crate::widgets::{badge, selection_row, truncate_end, SELECTION_FRAME};
use luxe_core::config::Layer;
use luxe_core::settings::{Control, Page, Setting};

/// Width of the page list, its own frame included.
const NAV_WIDTH: u16 = 24;

/// Rows under the list for the one under the cursor: a rule, its name and source, its words.
const DETAIL_ROWS: u16 = 5;

/// Cells the label column takes before the value.
const LABEL_WIDTH: usize = 32;

/// Cells the value takes at the right, before the source.
const VALUE_WIDTH: usize = 16;

/// Cells the source takes at the margin: `default`, `global`, `project`.
const SOURCE_WIDTH: usize = 7;

/// Widest the panel grows.
pub(in crate::configuration) const PANEL_WIDTH: u16 = 124;

/// The keys, for the footer.
const HINT: &str = "space change · ←/→ step · r reset · e open the file · / search · tab pages · \
                    esc back";

impl SettingsState {
    /// Rows the panel would like: the whole screen, less the one-line wordmark and its gap.
    pub fn panel_height(area: Rect) -> u16 {
        area.height.saturating_sub(3).min(44)
    }

    /// The surface had no room for the panel at all, so its keys wait like a too-small panel's.
    pub(crate) fn mark_undrawn(&self) {
        self.too_small.set(true);
    }

    /// Draw the panel into exactly `panel`.
    pub fn view(
        &self,
        f: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let block = crate::widgets::titled_panel(Icon::Config.glyph(), "Settings", theme)
            .padding(Padding::horizontal(1));
        let inner = crate::widgets::render_footed(f, block, panel, HINT, None, theme);
        let too_small = inner.width < NAV_WIDTH + 30 || inner.height < 8;
        self.too_small.set(too_small);
        if too_small {
            // said, rather than an empty box whose controls still answer keys nobody can see
            let notice = vec![
                Line::from(Span::styled("Make the window bigger", theme.error)),
                Line::from(Span::styled("Only esc acts until it fits", theme.muted)),
            ];
            let at = Rect { height: inner.height.min(2), ..inner };
            let centered = Paragraph::new(notice).alignment(ratatui::layout::Alignment::Center);
            f.render_widget(centered, at);
            return;
        }

        let nav = Rect { width: NAV_WIDTH, ..inner };
        self.render_nav(f, nav, theme, hit);
        let rule_x = nav.right() + 1;
        f.render_widget(
            Paragraph::new(vec![Line::from("│"); usize::from(inner.height)])
                .style(theme.border_muted),
            Rect::new(rule_x, inner.y, 1, inner.height),
        );
        let body =
            Rect::new(rule_x + 2, inner.y, inner.right().saturating_sub(rule_x + 2), inner.height);
        self.render_body(f, body, theme, hit);
    }

    fn render_nav(
        &self,
        f: &mut Frame,
        nav: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let mut lines = vec![Line::default()];
        let mut regions = hit.borrow_mut();
        for (index, page) in Page::ALL.iter().enumerate() {
            let chosen = self.query.is_none() && index == self.page;
            let y = nav.y + lines.len() as u16;
            if y < nav.bottom() {
                regions.push(Rect::new(nav.x, y, nav.width, 1), MouseTarget::ConfigPage(index));
            }
            let spans = vec![
                Span::styled(page_icon(*page), if chosen { theme.accent } else { theme.muted }),
                Span::raw(crate::glyphs::icon_gap()),
                Span::styled(
                    page.label(),
                    if chosen { theme.accent.add_modifier(Modifier::BOLD) } else { theme.text },
                ),
                Span::styled(format!("  {}", index + 1), theme.dim),
            ];
            lines.push(selection_row(spans, usize::from(nav.width), chosen, theme));
        }
        drop(regions);
        f.render_widget(Paragraph::new(lines), nav);
    }

    fn render_body(
        &self,
        f: &mut Frame,
        body: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let width = usize::from(body.width);
        let canvas = theme.background.bg.unwrap_or(theme.slab.canvas);

        // The heading: the page, or the search, and where writes go
        let mut heading = match &self.query {
            Some(query) => {
                let mut spans = badge("SEARCH", Depth::Hero, canvas, theme);
                spans.push(Span::styled(format!(" {query}"), theme.text));
                spans.push(Span::styled(" ", Style::new().bg(theme.text.fg.unwrap_or_default())));
                spans
            }
            None => badge(page_heading(self.page()), Depth::Hero, canvas, theme),
        };
        let path = luxe_core::paths::home_relative(&self.path);
        let pad = width.saturating_sub(spans_width(&heading) + path.width());
        heading.push(Span::raw(" ".repeat(pad)));
        heading.push(Span::styled(path, theme.dim));
        f.render_widget(Paragraph::new(Line::from(heading)), Rect { height: 1, ..body });

        let list =
            Rect { y: body.y + 2, height: body.height.saturating_sub(2 + DETAIL_ROWS), ..body };
        self.render_rows(f, list, theme, hit);
        self.render_detail(
            f,
            Rect { y: list.bottom(), height: body.bottom().saturating_sub(list.bottom()), ..body },
            theme,
        );
    }

    fn render_rows(
        &self,
        f: &mut Frame,
        list: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let rows = self.rows();
        if rows.is_empty() {
            f.render_widget(Paragraph::new(Span::styled("  Nothing matches.", theme.muted)), list);
            return;
        }
        let height = usize::from(list.height).max(1);
        // Keep the cursor in view with a row of context either side where there is one
        let mut scroll = self.scroll.get().min(rows.len().saturating_sub(1));
        if self.cursor < scroll + 1 {
            scroll = self.cursor.saturating_sub(1);
        } else if self.cursor + 2 > scroll + height {
            scroll = (self.cursor + 2).saturating_sub(height);
        }
        scroll = scroll.min(rows.len().saturating_sub(height));
        self.scroll.set(scroll);

        let columns = crate::widgets::list_rows_width(list.width, rows.len(), list.height);
        let width = usize::from(columns);
        let selectable = self.selectable();
        let mut regions = hit.borrow_mut();
        let lines: Vec<Line<'static>> = rows
            .iter()
            .enumerate()
            .skip(scroll)
            .take(height)
            .map(|(index, row)| {
                let selected = index == self.cursor;
                if let Some(position) = selectable.iter().position(|at| *at == index) {
                    let y = list.y + (index - scroll) as u16;
                    regions
                        .push(Rect::new(list.x, y, columns, 1), MouseTarget::ConfigRow(position));
                }
                self.row_line(*row, width, selected, theme)
            })
            .collect();
        let shown = (scroll, rows.len());
        let surface = crate::mouse::OverlayList::Configuration;
        crate::widgets::list_scrollbar(f, &mut regions, list, surface, shown, theme);
        drop(regions);
        f.render_widget(Paragraph::new(lines), Rect { width: columns, ..list });
    }

    fn row_line(
        &self,
        row: Row,
        width: usize,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Line<'static> {
        let canvas = theme.background.bg.unwrap_or(theme.slab.canvas);
        match row {
            Row::Blank => Line::default(),
            Row::Heading(label) => {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(badge(label, Depth::Recessed, canvas, theme));
                Line::from(spans)
            }
            Row::Essentials => {
                let names = luxe_core::config::ESSENTIAL_TOOLS.join(" · ");
                let mut spans = vec![Span::raw("    "), Span::styled(names, theme.muted)];
                let tail = "always on";
                // Where a control row's value lands, its frame's inset included
                let at = value_column(width) + crate::widgets::SELECTION_INSET;
                let pad = at.saturating_sub(spans_width(&spans));
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(tail, theme.muted));
                Line::from(spans)
            }
            Row::Setting(setting) => {
                let (layer, value) = setting.current(&self.layers);
                let label = Span::styled(setting.label.to_string(), label_style(selected, theme));
                let control = self.setting_value(setting, &value, selected, theme);
                control_row(label, None, control, layer, width, selected, theme)
            }
            Row::Tool(tool) => {
                let (layer, enabled) = self.tool_state(tool);
                let label = Span::styled(tool.name.to_string(), label_style(selected, theme));
                let summary = Span::styled(tool.summary.to_string(), theme.muted);
                control_row(
                    label,
                    Some(summary),
                    toggle(enabled, theme),
                    layer,
                    width,
                    selected,
                    theme,
                )
            }
        }
    }

    fn setting_value(
        &self,
        setting: &Setting,
        value: &serde_json::Value,
        selected: bool,
        theme: &ThemeStyles,
    ) -> Vec<Span<'static>> {
        match setting.control {
            Control::Toggle { .. } => toggle(value.as_bool().unwrap_or(false), theme),
            Control::Choice { .. } | Control::StartScreen => {
                let text = value.as_str().unwrap_or_default().replace('_', " ");
                let arrows = if selected { theme.accent } else { theme.dim };
                vec![
                    Span::styled("‹ ", arrows),
                    Span::styled(text, theme.text),
                    Span::styled(" ›", arrows),
                ]
            }
            Control::Theme => vec![
                Span::styled(self.theme.clone(), theme.text),
                Span::styled(" ›", if selected { theme.accent } else { theme.dim }),
            ],
        }
    }

    /// What the row under the cursor is, what it does, and when a change is felt.
    fn render_detail(&self, f: &mut Frame, area: Rect, theme: &ThemeStyles) {
        if area.height == 0 {
            return;
        }
        let width = usize::from(area.width);
        let mut lines = vec![Line::from(Span::styled("─".repeat(width), theme.border_muted))];
        match self.rows().get(self.cursor).copied() {
            Some(Row::Setting(setting)) => {
                let (layer, _) = setting.current(&self.layers);
                lines.push(Line::from(vec![
                    Span::styled(setting.label, theme.text.add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  {}", setting.key()), theme.accent),
                    Span::styled(format!("  ·  {}", setting.applies.note()), theme.muted),
                    Span::styled(source_note(layer), layer_style(layer, theme)),
                ]));
                lines.push(Line::from(Span::styled(
                    setting.description().unwrap_or_default().to_string(),
                    theme.text,
                )));
            }
            Some(Row::Tool(tool)) => {
                let (layer, _) = self.tool_state(tool);
                let mut head = vec![
                    Span::styled(tool.name, theme.text.add_modifier(Modifier::BOLD)),
                    Span::styled("  tools.disabled", theme.accent),
                    Span::styled("  ·  Takes effect the next time luxe starts", theme.muted),
                ];
                head.push(Span::styled(source_note(layer), layer_style(layer, theme)));
                lines.push(Line::from(head));
                let mut words = tool.detail.to_string();
                if let Some(needs) = tool.needs {
                    words.push_str(&format!(" Registers only where there is {needs}."));
                }
                lines.push(Line::from(Span::styled(words, theme.text)));
            }
            _ => {}
        }
        let mut paragraph_lines = lines;
        if let Some(notice) = &self.notice {
            // The notice replaces the description's last line, so a write is acknowledged where
            // the reader is already looking
            let style = if notice.warning { theme.warning } else { theme.success };
            paragraph_lines.truncate(2);
            paragraph_lines.push(Line::from(Span::styled(notice.text.clone(), style)));
        }
        f.render_widget(Paragraph::new(paragraph_lines).wrap(Wrap { trim: true }), area);
    }
}

/// One control: label (and a summary for a tool), the value, and where the value came from.
fn control_row(
    label: Span<'static>,
    summary: Option<Span<'static>>,
    value: Vec<Span<'static>>,
    layer: Layer,
    width: usize,
    selected: bool,
    theme: &ThemeStyles,
) -> Line<'static> {
    let cursor = Span::styled(
        if selected { Mark::Cursor.glyph() } else { " " },
        theme.accent.add_modifier(Modifier::BOLD),
    );
    let at = value_column(width);
    let mut spans = vec![cursor, Span::raw(" ")];
    let label_width = match summary {
        Some(_) => 18,
        None => LABEL_WIDTH.min(at.saturating_sub(4)),
    };
    let label_text = truncate_end(&label.content, label_width.saturating_sub(1));
    let label_pad = label_width.saturating_sub(label_text.width());
    spans.push(Span::styled(label_text, label.style));
    spans.push(Span::raw(" ".repeat(label_pad)));
    if let Some(summary) = summary {
        let room = at.saturating_sub(spans_width(&spans) + 2);
        spans.push(Span::styled(truncate_end(&summary.content, room), summary.style));
    }
    let pad = at.saturating_sub(spans_width(&spans));
    spans.push(Span::raw(" ".repeat(pad)));
    let value_width = spans_width(&value);
    spans.extend(value);
    spans.push(Span::raw(" ".repeat(VALUE_WIDTH.saturating_sub(value_width) + 1)));
    let (source, style) = match layer {
        Layer::Default => ("default", theme.dim),
        Layer::Global => ("global", theme.text),
        Layer::Project => ("project", theme.warning),
    };
    spans.push(Span::styled(source, style));
    selection_row(spans, width, selected, theme)
}

/// Where the value column starts, measured inside a row's frame.
fn value_column(width: usize) -> usize {
    width.saturating_sub(SELECTION_FRAME + VALUE_WIDTH + 1 + SOURCE_WIDTH)
}

fn toggle(on: bool, theme: &ThemeStyles) -> Vec<Span<'static>> {
    match on {
        true => vec![Span::styled("● on", theme.success)],
        false => vec![Span::styled("○ off", theme.muted)],
    }
}

fn label_style(selected: bool, theme: &ThemeStyles) -> Style {
    match selected {
        true => theme.accent.add_modifier(Modifier::BOLD),
        false => theme.text,
    }
}

fn layer_style(layer: Layer, theme: &ThemeStyles) -> Style {
    match layer {
        Layer::Project => theme.warning,
        _ => theme.muted,
    }
}

/// Where the value came from, said once in the detail.
fn source_note(layer: Layer) -> &'static str {
    match layer {
        Layer::Default => "  ·  the default",
        Layer::Global => "  ·  set in your config.json",
        Layer::Project => "  ·  set by this project's .luxe/config.json",
    }
}

fn page_heading(page: Page) -> &'static str {
    match page {
        Page::Appearance => "APPEARANCE",
        Page::Conversation => "CONVERSATION",
        Page::Layout => "DOCK & STATUSLINE",
        Page::Tools => "TOOLS",
        Page::Agent => "AGENT",
    }
}

fn page_icon(page: Page) -> &'static str {
    match page {
        Page::Appearance => Icon::Thinking.glyph(),
        Page::Conversation => Icon::Conversation.glyph(),
        Page::Layout => Icon::Context.glyph(),
        Page::Tools => Icon::ToolGeneric.glyph(),
        Page::Agent => Icon::ModeBuild.glyph(),
    }
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}
