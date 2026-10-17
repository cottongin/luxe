//! Color math — conversion to ratatui, luminance, blending, and the plan hues kept apart.

use luxe_core::theme::ColorValue;
use ratatui::style::{Color as RatatuiColor, Style};
use std::collections::HashMap;

/// How far apart two colors must be, in RGB, to read as different whatever their hues.
const MODE_RGB_DISTANCE: f32 = 100.0;

/// How far apart two saturated hues must be, in degrees, to read as different colors.
const MODE_HUE_GAP: f32 = 30.0;

/// The color plan mode wears: `modePlan` when the theme names one, else the first of the theme's
/// other hues that reads as different from the border accent the default mode wears.
///
/// A fixed token would not do. `info` is a blue in most palettes, and in a theme whose accent is
/// also blue a plan-mode border would look like the build one, so the mode would be told by its
/// word alone. The candidates are hues every theme already defines for its own purposes.
pub(super) fn plan_hue(resolved: &HashMap<String, ColorValue>) -> Style {
    let style = |token: &str| resolved.get(token).and_then(color_value_to_ratatui);
    if let Some(color) = style("modePlan") {
        return Style::new().fg(color);
    }
    let border = rgb_token(resolved, "borderAccent").or_else(|| rgb_token(resolved, "accent"));
    let candidates = ["accentSecondary", "thinkingUltra", "info", "success", "warning"];
    let distinct = candidates.iter().find(|token| match (rgb_token(resolved, token), border) {
        (Some(hue), Some(border)) => distinct_colors(hue, border),
        // A terminal-owned color cannot be measured, so it only has to be a different one
        _ => style(token).is_some() && style(token) != style("borderAccent"),
    });
    distinct
        .or(candidates.first())
        .and_then(|token| style(token))
        .map(|color| Style::new().fg(color))
        .unwrap_or_default()
}

/// Whether two colors read as different: hues well apart, or far apart in RGB.
///
/// RGB distance alone undersells a muted palette, where an olive and an orange of similar weight
/// are close in every channel and plainly different colors; hue alone means nothing for a gray.
pub(super) fn distinct_colors(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    if rgb_distance(a, b) >= MODE_RGB_DISTANCE {
        return true;
    }
    match (hue_of(a), hue_of(b)) {
        (Some(first), Some(second)) => {
            let gap = (first - second).abs();
            gap.min(360.0 - gap) >= MODE_HUE_GAP
        }
        _ => false,
    }
}

/// Euclidean distance between two colors, in 8-bit RGB.
fn rgb_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let channel = |x: u8, y: u8| (f32::from(x) - f32::from(y)).powi(2);
    (channel(a.0, b.0) + channel(a.1, b.1) + channel(a.2, b.2)).sqrt()
}

