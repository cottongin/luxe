//! Statusline activity — the live turn's label, glyph and elapsed time, and the thinking level's
//! label and color, as other surfaces show them too.

use super::Palette;
use crate::app::{App, AppState};
use crate::glyphs::spinner;
use luxe_core::provider::ThinkingLevel;
use ratatui::style::Color;

pub(super) fn activity_text(app: &App) -> String {
    let (_, label) = activity_parts(app).unwrap_or(("•", "idle".to_string()));
    let mut text = label;
    if let Some(elapsed) = app.status.elapsed(app.frame_now()) {
        text.push_str(" · ");
        text.push_str(&format_elapsed_compact(elapsed));
    }
    text
}

/// The animated activity glyph as a heap string, or `None` when idle (`activity_text` supplies the
/// idle bullet inline so the icon slot stays empty).
pub(super) fn activity_glyph(app: &App) -> Option<String> {
    activity_parts(app).map(|(glyph, _)| glyph.to_string())
}

/// Returns the live activity glyph and label for an in-flight turn, or `None` when idle.
///
/// Shared by the statusline, the Context Dock footer, and the above-composer indicator so a
/// running turn reads the same everywhere: an animated spinner plus the active tool (or
/// `thinking`/`compacting`).
pub(crate) fn activity_parts(app: &App) -> Option<(&'static str, String)> {
    match app.status.state {
        AppState::Idle => None,
        // "working" — not "thinking": this is the generic "a turn is streaming and no tool is
        // running" state, which has nothing to do with the model's reasoning level (that's the
        // separate, correctly-labeled thinking row in MODEL).
        AppState::Streaming => Some((
            spinner(app.status.spinner_frame),
            app.status.active_tool.clone().unwrap_or_else(|| "working".to_string()),
        )),
        AppState::Compacting => Some((spinner(app.status.spinner_frame), "compacting".to_string())),
    }
}

/// Formats a live timer: `45s`, `1m 23s`, then `1h 04m`.
///
/// The one format for every clock that counts up, tool cards included, so a turn and the call
/// inside it never read in two notations. Whole seconds: this is redrawn every frame, and a tenths
/// digit is a number moving ten times a second beside a bar that already shows motion. Seconds go
/// past an hour, where nobody acts on them.
pub(crate) fn format_elapsed_compact(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3_599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3_600, (secs % 3_600) / 60),
    }
}

pub(crate) fn thinking_label(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "min",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "med",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhi",
        ThinkingLevel::Ultra => "ultra",
    }
}

/// The thinking segment's per-level color, or `None` when that coloring is not opted in.
pub(super) fn thinking_color(app: &App, palette: Palette) -> Option<Color> {
    if app.config.statusline.thinking_colors != Some(true) {
        return None;
    }
    Some(match app.status.thinking {
        ThinkingLevel::Off => palette.thinking_off,
        ThinkingLevel::Minimal => palette.thinking_minimal,
        ThinkingLevel::Low => palette.thinking_low,
        ThinkingLevel::Medium => palette.thinking_medium,
        ThinkingLevel::High => palette.thinking_high,
        ThinkingLevel::XHigh => palette.thinking_xhigh,
        ThinkingLevel::Ultra => palette.thinking_ultra,
    })
}
