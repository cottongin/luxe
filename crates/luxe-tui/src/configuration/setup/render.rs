//! Setup rendering — where the steps stand, then the one showing.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Padding, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::{sign_in_rows, SetupMode, SetupState, StartScreen, Step};
use crate::glyphs::{Icon, Mark};
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::{Depth, ThemeStyles};
use crate::widgets::{badge, selection_block, selection_row, slab, truncate_end, SELECTION_FRAME};

/// Widest the panel grows.
pub(in crate::configuration) const PANEL_WIDTH: u16 = 112;

/// Width of the theme list and the tool list, beside what each explains.
const LIST_WIDTH: u16 = 34;

impl SetupState {
    /// Rows the panel would like, which leaves the wordmark what is left.
    pub fn panel_height(area: Rect) -> u16 {
        area.height.saturating_sub(9).clamp(18, 32).min(area.height)
    }

    /// Draw the panel into exactly `panel`. `configured` names what can answer a turn now.
    pub fn view(
        &self,
        f: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
        configured: &[String],
    ) {
        let title = match self.mode {
            SetupMode::FirstRun => "Welcome to Luxe",
            SetupMode::Init => "Set up Luxe",
        };
        let block = crate::widgets::titled_panel(Icon::Config.glyph(), title, theme)
            .padding(Padding::horizontal(1));
        let note = luxe_core::paths::home_relative(&luxe_core::config::Config::global_path());
        let inner = crate::widgets::render_footed(f, block, panel, self.hint(), Some(&note), theme);
        if inner.height < 6 {
            return;
        }
        f.render_widget(Paragraph::new(self.stepper(theme)), Rect { height: 1, ..inner });
        let body = Rect { y: inner.y + 2, height: inner.height - 2, ..inner };
        match self.step {
            Step::Theme => self.render_themes(f, body, theme, hit),
            Step::Provider => self.render_provider(f, body, theme, hit, configured),
            Step::Tools => self.render_tools(f, body, theme, hit),
            Step::Start => self.render_start(f, body, theme, hit),
        }
    }

