//! Statusline — the runtime metadata bar, cut into detached slabs.
//!
//! A single row: workspace identity on the left, model and context telemetry on the right. Each
//! cluster's slabs are one continuous band, colored off one accent receding into the canvas: the
//! project on the left and the model on the right wear it undiluted and sit flush against the
//! screen's edges, each module steps down from the one outside it, and the band fades into the bar
//! toward the middle ([`crate::widgets::slab`] has the fade). Read end to end the bar is one ramp,
//! brightest at the two facts worth anchoring — where you are, and what is answering.
//!
//! Two kinds of module ride bare on the bar instead. The long, variable strings, because a
//! checklist item in a pill is a block rather than a badge; and the context reading, because its
//! *color* is its datum and a surface behind it competes with the green it fills up through.
//! `statusline.style = "flat"` draws every module that way.
//!
//! What the agent is doing *right now* is deliberately not here: the composer's own footer owns the
//! live turn, and the runtime measures every part of it. This bar carries where you are and how far
//! the plan has got, which is a slower fact and survives a glance.

mod activity;
mod layout;
mod segments;

use crate::app::App;
use crate::theme::styles::{Depth, SlabShades, ThemeStyles};
use crate::workspace_dock::DockPresentation;
pub(crate) use activity::{activity_parts, format_elapsed_compact, thinking_label};
use layout::{compose_line, fit_segments, lay, Shape, Side};
use luxe_core::{
    config::{StatuslineModule, StatuslineStyle},
    provider::ThinkingLevel,
};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};
use segments::{left_segments, right_segments};
use unicode_width::UnicodeWidthStr;
#[cfg(test)]
use {crate::glyphs::Icon, ratatui::style::Modifier, segments::abbreviate_path};

const MAX_MODEL_WIDTH: usize = 28;
const MAX_CWD_WIDTH: usize = 30;
const MAX_PROJECT_WIDTH: usize = 18;
const MAX_TODO_WIDTH: usize = 34;

/// Blank cells between adjacent segments when the bar is flat.
const SEGMENT_GAP: usize = 2;

/// Blank cells between a band's fade and whatever rides bare beside it.
///
/// One: the fade's last step is already most of the way to the canvas, so a wider gap reads as the
/// bare text having nothing to do with the band it trails.
const SLAB_GAP: usize = 1;

/// Where you are, on what branch, how far through the plan, and how much has changed.
///
/// `cwd` is absent because the project name now answers "where am I" in a third of the cells, and a
/// path was the longest thing on the bar. `lsp` is absent because the bar rarely has width to spare
/// for it — language-server state is a question you ask (`/lsp`) rather than one you watch.
/// `activity` is absent because a running turn already announces itself above the composer, on a
/// surface built for it. All three stay valid config values.
const DEFAULT_LEFT_MODULES: [StatuslineModule; 5] = [
    StatuslineModule::Project,
    StatuslineModule::Branch,
    StatuslineModule::Pr,
    StatuslineModule::Todo,
    StatuslineModule::Changes,
];
const DEFAULT_RIGHT_MODULES: [StatuslineModule; 5] = [
    StatuslineModule::Context,
    StatuslineModule::Cost,
    StatuslineModule::Tokens,
    StatuslineModule::Thinking,
    StatuslineModule::Model,
];

/// Rows from a single-line composer top through the bottom statusline.
pub const COMPOSER_BOTTOM_HEIGHT: u16 = 5;

/// Runtime metadata supplied when the TUI starts.
#[derive(Debug, Clone)]
pub struct StatusContext {
    /// Active model identifier
    pub model: String,
    /// The provider serving it
    pub provider: String,
    /// Active model context-window size
    pub context_window: u32,
    /// Active thinking level
    pub thinking: ThinkingLevel,
    /// Current git branch
    pub git_branch: Option<String>,
    /// Whether the Git worktree has changes
    pub git_dirty: bool,
    /// Project name for the left anchor
    pub project: Option<String>,
    /// Compact working-directory path for the optional cwd module
    pub cwd: Option<String>,
    /// Whether the session starts in walk-away mode (`--auto-approve`)
    pub auto_approve: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    /// The badge a dedicated footer leads with, rather than a configurable module.
    Heading,
    /// The keys that footer explains.
    Hint,
    Project,
    Branch,
    PullRequest,
    Todo,
    Changes,
    Cwd,
    Activity,
    Context,
    Tokens,
    Thinking,
    Model,
    Cost,
    /// Walk-away mode, drawn only while it is on — not a configurable module.
    AutoApprove,
}

