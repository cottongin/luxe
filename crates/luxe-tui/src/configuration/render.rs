//! Configuration rendering — the logotype, and one panel under it.
//!
//! The frame is the surface's contribution: it measures what is about to be shown, gives the
//! logotype whatever height is left, and centers the pair. Everything that appears in the panel —
//! this module's menu, the login wizard, the model picker — is handed the same region, so a row
//! that opens a list does not make the screen jump.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Padding, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::{ConfigFile, ConfigurationState, SettingsState, SetupState, Stage};
use crate::glyphs::Icon;
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::ThemeStyles;
use crate::widgets::{
    centered_rect, render_wordmark, selection_block, truncate_end, BannerClock, Wordmark,
    SELECTION_FRAME,
};

/// Width of the panel, matching the login wizard's so a row that opens one keeps the same frame.
const PANEL_WIDTH: u16 = 76;

/// Rows an entry occupies: what it is, and what choosing it reaches.
const ENTRY_ROWS: u16 = 2;

/// Narrowest panel worth drawing; below this the terminal is too small for the surface.
const MIN_WIDTH: u16 = 34;

/// Shortest terminal the surface draws in at all.
const MIN_HEIGHT: u16 = 10;

/// Rows of air between the logotype and the panel.
const BANNER_GAP: u16 = 2;

/// The gap a short terminal falls back to, rather than spend a whole logotype size on air.
const TIGHT_BANNER_GAP: u16 = 1;

/// Where the logotype and the panel go, for one draw.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceFrame {
    /// The logotype's row band. Zero-height when the terminal has no room for one.
    pub banner: Rect,
    /// Which drawing of the logotype fits.
    pub size: Wordmark,
    /// The panel's exact rectangle — not a region to center in.
    pub panel: Rect,
}

impl SurfaceFrame {
    /// Whether there is a logotype to draw.
    pub fn has_banner(self) -> bool {
        self.banner.height > 0
    }
}

/// Measure the surface for a panel `panel_height` rows tall.
///
/// The panel is the fixed thing here and the logotype gives way to it: a login transcript that
/// grows takes its rows from the mark above it, which is the opposite of the dashboard's rule
/// because this surface is being *used* rather than looked at.
pub fn frame(area: Rect, panel_height: u16) -> SurfaceFrame {
    frame_with_width(area, PANEL_WIDTH, panel_height)
}

/// [`frame`] for a panel `panel_width` wide, as the settings pages are.
pub fn frame_with_width(area: Rect, panel_width: u16, panel_height: u16) -> SurfaceFrame {
    let width = panel_width.min(area.width.saturating_sub(2)).max(MIN_WIDTH).min(area.width);
    let panel_height = panel_height.min(area.height);
    let room = |gap: u16| area.height.saturating_sub(panel_height + gap);
    let fit = |height: u16| match crate::glyphs::tier() == crate::glyphs::GlyphTier::Ascii {
        // The drawn logotype is block elements all the way down, so a terminal promised nothing
        // beyond ASCII gets the letter-spaced line, which is plain text.
        true => Wordmark::Line,
        false => Wordmark::fit(area.width, height),
    };
    // A row of air is worth having, but not at the price of a whole logotype size — so the gap is
    // measured both ways and only tightens when that buys a bigger drawing.
    let (size, gap) = match (fit(room(BANNER_GAP)), fit(room(TIGHT_BANNER_GAP))) {
        _ if room(TIGHT_BANNER_GAP) == 0 => (Wordmark::Line, 0),
        (roomy, tight) if roomy == tight => (roomy, BANNER_GAP),
        (_, tight) => (tight, TIGHT_BANNER_GAP),
    };
    let banner_height = match gap {
        0 => 0,
        _ => size.height().min(room(gap)),
    };

    let banner_width = if banner_height > 0 { size.width() } else { 0 };
    let block_width = width.max(banner_width).min(area.width);
    let block = centered_rect(block_width, banner_height + gap + panel_height, area);
    SurfaceFrame {
        banner: Rect::new(block.x, block.y, block_width, banner_height),
        size,
        panel: Rect::new(
            block.x + block_width.saturating_sub(width) / 2,
            block.y + banner_height + gap,
            width,
            panel_height,
        ),
    }
}

