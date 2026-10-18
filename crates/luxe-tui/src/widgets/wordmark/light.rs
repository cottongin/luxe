//! Light — the shine's band, the curve it falls off along, and the blend both lights paint with.
//!
//! The math of the two lights on their own, apart from the art they land on: the parent decides
//! which cells are ink and asks here how far each one is lifted.

use super::{Wordmark, WordmarkInk, BAND, CYCLE_TICKS, LEAN, PEAK_DARK, PEAK_LIGHT, SWEEP_TICKS};
use ratatui::style::Color;

/// One tick of one pass: where the highlight band sits, and how hard it pushes.
pub(super) struct Pass {
    center: f32,
    half: f32,
    peak: f32,
    lean: f32,
}

impl Pass {
    /// The pass in flight at `tick`, or `None` while the logotype rests.
    pub(super) fn at(size: Wordmark, ink: WordmarkInk, tick: u64) -> Option<Self> {
        let phase = tick % CYCLE_TICKS;
        if phase >= SWEEP_TICKS {
            return None;
        }
        let span = size.span();
        let half = (span * BAND).max(1.0);
        let travel = span + 2.0 * half;
        let progress = phase as f32 / SWEEP_TICKS as f32;

        Some(Self {
            // The band starts and ends clear of the letters, so a pass arrives and leaves rather
            // than appearing half-drawn across them.
            center: -half + progress * travel,
            half,
            peak: if ink.dark { PEAK_DARK } else { PEAK_LIGHT },
            lean: LEAN,
        })
    }

    /// How far this cell is lifted toward the highlight.
    pub(super) fn lift(&self, column: u16, row: u16) -> f32 {
        let axis = f32::from(column) + self.lean * f32::from(row);
        let distance = ((axis - self.center).abs() / self.half).min(1.0);
        smoothstep(1.0 - distance) * self.peak
    }
}

/// Smooth 0→1 with zero slope at both ends, so a band edge has no seam.
///
/// The shape of the highlight, and the whole reason it reads as light rather than as a stripe: a
/// linear falloff leaves a crease along the ridge and a hard shoulder at each edge. Public so the
/// website samples this curve rather than approximating it.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Alpha-blend `base` `amount` of the way toward `target`.
///
/// An ANSI/indexed palette has no channels to mix, so an unblendable color comes back untouched
/// rather than as an invented RGB approximation of the terminal's own.
pub(super) fn blend_toward(base: Color, target: Color, amount: f32) -> Color {
    let (Color::Rgb(red, green, blue), Color::Rgb(tr, tg, tb)) = (base, target) else {
        return base;
    };
    if amount <= 0.0 {
        return base;
    }
    let amount = amount.clamp(0.0, 1.0);
    let lift = |channel: u8, toward: u8| {
        (f32::from(channel) + (f32::from(toward) - f32::from(channel)) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lift(red, tr), lift(green, tg), lift(blue, tb))
}