impl SegmentKind {
    /// What this module sits on.
    ///
    /// By kind rather than by position, so a module dropping out for width does not re-shade every
    /// module behind it — the bar's colors have to stay still as the terminal is resized. Read from
    /// the outside in, the assignment is a ramp: the accent at each end, receding toward the middle,
    /// which is what puts the eye on the two facts worth anchoring — where you are, and what is
    /// answering.
    fn surface(self) -> Surface {
        match self {
            // The long, variable strings — a path, a checklist item, a list of keys — plus the one
            // module whose *color* is its datum. A surface behind the context reading competes with
            // the green/yellow/red it fills up through; on the bar that reading is unobstructed.
            // `changes` joins them because it is half affordance: a pill around a key someone is
            // meant to press reads as a label rather than as something to do.
            Self::Cwd | Self::Todo | Self::Changes | Self::Hint | Self::Context => Surface::Bare,
            // Like the context reading: its color is the datum, and a slab behind it would argue
            // with the warning hue that is the whole point of drawing it.
            Self::AutoApprove => Surface::Bare,
            Self::Heading | Self::Project | Self::Model => Surface::Slab(Depth::Hero),
            Self::Branch | Self::PullRequest | Self::Activity | Self::Thinking => {
                Surface::Slab(Depth::Raised)
            }
            // Telemetry: present, and deliberately not competing with the two above it.
            Self::Tokens | Self::Cost => Surface::Slab(Depth::Recessed),
        }
    }
}

/// What a segment is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// Straight on the bar, with no fill behind it
    Bare,
    /// A slab at this depth in the accent's recession
    Slab(Depth),
}

/// The surface `kind` actually gets in `style` — `flat` puts every module on the bar.
///
/// One owner, because the answer is needed twice: once to measure a segment and once to paint it,
/// and a bar measured as bare but painted as a slab overflows by four cells per module.
fn surface_of(kind: SegmentKind, style: StatuslineStyle) -> Surface {
    match style {
        StatuslineStyle::Slab => kind.surface(),
        StatuslineStyle::Flat | StatuslineStyle::Unknown => Surface::Bare,
    }
}

/// What a fragment's color is doing.
///
/// A neutral fragment follows whatever it sits on — muted on the bar, foreground on a receded slab,
/// ink on the accent — while a signal keeps its color because the color *is* the information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Neutral,
    Signal(Color),
}

/// One statusline entry rendered as a run of colored fragments back-to-back.
///
/// Splitting into `parts` lets an icon keep a semantic tint while the text beside it stays neutral.
/// `emphasized` bolds the whole run.
#[derive(Debug, Clone)]
struct Segment {
    kind: SegmentKind,
    parts: Vec<(String, Ink)>,
    emphasized: bool,
}

impl Segment {
    /// Cells the content occupies, before any frame around it.
    fn content_width(&self) -> usize {
        self.parts.iter().map(|(text, _)| UnicodeWidthStr::width(text.as_str())).sum()
    }

    fn is_empty(&self) -> bool {
        self.content_width() == 0
    }

    /// Bold the whole run — for a slab that anchors its cluster.
    fn emphasize(mut self) -> Self {
        self.emphasized = true;
        self
    }
}