/// Draw the logotype, when the frame left room for one.
///
/// The same light the dashboard shows, on the same clock: setup is where most people meet the
/// product, and walking here from an animated dashboard should not put the light out mid-stride,
/// which reads as something broken. `banner_animation = false` stills both, in one place.
pub fn render_banner(
    f: &mut Frame,
    surface: SurfaceFrame,
    theme: &ThemeStyles,
    clock: BannerClock,
) {
    if !surface.has_banner() {
        return;
    }
    render_wordmark(f, surface.banner, theme, surface.size, clock);
}

/// Whether the terminal can hold the surface at all.
pub fn fits(area: Rect) -> bool {
    area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT
}

/// What a terminal too small for the surface says instead. Open `settings` hold their keys, since
/// their controls are not drawn either.
pub fn render_too_small(
    f: &mut Frame,
    area: Rect,
    theme: &ThemeStyles,
    settings: Option<&SettingsState>,
) {
    settings.inspect(|settings| settings.mark_undrawn());
    f.render_widget(
        Paragraph::new("Resize to continue")
            .style(theme.muted)
            .alignment(ratatui::layout::Alignment::Center),
        centered_rect(area.width.min(18), 1, area),
    );
}

impl ConfigurationState {
    /// Draw the surface: the logotype, and the menu (or the file list) under it.
    pub fn view(
        &self,
        f: &mut Frame,
        area: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
        clock: BannerClock,
    ) {
        self.note_drawn(fits(area));
        if !fits(area) {
            render_too_small(f, area, theme, self.settings());
            return;
        }
        if let Some(setup) = self.setup() {
            let surface = frame_with_width(
                area,
                super::setup::render::PANEL_WIDTH,
                SetupState::panel_height(area),
            );
            render_banner(f, surface, theme, clock);
            setup.view(f, surface.panel, theme, hit, self.configured());
            return;
        }
        if let (Stage::Settings, Some(settings)) = (self.stage, self.settings()) {
            let width = super::settings::render::PANEL_WIDTH;
            let surface = frame_with_width(area, width, SettingsState::panel_height(area));
            render_banner(f, surface, theme, clock);
            settings.view(f, surface.panel, theme, hit);
            return;
        }
        let surface = frame(area, self.panel_height());
        render_banner(f, surface, theme, clock);
        self.render_panel(f, surface.panel, theme, hit);
    }

    /// Rows the panel needs: its chrome, a row per list entry and its detail line.
    pub fn panel_height(&self) -> u16 {
        // Borders (2) + the summary and the blank under it (2) + the footer (2).
        const CHROME: u16 = 6;
        let rows = match self.stage {
            Stage::Menu | Stage::Settings | Stage::Setup => self.choices().len(),
            Stage::Files => self.files().len(),
        } as u16;
        CHROME + rows * ENTRY_ROWS
    }

    /// The panel itself: what is configured, the list, and how to drive it.
    ///
    /// Nothing here takes typed input, so no cursor position is set — and a frame that sets none
    /// leaves the cursor hidden, which is what a menu wants. The wizard parks a block in its corner
    /// instead, because it owns a text field and something has to claim the cursor every frame.
    fn render_panel(
        &self,
        f: &mut Frame,
        panel: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let block = crate::widgets::titled_panel(Icon::Config.glyph(), self.title(), theme)
            .padding(Padding::horizontal(1));
        let inner = crate::widgets::render_footed(f, block, panel, self.hint(), None, theme);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        f.render_widget(
            Paragraph::new(self.summary_line(theme)).wrap(Wrap { trim: true }),
            chunks[0],
        );

        let rows: Vec<(String, String)> = match self.stage {
            Stage::Menu | Stage::Settings | Stage::Setup => self
                .choices()
                .iter()
                .map(|choice| (choice.label().to_string(), choice.detail().to_string()))
                .collect(),
            Stage::Files => self
                .files()
                .iter()
                .map(|file| (file.name.to_string(), file.detail.to_string()))
                .collect(),
        };
        let selected = self.selected_row();
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut regions = hit.borrow_mut();
        for (index, (label, detail)) in rows.iter().enumerate() {
            let row_y = chunks[1].y.saturating_add(lines.len() as u16);
            if row_y < chunks[1].bottom() {
                regions.push(
                    Rect::new(
                        chunks[1].x,
                        row_y,
                        chunks[1].width,
                        ENTRY_ROWS.min(chunks[1].bottom() - row_y),
                    ),
                    MouseTarget::ConfigRow(index),
                );
            }
            // One slab for the whole entry rather than one per line: a label and what it reaches
            // are a single choice, and two rows capped at the same column read as two of them.
            lines.extend(selection_block(
                vec![
                    menu_row(label, index + 1, index == selected, chunks[1].width, theme),
                    detail_row(detail, chunks[1].width, theme),
                ],
                usize::from(chunks[1].width),
                index == selected,
                theme,
            ));
        }
        drop(regions);
        f.render_widget(Paragraph::new(lines), chunks[1]);
    }

