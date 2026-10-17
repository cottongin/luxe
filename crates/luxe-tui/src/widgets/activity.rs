//! Activity bar — a comet of accent shades sweeping back and forth over a rail of units.
//!
//! Hand-rolled rather than driven by a spinner crate: the look wanted here shades every cell
//! independently, runs the comet clean off one end of the rail, waits a beat, and brings it back the
//! other way. A frames-of-strings spinner has one color per frame and cannot do that.
//!
//! # Why the units are block elements
//!
//! The rail is packed — one glyph per cell, no separator — so each unit's gap has to come from
//! inside its own glyph. A terminal is an integer cell grid: there is no half-cell advance, and
//! `U+2009 THIN SPACE` and friends get snapped to one cell or none.
//!
//! That makes the choice of glyph a question about *font coverage*, not taste. The geometric shapes
//! (`▰`, `▮`, `■`, and braille) are absent from a great many programming fonts, so they are drawn
//! by whatever fontconfig substitutes, at that font's metrics. In Noto Sans Mono, the usual
//! substitute, `▰` is a **double-width** glyph that Unicode calls one column wide, so the ink
//! spills over both edges into its neighbors: packed, the units bleed together; spaced, they read
//! as too far apart. Nothing can measure which way a given font will go, so those shapes are not
//! offered. Block elements (`U+2580`–`U+259F`) are what is left, and they are the exception that
//! matters — terminals depend on them, so fonts ship them at exact cell fractions.
//!
//! The rail is lit by the comet rather than drawn at a fixed weight: a spent unit dims with its
//! distance from the comet's head, so the bar's ends dissolve into the canvas and only the stretch
//! the comet is passing through reads as a rail at all.
//!
//! The same idiom as [`super::fill_gauge`]: the indicator and the gauge are the same bar, one
//! moving and one standing still.

use super::cells::{push_cell, ramp_step};
use ratatui::style::Style;
use ratatui::text::Span;

/// The glyphs a style draws the bar with: lit units, and the spent rail they sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityGlyphs {
    /// Lit units, heaviest (the comet's leading cell) first, tapering toward [`Self::track`].
    ///
    /// The comet dims *and* thins as it trails away, so its last cells converge on the rail in both
    /// color and shape and vanish outright. A style with one entry simply doesn't taper.
    pub lit: &'static [char],
    /// Unlit unit.
    pub track: char,
}

impl ActivityGlyphs {
    /// `▌▍▎▏` — block segments narrowing from a half cell to an eighth: the default.
    ///
    /// Each glyph inks a fraction of its cell and leaves the rest clear, so packed units carry a
    /// real gap with no separator character — and, unlike the geometric shapes, a gap no font
    /// substitution can take away. The ramp ends on `▏`, which *is* the rail, so the comet's tail
    /// narrows into it and disappears.
    pub const BARS: Self = Self { lit: &['▌', '▍', '▎', '▏'], track: '▏' };
    /// `●•·` — round marks shrinking onto a dotted rail. Softer and lighter than [`Self::BARS`].
    pub const DOTS: Self = Self { lit: &['●', '•', '·'], track: '·' };
}

/// Ticks the comet spends on each cell, as a fraction — `2/1` is two.
///
/// A fraction rather than a plain count because at the ~30fps tick a whole frame is a coarse step:
/// three frames a cell and two are a 50% difference in speed, with nothing available in between.
/// This pace crosses the default bar in about two seconds — half the linear speed of the old spaced
/// bar, which was reported as distracting an inch above the prompt, and unhurried enough not to
/// compete with the text the user is trying to read.
const CELL_FRAMES: (u64, u64) = (2, 1);

/// Ticks the rail stays empty between legs.
///
/// Long enough that the reversal registers as a beat rather than a bounce, short enough that the
/// indicator never looks stalled.
const HOLD_FRAMES: u64 = 9;

/// One frame of the bar: `width` packed units with a shaded comet sweeping back and forth.
///
/// `trail` is the comet's palette, brightest (its leading cell) first; `rail` is the spent unit's,
/// nearest the comet first. The comet enters at one end, runs clean off the other — leaving the rail
/// empty for `HOLD_FRAMES` — then re-enters from the end it left by and sweeps back. It bounces
/// rather than wrapping because a run reappearing at the far edge reads as something restarting; a
/// bounce reads as one thing still going.
pub fn activity_bar(
    frame: u64,
    width: usize,
    glyphs: ActivityGlyphs,
    trail: &[Style],
    rail: &[Style],
) -> Vec<Span<'static>> {
    // Packed: one glyph per cell, no separator. The gradient is what tells the units apart now, and
    // a gap between them breaks the flow it draws.
    let count = width;
    let comet = comet_span(count, trail.len());
    if count == 0 || comet == 0 || rail.is_empty() {
        return Vec::new();
    }
    let sweep = sweep(frame, count, comet);
    let halo = halo_span(count);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for index in 0..count {
        match sweep.shade_of(index, comet) {
            // Color and shape ride the same ramp, so a cell dims and thins in step.
            Some(behind) => push_cell(
                &mut spans,
                glyphs.lit[ramp_step(comet, behind, glyphs.lit.len())],
                trail[ramp_step(comet, behind, trail.len())],
            ),
            None => {
                let step = match sweep.distance_from_head(index) {
                    Some(away) => ramp_step(halo + 1, away.min(halo), rail.len()),
                    // Nothing lit: the glow has left the bar entirely.
                    None => rail.len() - 1,
                };
                push_cell(&mut spans, glyphs.track, rail[step]);
            }
        }
    }
    spans
}