    /// `✓ Theme ── 2 Provider ── 3 Tools ── 4 Start`, each step cut into a slab: the ones behind
    /// on the raised step, the one here on the accent, the rest on the idle surface.
    fn stepper(&self, theme: &ThemeStyles) -> Line<'static> {
        let canvas = theme.background.bg.unwrap_or(theme.slab.canvas);
        let mut spans = Vec::new();
        for (index, step) in Step::ALL.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" ── ", theme.border_muted));
            }
            let here = self.step.index();
            let (surface, style, mark) = match index.cmp(&here) {
                std::cmp::Ordering::Less => {
                    (theme.slab.at(Depth::Raised), theme.text, Mark::Yes.glyph().to_string())
                }
                std::cmp::Ordering::Equal => (
                    theme.slab.at(Depth::Hero),
                    Style::new().fg(theme.slab.ink).add_modifier(Modifier::BOLD),
                    (index + 1).to_string(),
                ),
                std::cmp::Ordering::Greater => {
                    (theme.slab.idle[0], theme.muted, (index + 1).to_string())
                }
            };
            spans.extend(slab(
                [Span::styled(format!("{mark} {}", step.label()), style)],
                surface,
                canvas,
                Style::new(),
            ));
        }
        Line::from(spans)
    }

    fn hint(&self) -> &'static str {
        match self.step {
            Step::Theme => "↑/↓ preview · enter use this one · s skip setup",
            Step::Provider => "enter open · ↑/↓ choose · ← back · s skip setup",
            Step::Tools => "space turn on or off · enter continue · ← back · s skip setup",
            Step::Start => "enter finish · ↑/↓ choose · ← back · s skip setup",
        }
    }

    fn render_themes(
        &self,
        f: &mut Frame,
        body: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let list = Rect { width: LIST_WIDTH.min(body.width), ..body };
        let height = usize::from(list.height);
        let start =
            (self.theme + 1).saturating_sub(height).min(self.themes.len().saturating_sub(height));
        let mut regions = hit.borrow_mut();
        let lines: Vec<Line<'static>> = self
            .themes
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(index, offer)| {
                regions.push(
                    Rect::new(list.x, list.y + (index - start) as u16, list.width, 1),
                    MouseTarget::ConfigRow(index),
                );
                let chosen = index == self.theme;
                let label = match offer {
                    None => {
                        format!("Follow the desktop ({})", self.desktop.clone().unwrap_or_default())
                    }
                    Some(name) if name == "luxe" => "luxe  (Luxe's own)".to_string(),
                    Some(name) => name.clone(),
                };
                let spans = vec![
                    Span::styled(
                        Mark::Cursor.gutter(chosen),
                        theme.accent.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(label, theme.label_on(chosen)),
                ];
                selection_row(spans, usize::from(list.width), chosen, theme)
            })
            .collect();
        drop(regions);
        f.render_widget(Paragraph::new(lines), list);

        let sample = Rect {
            x: list.right() + 3,
            width: body.right().saturating_sub(list.right() + 3),
            ..body
        };
        if sample.width > 20 {
            render_sample(f, sample, theme);
        }
    }

    fn render_provider(
        &self,
        f: &mut Frame,
        body: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
        configured: &[String],
    ) {
        let summary = match configured.is_empty() {
            true => Line::from(Span::styled(
                "Nothing can answer a turn yet. Choose how you reach a model, or skip and set one \
                 up later in /config.",
                theme.warning,
            )),
            false => Line::from(vec![
                Span::styled("Ready: ", theme.success),
                Span::styled(configured.join(" · "), theme.text),
                Span::styled("  Add another, or continue.", theme.muted),
            ]),
        };
        f.render_widget(
            Paragraph::new(summary).wrap(Wrap { trim: true }),
            Rect { height: 2, ..body },
        );
        let list = Rect { y: body.y + 3, height: body.height.saturating_sub(3), ..body };
        let width = usize::from(list.width);
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut regions = hit.borrow_mut();
        let rows: Vec<(String, String)> = sign_in_rows()
            .iter()
            .map(|choice| (choice.label().to_string(), choice.detail().to_string()))
            .chain(std::iter::once(match configured.is_empty() {
                true => (
                    "Skip for now".to_string(),
                    "Set a provider up any time in /config".to_string(),
                ),
                false => ("Continue".to_string(), "On to the tools the agent can use".to_string()),
            }))
            .collect();
        for (index, (label, detail)) in rows.into_iter().enumerate() {
            let chosen = index == self.provider;
            let y = list.y + lines.len() as u16;
            if y < list.bottom() {
                regions.push(
                    Rect::new(list.x, y, list.width, 2.min(list.bottom() - y)),
                    MouseTarget::ConfigRow(index),
                );
            }
            lines.extend(selection_block(
                vec![
                    vec![
                        Span::styled(
                            Mark::Cursor.gutter(chosen),
                            theme.accent.add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(label, theme.label_on(chosen)),
                    ],
                    vec![
                        Span::raw("  "),
                        Span::styled(
                            truncate_end(&detail, width.saturating_sub(SELECTION_FRAME + 2)),
                            theme.muted,
                        ),
                    ],
                ],
                width,
                chosen,
                theme,
            ));
        }
        drop(regions);
        f.render_widget(Paragraph::new(lines), list);
    }

    fn render_tools(
        &self,
        f: &mut Frame,
        body: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let list = Rect { width: (LIST_WIDTH + 18).min(body.width), ..body };
        let width = usize::from(list.width);
        let height = usize::from(list.height);
        let start =
            (self.tool + 1).saturating_sub(height).min(self.tools.len().saturating_sub(height));
        let mut regions = hit.borrow_mut();
        let lines: Vec<Line<'static>> = self
            .tools
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(index, (tool, enabled))| {
                regions.push(
                    Rect::new(list.x, list.y + (index - start) as u16, list.width, 1),
                    MouseTarget::ConfigRow(index),
                );
                let chosen = index == self.tool;
                let mut spans = vec![
                    Span::styled(
                        Mark::Cursor.gutter(chosen),
                        theme.accent.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{:<17}", tool.name), theme.label_on(chosen)),
                ];
                let toggle = match enabled {
                    true => Span::styled("● on", theme.success),
                    false => Span::styled("○ off", theme.muted),
                };
                let used: usize = spans.iter().map(|span| span.content.width()).sum();
                let pad = width.saturating_sub(SELECTION_FRAME + used + toggle.content.width());
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(toggle);
                selection_row(spans, width, chosen, theme)
            })
            .collect();
        drop(regions);
        f.render_widget(Paragraph::new(lines), list);

        let side = Rect {
            x: list.right() + 3,
            width: body.right().saturating_sub(list.right() + 3),
            ..body
        };
        let Some((tool, _)) = self.tools.get(self.tool) else { return };
        if side.width < 16 {
            return;
        }
        let mut words = vec![
            Line::from(vec![
                Span::styled(tool.name, theme.text.add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", tool.summary), theme.muted),
            ]),
            Line::default(),
            Line::from(Span::styled(tool.detail, theme.text)),
        ];
        if let Some(needs) = tool.needs {
            words.push(Line::default());
            words.push(Line::from(Span::styled(
                format!("Registers only where there is {needs}."),
                theme.muted,
            )));
        }
        words.push(Line::default());
        words.push(Line::from(vec![
            Span::styled("read, write, edit and bash", theme.text),
            Span::styled(" are always on: the agent cannot work without them.", theme.muted),
        ]));
        words.push(Line::default());
        words.push(Line::from(vec![
            Span::styled("Change these later in ", theme.muted),
            Span::styled("/settings", theme.accent),
            Span::styled(", on the Tools page.", theme.muted),
        ]));
        f.render_widget(Paragraph::new(words).wrap(Wrap { trim: true }), side);
    }

    fn render_start(
        &self,
        f: &mut Frame,
        body: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "What Luxe opens on when you run it in a project.",
                theme.muted,
            ))),
            Rect { height: 1, ..body },
        );
        let list = Rect { y: body.y + 2, height: body.height.saturating_sub(2), ..body };
        let width = usize::from(list.width);
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut regions = hit.borrow_mut();
        for (index, screen) in StartScreen::ALL.iter().enumerate() {
            let chosen = index == self.start;
            let y = list.y + lines.len() as u16;
            if y < list.bottom() {
                regions.push(
                    Rect::new(list.x, y, list.width, 2.min(list.bottom() - y)),
                    MouseTarget::ConfigRow(index),
                );
            }
            let radio = match chosen {
                true => Span::styled("● ", theme.accent),
                false => Span::styled("○ ", theme.muted),
            };
            lines.extend(selection_block(
                vec![
                    vec![
                        Span::styled(
                            Mark::Cursor.gutter(chosen),
                            theme.accent.add_modifier(Modifier::BOLD),
                        ),
                        radio,
                        Span::styled(screen.label(), theme.label_on(chosen)),
                    ],
                    vec![Span::raw("    "), Span::styled(screen.detail(), theme.muted)],
                ],
                width,
                chosen,
                theme,
            ));
            lines.push(Line::default());
        }
        drop(regions);
        f.render_widget(Paragraph::new(lines), list);
    }
}