/// Builds a segment from an optional icon plus its text.
///
/// An icon with no rendering at the current tier takes its separating space with it, so the bar is
/// never one cell narrower for want of a glyph — which is the shift `glyph_tiers.rs` fails on. Only
/// the tool vocabulary is omitted that way, and a tool icon has no business on the bar, but the
/// guard is here because the failure is a column of drift rather than anything you would notice.
fn segment(
    kind: SegmentKind,
    icon: Option<(&str, Ink)>,
    text: impl Into<String>,
    text_ink: Ink,
) -> Segment {
    let mut parts = Vec::new();
    if let Some((glyph, ink)) = icon.filter(|(glyph, _)| !glyph.is_empty()) {
        parts.push((crate::glyphs::icon_prefix(glyph), ink));
    }
    let text = text.into();
    if !text.is_empty() {
        parts.push((text, text_ink));
    }
    Segment { kind, parts, emphasized: false }
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    background: Color,
    text: Color,
    muted: Color,
    accent: Color,
    warning: Color,
    slab: SlabShades,
    thinking_off: Color,
    thinking_minimal: Color,
    thinking_low: Color,
    thinking_medium: Color,
    thinking_high: Color,
    thinking_xhigh: Color,
    thinking_ultra: Color,
}

/// Renders the bottom-anchored bar into one terminal row.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let palette = palette(&app.theme_styles);
    let (line, changes) = status_line_with_changes(app, usize::from(area.width));
    // The one module on the bar that does something gets a click region, so the affordance it
    // advertises is reachable by the pointer that is already over it.
    if let Some((start, width)) = changes {
        let x = area.x.saturating_add(start);
        if x < area.right() {
            app.hit.borrow_mut().push(
                Rect::new(x, area.y, width.min(area.right().saturating_sub(x)), 1),
                crate::mouse::MouseTarget::StatuslineChanges,
            );
        }
    }
    f.render_widget(Paragraph::new(line).style(Style::new().bg(palette.background)), area);
}

#[cfg(test)]
fn status_line(app: &App, width: usize) -> Line<'static> {
    status_line_with_changes(app, width).0
}

/// The composed bar, plus the changes module's `(column, width)` when it survived the fit.
fn status_line_with_changes(app: &App, width: usize) -> (Line<'static>, Option<(u16, u16)>) {
    if width == 0 {
        return (Line::default(), None);
    }

    let palette = palette(&app.theme_styles);
    let style = app.config.statusline.style();
    let context_replacement =
        app.dock.presentation(app.workspace_layout) == DockPresentation::Replacement;
    let mut left = left_segments(app, palette, context_replacement);
    let mut right = right_segments(app, palette);
    let shape = Shape { palette, style };
    fit_segments(&mut left, &mut right, width, shape, context_replacement);
    let laid_left = lay(&left, Side::Left, shape);
    let laid_right = lay(&right, Side::Right, shape);
    let changes = laid_left.place(SegmentKind::Changes);
    (compose_line(laid_left, laid_right, width, palette), changes)
}

pub(crate) use crate::glyphs::spinner;

fn palette(theme: &ThemeStyles) -> Palette {
    let background = theme.background.bg.or(theme.tool_bg.bg).unwrap_or(Color::Reset);
    let text = theme.text.fg.unwrap_or(Color::Reset);
    let muted = theme.muted.fg.unwrap_or(text);
    Palette {
        background,
        text,
        muted,
        accent: theme.accent.fg.unwrap_or(text),
        warning: theme.warning.fg.unwrap_or(text),
        slab: theme.slab,
        thinking_off: theme.thinking_off.fg.or(theme.muted.fg).unwrap_or(text),
        thinking_minimal: theme.thinking_minimal.fg.unwrap_or(text),
        thinking_low: theme.thinking_low.fg.or(theme.warning.fg).unwrap_or(text),
        thinking_medium: theme.thinking_medium.fg.or(theme.success.fg).unwrap_or(text),
        thinking_high: theme.thinking_high.fg.or(theme.accent.fg).unwrap_or(text),
        thinking_xhigh: theme.thinking_xhigh.fg.or(theme.accent_secondary.fg).unwrap_or(text),
        thinking_ultra: theme.thinking_ultra.fg.or(theme.accent_secondary.fg).unwrap_or(text),
    }
}

pub(crate) fn compact_number(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 10_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else if value < 1_000_000 {
        format!("{}k", value / 1_000)
    } else {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests;