    /// Panel title — the file list is a step inside the surface, and says so.
    fn title(&self) -> &'static str {
        match self.stage {
            Stage::Menu | Stage::Settings | Stage::Setup => "Configuration",
            Stage::Files => "Configuration files",
        }
    }

    /// The bottom row: the keys that drive whichever list is showing.
    fn hint(&self) -> &'static str {
        match self.stage {
            Stage::Menu | Stage::Settings | Stage::Setup => "enter open · ↑/↓ choose · esc back",
            Stage::Files => "enter open in your editor · ↑/↓ choose · esc back",
        }
    }

    /// The line above the list: what is configured, or where the files are.
    fn summary_line(&self, theme: &ThemeStyles) -> Line<'static> {
        match self.stage {
            // Nothing configured is the one state worth a color: it is the reason this surface
            // opened, and every row under it is an answer to it.
            Stage::Menu | Stage::Settings | Stage::Setup => match self.is_configured() {
                true => Line::from(Span::styled(self.summary(), theme.muted)),
                false => Line::from(Span::styled(self.summary(), theme.warning)),
            },
            Stage::Files => Line::from(Span::styled(files_summary(self.files()), theme.muted)),
        }
    }
}

/// Where the files are, when they share a directory — the row labels are bare names, so something
/// has to say which directory they are names in.
fn files_summary(files: &[ConfigFile]) -> String {
    let mut directories = files.iter().filter_map(|file| file.path.parent());
    let shared = directories.next().filter(|first| directories.all(|next| next == *first));
    match shared {
        Some(directory) => {
            format!("Opens in your editor · {}", luxe_core::paths::home_relative(directory))
        }
        None => "Opens in your editor".to_string(),
    }
}

/// Cells an entry's line may fill: the panel, less the slab's own frame.
fn room(width: u16) -> usize {
    usize::from(width).saturating_sub(SELECTION_FRAME)
}

/// An entry's first line: the cursor, the label, and the number that runs it.
fn menu_row(
    label: &str,
    number: usize,
    selected: bool,
    width: u16,
    theme: &ThemeStyles,
) -> Vec<Span<'static>> {
    let shortcut = format!("[{number}]");
    // The shortcut is the part that has to survive a narrow panel — it is how the row is run — so
    // the label gives way to it, keeping a cell of gap between the two.
    let label = truncate_end(label, room(width).saturating_sub(2 + shortcut.width() + 1));
    let used = 2 + UnicodeWidthStr::width(label.as_str()) + shortcut.width();
    let gap = room(width).saturating_sub(used).max(1);
    vec![
        Span::styled(
            crate::glyphs::Mark::Cursor.gutter(selected),
            theme.accent.add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, theme.label_on(selected)),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            shortcut,
            if selected { theme.text.add_modifier(Modifier::BOLD) } else { theme.muted },
        ),
    ]
}

/// The line under a label: what choosing it actually reaches.
///
/// Clipped rather than wrapped. The panel is sized to two rows an entry, so a third line here would
/// push every entry below it out through the bottom border.
fn detail_row(detail: &str, width: u16, theme: &ThemeStyles) -> Vec<Span<'static>> {
    // Under the label, which the cursor gutter alone sets in.
    const INDENT: usize = 2;
    vec![
        Span::raw(" ".repeat(INDENT)),
        Span::styled(truncate_end(detail, room(width).saturating_sub(INDENT)), theme.muted),
    ]
}

#[cfg(test)]
mod tests;
