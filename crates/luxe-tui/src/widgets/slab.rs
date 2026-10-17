//! slab — the filled block the bar, the headings and the selected row are cut from.
//!
//! A body filled with the slab's color, with a pad cell of that color inside each end, because a
//! word that starts on the edge of its own highlight reads as pinned to it. What happens at the ends
//! depends on the user. A selected row and the `@` picker's open tab end on a half-cell cap: the
//! slab's color as the *foreground* over the canvas behind it, so the fill stops half a cell in. A
//! section heading and the statusline's bands fade instead ([`fade`], [`ramp`]).
//!
//! # Why detached
//!
//! A connected powerline gives one cell the job of transitioning between two fills, so its glyph has
//! to tile with the neighboring cell exactly, and a terminal that draws it a column too wide shows
//! the thin seam those separators are notorious for. A detached slab asks nothing of its neighbor:
//! the worst a badly drawn cap can do is change the shape of an edge that has nothing to line up
//! with.
//!
//! # Why half blocks
//!
//! Not the Powerline Extra triangles (`U+E0BA`, `U+E0BC`): terminals draw those at the wrong size
//! (alacritty inks the whole right edge of the cell), so a closing cap comes out blunter than the
//! opening one. `U+2590` and `U+258C` come from the block-element range the wordmark is drawn in,
//! for the same reason: Unicode 1.1, in every font ever cut, and synthesized against the cell box
//! by every terminal that synthesizes anything. There is no tier and no fallback, because there is
//! nothing left to fall back from.
//!
//! # Why a fade, where there is room
//!
//! A slab's free edge steps down into the canvas over a few cells instead of stopping on a half
//! block: each cell is a plain space whose *background* is the slab's color with more of the canvas
//! mixed in, the way Powerlevel10k's faded segments end. It needs nothing from the font at all, so
//! it looks the same in every terminal that draws color, which the half block only nearly does. An
//! edge against a boundary (the screen's side, the column a heading labels) stays flush; an edge
//! into open space fades. A theme that names ANSI colors has no channels to mix, so its fade is the
//! half-block cap it always had.

use crate::theme::styles::{Depth, ThemeStyles};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// Cells a slab spends around its content: a cap and a space of padding at each end.
pub const SLAB_FRAME: usize = 4;

/// Cells every change of color takes: a free edge fading into the canvas, and one slab stepping
/// down to the next.
///
/// Three, and never fewer, as in the prompt this comes from: one step between two colors reads as
/// a seam, two as a hard edge with a smudge on it, and four start to look like a second, dimmer
/// slab. A bar that is short of room drops a module rather than a step.
pub const FADE_CELLS: usize = 3;

/// Which way a fade runs, reading left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fade {
    /// Out of the canvas and up into the slab: a leading edge
    In,
    /// Down out of the slab into the canvas: a trailing edge
    Out,
}

/// `cells` of background stepping between `surface` and `canvas`, in reading order.
///
/// A [`ramp`] from the slab into the canvas or back out of it. With no RGB to mix, a fade is the
/// half-block cap that edge had before, which keeps an ANSI theme's slab the shape it has always
/// been.
pub fn fade(surface: Color, canvas: Color, cells: usize, direction: Fade) -> Vec<Span<'static>> {
    if rgb(surface).is_none() || rgb(canvas).is_none() {
        let cap = match direction {
            Fade::In => Cap::Open,
            Fade::Out => Cap::Close,
        };
        return vec![cap.span(surface, canvas)];
    }
    match direction {
        Fade::In => ramp(canvas, surface, cells),
        Fade::Out => ramp(surface, canvas, cells),
    }
}

/// `cells` of background stepping from `from` to `to`, in reading order, reaching neither.
///
/// Each step mixes a further equal share of `to` in, so the colors either side of the run are its
/// first and last full steps: three cells between two slabs go a quarter, a half and three quarters
/// of the way. Without channels to mix, the run is one cell of `to`, a hard edge that keeps the
/// band the same shape in an ANSI theme.
pub fn ramp(from: Color, to: Color, cells: usize) -> Vec<Span<'static>> {
    let (Some(start), Some(end)) = (rgb(from), rgb(to)) else {
        return vec![Span::styled(" ", Style::new().bg(to))];
    };
    (1..=cells)
        .map(|step| {
            let share = step as f32 / (cells + 1) as f32;
            Span::styled(" ", Style::new().bg(mix(end, start, share)))
        })
        .collect()
}

/// `share` of `over` mixed into `base`.
fn mix(over: (u8, u8, u8), base: (u8, u8, u8), share: f32) -> Color {
    let channel = |over: u8, base: u8| {
        (f32::from(over) * share + f32::from(base) * (1.0 - share)).round() as u8
    };
    Color::Rgb(channel(over.0, base.0), channel(over.1, base.1), channel(over.2, base.2))
}

fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        _ => None,
    }
}

/// Cells a selected list row spends on its frame.
///
/// The same cost as [`SLAB_FRAME`], and for the same reason: a row is the bar's shape at full width,
/// so its content stands off the cut by the same cell the bar's does.
///
/// A row that right-aligns anything has to size its content to `width - SELECTION_FRAME`, or the
/// closing cap pushes it off the end.
pub const SELECTION_FRAME: usize = SLAB_FRAME;

/// Cells before a row's content, whether or not it is under the cursor.
///
/// Half the frame: the leading cap and the pad inside it. Spent on a plain row too, or the label
/// column would move sideways as the selection arrived — which makes this the offset a surface has
/// to know about when it is measuring where its own ink starts.
pub const SELECTION_INSET: usize = SELECTION_FRAME / 2;

/// Which half of one diagonal cut a cap carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// The leading edge, inked below the diagonal
    Open,
    /// The trailing edge, inked above it
    Close,
}

impl Cap {
    /// The half block this cap is cut with — the inner half, so the fill stops short of the edge.
    ///
    /// Each one inks the half facing the slab's body: an opening cap fills the right of its cell and
    /// a closing cap the left. Swap them and the slab grows a cell at each end while appearing to
    /// float away from its own content.
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Open => "\u{2590}",
            Self::Close => "\u{258c}",
        }
    }

    /// The single cell drawing this cap in `surface` over `canvas`.
    ///
    /// The slab's color is the *foreground* here and the canvas the background — the cell is half
    /// one and half the other. Painting it the other way round fills the whole cell and squares the
    /// end off.
    pub fn span(self, surface: Color, canvas: Color) -> Span<'static> {
        Span::styled(self.glyph(), Style::new().fg(surface).bg(canvas))
    }
}

/// One detached slab: `body` on `surface`, capped at both ends over `canvas`.
///
/// Each body span keeps its own foreground and takes the surface as its background, so an icon can
/// hold a semantic tint while the text beside it stays neutral. `emphasis` rides every body span,
/// which is how the one slab that anchors a cluster gets its weight.
pub fn slab(
    body: impl IntoIterator<Item = Span<'static>>,
    surface: Color,
    canvas: Color,
    emphasis: Style,
) -> Vec<Span<'static>> {
    capped(body, surface, canvas, emphasis)
}

fn capped(
    body: impl IntoIterator<Item = Span<'static>>,
    surface: Color,
    canvas: Color,
    emphasis: Style,
) -> Vec<Span<'static>> {
    let pad = || Span::styled(" ", Style::new().bg(surface));
    let mut spans = vec![Cap::Open.span(surface, canvas), pad()];
    spans.extend(body.into_iter().map(|span| {
        let style = span.style.bg(surface).patch(emphasis);
        Span::styled(span.content, style)
    }));
    spans.push(pad());
    spans.push(Cap::Close.span(surface, canvas));
    spans
}

/// A section heading cut into a slab at `depth`, over the panel fill behind it.
///
/// One owner for the badge, because two surfaces draw it: the Context Dock's section headings and
/// the Guided Review rail's, which sit at the same screen column so that moving between chat and
/// review reads as one product rather than two.
///
/// It hugs its label rather than spanning the rail. A slab across a whole row already means
/// *chosen* ([`selection_row`]), and at forty cells a cap is a fortieth of the shape and stops
/// reading as one. The cap tapers into `canvas`, which is the panel's own fill — not the app
/// background the bar floats on, since those can differ in a custom theme.
///
/// # Why only one edge fades
///
/// A badge heads the column under it, and a statusline module does not — it floats on a bar with
/// background either side. So this one is **flush** at its leading edge and fades only at the
/// trailing one, over [`FADE_CELLS`]. A leading cap would start the ink half a cell in, putting
/// `MODEL` half a character right of the `anthropic` it labels, and a vertical edge does not get to
/// be approximate.
pub fn badge(
    label: &'static str,
    depth: Depth,
    canvas: Color,
    theme: &ThemeStyles,
) -> Vec<Span<'static>> {
    // Ink on the accent, body text on a receded surface — the bar's rule, so a heading and a
    // statusline module at the same depth wear the same two colors.
    let ink = match depth {
        Depth::Hero => theme.slab.ink,
        _ => theme.text.fg.unwrap_or(theme.slab.ink),
    };
    let surface = theme.slab.at(depth);
    let pad = || Span::styled(" ", Style::new().bg(surface));
    let mut spans = vec![
        pad(),
        pad(),
        Span::styled(label, Style::new().fg(ink).bg(surface).add_modifier(Modifier::BOLD)),
        pad(),
    ];
    spans.extend(fade(surface, canvas, FADE_CELLS, Fade::Out));
    spans
}

