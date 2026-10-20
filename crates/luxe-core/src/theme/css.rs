//! Theme CSS — a theme's colors as CSS custom properties.
//!
//! The website and the phone app are drawn in the terminal's colors, and stay that way because
//! neither keeps a copy: both read the theme through here. `accentSecondary` is published as
//! `--luxe-accent-secondary`.

use super::ColorValue;
use std::collections::HashMap;
use std::fmt::Write as _;

/// A token's name as CSS spells it: `accentSecondary` is `accent-secondary`.
pub fn css_name(token: &str) -> String {
    let mut out = String::with_capacity(token.len() + 4);
    for (index, character) in token.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            out.push('-');
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// A color as CSS hex. An ANSI index or the terminal's default has no fixed color, so none.
pub fn hex(value: &ColorValue) -> Option<String> {
    match value {
        ColorValue::Rgb(red, green, blue) => Some(format!("#{red:02x}{green:02x}{blue:02x}")),
        _ => None,
    }
}

/// One `--luxe-*` line for each of `tokens` that `theme` gives a color, in that order.
///
/// A name that isn't plain letters and digits is skipped: theme files can be the user's own, and
/// the name lands inside a stylesheet.
pub fn custom_properties<'a>(
    theme: &HashMap<String, ColorValue>,
    tokens: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut out = String::new();
    for token in tokens {
        if !token.chars().all(|character| character.is_ascii_alphanumeric()) {
            continue;
        }
        if let Some(color) = theme.get(token).and_then(hex) {
            let _ = writeln!(out, "  --luxe-{}: {color};", css_name(token));
        }
    }
    out
}

/// A stylesheet for a page drawn in `theme`: every token with a color, by name, and the color
/// scheme its background calls for, so form controls and scrollbars match.
pub fn stylesheet(theme: &HashMap<String, ColorValue>) -> String {
    let mut tokens: Vec<&str> = theme.keys().map(String::as_str).collect();
    tokens.sort_unstable();
    let scheme = if is_light(theme.get("background")) { "light" } else { "dark" };
    format!(":root {{\n  color-scheme: {scheme};\n{}}}\n", custom_properties(theme, tokens))
}

/// Whether a background is light enough to want dark text, by its luminance. A color that isn't
/// RGB counts as dark, the terminal default it most likely is.
pub fn is_light(background: Option<&ColorValue>) -> bool {
    let Some(ColorValue::Rgb(red, green, blue)) = background else {
        return false;
    };
    let luminance =
        0.2126 * f32::from(*red) + 0.7152 * f32::from(*green) + 0.0722 * f32::from(*blue);
    luminance > 127.5
}

#[cfg(test)]
mod tests;
