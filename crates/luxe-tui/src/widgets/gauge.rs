//! Gauge — a static fill bar drawn in the activity bar's own vocabulary.
//!
//! The indicator above the composer and this bar are the same shape, one moving and one standing
//! still, so they share their shading: a run of cells that brightens toward its leading edge, over a
//! rail that fades out past it into the canvas.
//!
//! It meters a tool run that is still dispatching, which is the case a bar earns: the value moves
//! while you watch it. A standing figure gets printed instead, since a bar beside it would say
//! nothing the number does not.
//!
//! # Why the units are lower blocks
//!
//! `▁▂▃▄▅▆▇█` all occupy the full cell width and differ only in height, which is what a meter wants:
//! the boundary cell can show the fraction it has actually reached instead of rounding to a lie.
//! Eight of them means ten cells resolve eighty steps, so 42% and 45% are different pictures.
//!
//! Not braille (`⣀⣤⣶⣿`): four steps rather than eight, and many programming fonts do not contain
//! it at all, leaving it to whatever fontconfig substitutes. See [`super::activity`] for what that
//! costs.

use super::cells::{push_cell, ramp_step};
use ratatui::style::Style;
use ratatui::text::Span;

/// Vertical fill ramp, empty → full. Index is the number of eighths lit.
///
/// One eighth is the thinnest ink a cell has, so an eighth-full cell and an empty one share a glyph
/// and are told apart by color — which is exactly what the leading edge of the fill is for.
const RAMP: [char; LEVELS + 1] = ['▁', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Sub-cell resolution: the lower blocks fill a cell in eight steps.
const LEVELS: usize = 8;

/// Renders `percent` (0–100) across `cells` as spans.
///
/// `fill` shades the value, its leading edge first, stretched over however much of the bar the value
/// occupies — so the frontier is always the brightest cell and the start always the dimmest, and the
/// bar reads as filling toward where it has got to. `rail` shades what is left, nearest the frontier
/// first, so the untouched remainder dissolves into the band rather than ending in a hard strip.
///
/// A value that rounds to nothing still draws the rail, so the meter never vanishes entirely.
pub fn fill_gauge(
    percent: f32,
    cells: usize,
    fill: &[Style],
    rail: &[Style],
) -> Vec<Span<'static>> {
    if cells == 0 || fill.is_empty() || rail.is_empty() {
        return Vec::new();
    }
    let ratio = (percent / 100.0).clamp(0.0, 1.0);
    let steps = (ratio * (cells * LEVELS) as f32).round() as usize;
    let full = (steps / LEVELS).min(cells);
    let partial = if full < cells { steps % LEVELS } else { 0 };
    let lit = full + usize::from(partial > 0);
    // The glow reaches half the bar past the frontier, matching the activity rail's halo.
    let halo = (cells / 2).max(2);

    let mut spans = Vec::new();
    for index in 0..cells {
        if index < lit {
            let behind = lit - 1 - index;
            let glyph = if index < full { RAMP[LEVELS] } else { RAMP[partial] };
            push_cell(&mut spans, glyph, fill[ramp_step(lit, behind, fill.len())]);
        } else {
            let away = (index - lit).min(halo);
            push_cell(&mut spans, RAMP[0], rail[ramp_step(halo + 1, away, rail.len())]);
        }
    }
    spans
}

#[cfg(test)]
mod tests;

/// Prints the meter across its range in a real theme's colors, so it can be eyeballed as text.
///
/// `cargo test -p luxe-tui --lib fill_gauge_filmstrip -- --nocapture`
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
    fn fill_gauge_filmstrip() {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let dark = registry.resolve("catppuccin").expect("catppuccin");
        for (theme, styles) in [
            ("catppuccin", ThemeStyles::from_resolved(&dark, "catppuccin")),
            ("light-fixture", crate::fixture::light_theme_styles()),
        ] {
            println!("\n  {theme}");
            for percent in (0..=100).step_by(4).map(|step| step as f32) {
                // Three hues, because the ramp is derived from whatever the caller is already
                // saying and the shading has to hold up on each of them.
                let value = if percent >= 90.0 {
                    styles.error
                } else if percent >= 75.0 {
                    styles.warning
                } else {
                    styles.text
                };
                let shades = styles.gauge_shades(value, styles.tool_bg);
                let rendered = fill_gauge(percent, 10, &shades.fill, &shades.rail)
                    .into_iter()
                    .map(|span| format!("{}{}", sgr(span.style), span.content))
                    .collect::<String>();
                println!("  {percent:>3.0}%  {rendered}\x1b[0m");
            }
        }
    }
}