/// Cells the comet's glow reaches along the rail.
///
/// Half the bar, so a comet mid-rail lights most of it and only the ends fall away — a pool of light
/// traveling an unlit track, rather than a fully drawn track with a bright patch on it.
fn halo_span(count: usize) -> usize {
    (count / 2).max(2)
}

/// Cells the comet spans on a rail of `count` units, given how many shades are on offer.
///
/// Two fifths of the rail: enough that the gradient has room to read as a gradient, little enough
/// that the rail is visibly a rail. Narrow bars get a shorter comet rather than a solid one.
fn comet_span(count: usize, shades: usize) -> usize {
    (count * 2 / 5).max(2).min(shades).min(count)
}

/// Where the comet is in its cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sweep {
    /// The comet is on the move.
    ///
    /// Args: `{ head, forward }` — the leading cell's index, which may sit off either end of the
    /// rail while the tail is still draining, and which way it is traveling.
    Moving { head: isize, forward: bool },
    /// The rail is empty, between legs.
    Hold,
}

impl Sweep {
    /// How far cell `index` trails behind the comet's head, or `None` when it is unlit.
    fn shade_of(self, index: usize, comet: usize) -> Option<usize> {
        let Sweep::Moving { head, forward } = self else { return None };
        let index = index as isize;
        // The tail lies opposite the direction of travel, so which side counts flips with it.
        let behind = if forward { head - index } else { index - head };
        (behind >= 0 && behind < comet as isize).then_some(behind as usize)
    }

    /// How far cell `index` lies from the comet's head, whichever side it is on — the glow is
    /// ambient, so it reaches ahead as well as behind. `None` while there is no comet on the rail.
    fn distance_from_head(self, index: usize) -> Option<usize> {
        let Sweep::Moving { head, .. } = self else { return None };
        Some((index as isize - head).unsigned_abs())
    }
}

/// Frames one leg of `steps` cells takes at [`CELL_FRAMES`].
fn leg_frames(steps: u64) -> u64 {
    (steps * CELL_FRAMES.0).div_ceil(CELL_FRAMES.1)
}

/// Cells the comet has travelled by frame `phase` of a leg, capped at the leg's length.
///
/// The cap matters at the end of a leg: rounding the leg's length up can leave a frame or two over,
/// and the head must stop exactly `comet` cells past the edge rather than drifting further.
fn cells_travelled(phase: u64, steps: u64) -> isize {
    (phase * CELL_FRAMES.1 / CELL_FRAMES.0).min(steps.saturating_sub(1)) as isize
}

/// Resolves the animation frame to a position in the back-and-forth cycle.
///
/// A leg runs the head from one edge to `comet` cells past the other, so the tail clears the rail
/// entirely before the hold begins — the pause is empty rail, not a stalled comet.
fn sweep(frame: u64, count: usize, comet: usize) -> Sweep {
    let steps = (count + comet - 1) as u64;
    let leg = leg_frames(steps);
    let cycle = 2 * (leg + HOLD_FRAMES);
    let phase = frame % cycle;

    if phase < leg {
        Sweep::Moving { head: cells_travelled(phase, steps), forward: true }
    } else if phase < leg + HOLD_FRAMES {
        Sweep::Hold
    } else if phase < 2 * leg + HOLD_FRAMES {
        let step = cells_travelled(phase - leg - HOLD_FRAMES, steps);
        Sweep::Moving { head: count as isize - 1 - step, forward: false }
    } else {
        Sweep::Hold
    }
}

#[cfg(test)]
mod tests;

/// Prints a cycle of the bar in a real theme's colors, so the motion can be eyeballed as text.
///
/// `cargo test -p luxe-tui --lib activity_bar_filmstrip -- --nocapture`
#[cfg(test)]
mod filmstrip {
    use super::*;
    use crate::theme::styles::ThemeStyles;
    use ratatui::style::Color;

    /// The SGR sequence for a span's foreground, so a terminal shows the real gradient.
    fn sgr(style: Style) -> String {
        match style.fg {
            Some(Color::Rgb(r, g, b)) => format!("\x1b[38;2;{r};{g};{b}m"),
            Some(Color::Indexed(index)) => format!("\x1b[38;5;{index}m"),
            _ => "\x1b[39m".to_string(),
        }
    }

    #[test]
    fn activity_bar_filmstrip() {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let width = 24;
        let dark = registry.resolve("catppuccin").expect("catppuccin");
        for (theme, styles) in [
            ("catppuccin", ThemeStyles::from_resolved(&dark, "catppuccin")),
            ("light-fixture", crate::fixture::light_theme_styles()),
        ] {
            for (label, glyphs) in [("bars", ActivityGlyphs::BARS), ("dots", ActivityGlyphs::DOTS)]
            {
                println!("\n  {theme}/{label}");
                let comet = comet_span(width, styles.activity_trail.len());
                let cycle = 2 * (leg_frames((width + comet - 1) as u64) + HOLD_FRAMES);
                for frame in (0..cycle).step_by(3) {
                    let rendered = activity_bar(
                        frame,
                        width,
                        glyphs,
                        &styles.activity_trail,
                        &styles.activity_rail,
                    )
                    .into_iter()
                    .map(|span| format!("{}{}", sgr(span.style), span.content))
                    .collect::<String>();
                    println!("  {frame:>3}  {rendered}\x1b[0m");
                }
            }
        }
    }
}