/// A [`badge`] for the face of a switch that is *not* showing: the same footprint, on the idle
/// surface, in the comment color — or body text when `lifted` (under the pointer).
///
/// Same width as its lit twin on purpose, so a pair of them never moves as the choice flips, and
/// a slab of its own rather than a bare word, so it reads as the other half of the control rather
/// than a caption beside it.
pub fn idle_badge(
    label: &'static str,
    lifted: bool,
    canvas: Color,
    theme: &ThemeStyles,
) -> Vec<Span<'static>> {
    let surface = theme.slab.idle[usize::from(lifted)];
    let ink = match lifted {
        true => theme.text.fg,
        false => theme.muted.fg,
    }
    .unwrap_or(theme.slab.ink);
    let pad = || Span::styled(" ", Style::new().bg(surface));
    let mut spans =
        vec![pad(), pad(), Span::styled(label, Style::new().fg(ink).bg(surface)), pad()];
    spans.extend(fade(surface, canvas, FADE_CELLS, Fade::Out));
    spans
}

/// A list row cut into a slab when `selected`, on the theme's selection surface.
///
/// The one call a picker or the dashboard menu makes. `content` is the row *without* its frame —
/// this supplies it — and anything the row right-aligns must be sized to `width - SELECTION_FRAME`.
///
/// The color stays the theme's own `selectedBg`, so a chosen row means the same thing here as in the
/// transcript and the Configuration surface; only the *shape* joins the bar. A theme that names no
/// selection surface borrows the near end of the slab ramp instead of drawing nothing.
pub fn selection_row(
    content: Vec<Span<'static>>,
    width: usize,
    selected: bool,
    theme: &ThemeStyles,
) -> Line<'static> {
    selection_row_on(content, width, 0, selected, theme.slab.canvas, theme)
}

/// The same row, for a list that sits on a panel's own fill rather than the app background.
///
/// A cap is the slab's color in the foreground *over* a canvas, so it has to be told which canvas:
/// drawn against the color behind a filled panel it leaves a notch exactly where the diagonal
/// should meet the panel. The Context Dock and the Guided Review rail are both raised surfaces, and
/// the section [`badge`] above their rows already takes its canvas the same way.
///
/// `lean` indents the whole shape, for a list with a level under its headings: the highlight moves
/// in with the row rather than reaching back out to the margin, which is what keeps the nesting
/// legible under the cursor.
pub fn selection_row_on(
    content: Vec<Span<'static>>,
    width: usize,
    lean: usize,
    selected: bool,
    canvas: Color,
    theme: &ThemeStyles,
) -> Line<'static> {
    let surface = theme.selected_bg.bg.unwrap_or_else(|| theme.slab.at(Depth::Raised));
    row(content, width, lean, selected, surface, canvas)
}

/// A row taller than one line, cut into a **single** continuous slab.
///
/// Every line is capped at the same two columns, which is all a square end needs to read as one
/// shape. Half blocks stack with no per-line inset and no tallest-row anchor to keep a list of them
/// aligned.
pub fn selection_block(
    rows: Vec<Vec<Span<'static>>>,
    width: usize,
    selected: bool,
    theme: &ThemeStyles,
) -> Vec<Line<'static>> {
    let surface = theme.selected_bg.bg.unwrap_or_else(|| theme.slab.at(Depth::Raised));
    let canvas = theme.slab.canvas;
    rows.into_iter().map(|content| row(content, width, 0, selected, surface, canvas)).collect()
}

/// One row of slab: `room` cells wide, indented `lean` cells from the margin.
///
/// One place decides what a chosen row spends on its frame and where its content starts, so a
/// single-line row and a line of a [`selection_block`] cannot drift apart.
fn row(
    content: Vec<Span<'static>>,
    room: usize,
    lean: usize,
    selected: bool,
    surface: Color,
    canvas: Color,
) -> Line<'static> {
    if !selected || room <= SELECTION_FRAME {
        let mut spans = vec![Span::raw(" ".repeat(lean + SELECTION_INSET))];
        spans.extend(content);
        return Line::from(spans);
    }
    let used: usize =
        content.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
    let fill = room.saturating_sub(SELECTION_FRAME + used);

    let pad = || Span::styled(" ", Style::new().bg(surface));
    let mut spans = Vec::with_capacity(content.len() + 6);
    if lean > 0 {
        spans.push(Span::raw(" ".repeat(lean)));
    }
    spans.push(Cap::Open.span(surface, canvas));
    spans.push(pad());
    spans
        .extend(content.into_iter().map(|span| Span::styled(span.content, span.style.bg(surface))));
    if fill > 0 {
        spans.push(Span::styled(" ".repeat(fill), Style::new().bg(surface)));
    }
    spans.push(pad());
    spans.push(Cap::Close.span(surface, canvas));
    Line::from(spans)
}

#[cfg(test)]
mod tests;