/// A sample of the product in whatever theme is showing: a message, a reply, an edit with its diff,
/// and the slabs the dock is built from — the things a palette has to get right.
fn render_sample(f: &mut Frame, area: Rect, theme: &ThemeStyles) {
    let width = usize::from(area.width);
    let canvas = theme.background.bg.unwrap_or(theme.slab.canvas);
    let filled = |spans: Vec<Span<'static>>, bg: ratatui::style::Color| {
        let mut spans: Vec<Span<'static>> = spans
            .into_iter()
            .map(|span| {
                let style = if span.style.bg.is_some() { span.style } else { span.style.bg(bg) };
                Span::styled(span.content, style)
            })
            .collect();
        let used: usize = spans.iter().map(|span| span.content.width()).sum();
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), Style::new().bg(bg)));
        Line::from(spans)
    };
    let user = theme.user_msg_bg.bg.unwrap_or_else(|| theme.slab.at(Depth::Recessed));
    let card = theme.tool_call_success_bg.bg.unwrap_or_else(|| theme.slab.at(Depth::Recessed));
    let add = theme.diff_add_bg.bg.unwrap_or(card);
    let del = theme.diff_del_bg.bg.unwrap_or(card);
    let mut heads = badge("PLAN", Depth::Hero, canvas, theme);
    heads.push(Span::raw(" "));
    heads.extend(badge("CHANGES", Depth::Raised, canvas, theme));
    heads.push(Span::raw(" "));
    heads.extend(badge("MODEL", Depth::Recessed, canvas, theme));
    let lines = vec![
        filled(
            vec![
                Span::styled("▌ ", theme.accent),
                Span::styled("Rename the dock's headings to match the rail", theme.text),
            ],
            user,
        ),
        Line::default(),
        Line::from(Span::styled(
            "Two headings drift from the rail's names; renaming both.",
            theme.text,
        )),
        Line::default(),
        filled(
            vec![
                Span::styled(" ± edit", theme.success.add_modifier(Modifier::BOLD)),
                Span::styled("  src/view/dock.rs", theme.text),
                Span::styled("  6ms", theme.muted),
            ],
            card,
        ),
        filled(
            vec![
                Span::styled("   734 ", theme.muted),
                Span::styled("fn ", theme.accent),
                Span::styled("heading(section: Section) {", theme.text),
            ],
            card,
        ),
        filled(
            vec![
                Span::styled("  -735 ", theme.error),
                Span::styled("    let label = \"FILES\";", theme.text),
            ],
            del,
        ),
        filled(
            vec![
                Span::styled("  +735 ", theme.success),
                Span::styled("    let label = \"CHANGES\";", theme.text),
            ],
            add,
        ),
        filled(vec![Span::styled("   736 ", theme.muted), Span::styled("}", theme.text)], card),
        Line::default(),
        Line::from(heads),
        Line::default(),
        Line::from(vec![
            Span::styled("info ", theme.info),
            Span::styled("warning ", theme.warning),
            Span::styled("error ", theme.error),
            Span::styled("success", theme.success),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}