/// A color's hue in degrees, or `None` for one too gray to have one worth comparing.
fn hue_of((red, green, blue): (u8, u8, u8)) -> Option<f32> {
    let [r, g, b] = [red, green, blue].map(|channel| f32::from(channel) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let chroma = max - min;
    if max <= 0.0 || chroma / max < 0.2 {
        return None;
    }
    let sector = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    Some(sector * 60.0)
}

/// The RGB triple of a resolved token, or `None` when it isn't an RGB color (ANSI/indexed/default
/// can't be alpha-blended without guessing the terminal palette).
pub(super) fn rgb_token(
    resolved: &HashMap<String, ColorValue>,
    token: &str,
) -> Option<(u8, u8, u8)> {
    match resolved.get(token) {
        Some(ColorValue::Rgb(r, g, b)) => Some((*r, *g, *b)),
        _ => None,
    }
}

/// Whether the theme paints a dark canvas, measured rather than guessed.
///
/// The `background` token is what decides it, so read it — a theme called `porcelain` is light and
/// a theme called `luxe` is dark, and neither says so in its name. Two cases leave nothing to
/// measure: ANSI 0–15 and the terminal default are colors the *terminal* picks, not the theme, and
/// only then does the name get a say. Falling back to dark matches what a terminal defaults to.
///
/// The token chain mirrors `bg_or("background", "userMsgBg")`, so this cannot disagree with the
/// color actually painted behind the app.
pub(super) fn theme_is_dark(resolved: &HashMap<String, ColorValue>, theme_name: &str) -> bool {
    match resolved.get("background").or_else(|| resolved.get("userMsgBg")) {
        Some(ColorValue::Rgb(red, green, blue)) => is_dark_rgb(*red, *green, *blue),
        // The 6×6×6 cube and the grayscale ramp are fixed by the spec, so they can be measured.
        Some(ColorValue::Ansi(index)) if *index >= 16 => {
            let (red, green, blue) = indexed_rgb(*index);
            is_dark_rgb(red, green, blue)
        }
        _ => !theme_name.to_lowercase().contains("light"),
    }
}

/// Whether a color is perceptually darker than mid-gray.
///
/// The threshold is the relative luminance of `L* = 50` — the midpoint of *perceived* lightness,
/// which sits far darker than the midpoint of the sRGB byte range.
pub(super) fn is_dark_rgb(red: u8, green: u8, blue: u8) -> bool {
    const MID_GRAY_LUMINANCE: f32 = 0.1842;
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue) < MID_GRAY_LUMINANCE
}

/// The RGB an xterm-256 index 16–255 stands for: the 6×6×6 color cube, then the grayscale ramp.
fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return (level, level, level);
    }
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let index = index - 16;
    (LEVELS[(index / 36) as usize], LEVELS[(index % 36 / 6) as usize], LEVELS[(index % 6) as usize])
}

/// The RGB triple of a resolved [`RatatuiColor`], or `None` when it isn't blendable.
pub(super) fn rgb_color(color: Option<RatatuiColor>) -> Option<(u8, u8, u8)> {
    match color {
        Some(RatatuiColor::Rgb(r, g, b)) => Some((r, g, b)),
        _ => None,
    }
}

/// Alpha-blend `hue` over `base` (`hue*alpha + base*(1-alpha)`) into an RGB color.
pub(super) fn blend_rgb(hue: (u8, u8, u8), base: (u8, u8, u8), alpha: f32) -> RatatuiColor {
    let mix = |h: u8, b: u8| (h as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
    RatatuiColor::Rgb(mix(hue.0, base.0), mix(hue.1, base.1), mix(hue.2, base.2))
}

/// Convert a [`ColorValue`] to a ratatui [`ratatui::style::Color`].
///
/// Returns `None` for unparseable values (e.g. ANSI index > 255, invalid hex).
pub(super) fn color_value_to_ratatui(value: &ColorValue) -> Option<RatatuiColor> {
    match value {
        ColorValue::Rgb(r, g, b) => Some(RatatuiColor::Rgb(*r, *g, *b)),
        ColorValue::Ansi(idx) => {
            if *idx < 16 {
                Some(match *idx {
                    0 => RatatuiColor::Black,
                    1 => RatatuiColor::Red,
                    2 => RatatuiColor::Green,
                    3 => RatatuiColor::Yellow,
                    4 => RatatuiColor::Blue,
                    5 => RatatuiColor::Magenta,
                    6 => RatatuiColor::Cyan,
                    7 => RatatuiColor::White,
                    8 => RatatuiColor::DarkGray,
                    9 => RatatuiColor::LightRed,
                    10 => RatatuiColor::LightGreen,
                    11 => RatatuiColor::LightYellow,
                    12 => RatatuiColor::LightBlue,
                    13 => RatatuiColor::LightMagenta,
                    14 => RatatuiColor::LightCyan,
                    15 => RatatuiColor::White,
                    _ => unreachable!(),
                })
            } else {
                Some(RatatuiColor::Indexed(*idx))
            }
        }
        ColorValue::Default => None,
    }
}
