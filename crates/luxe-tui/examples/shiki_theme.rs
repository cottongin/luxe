//! shiki_theme — luxe's palette as a syntax theme for the website's code blocks.
//!
//! The docs describe a terminal program; highlighting their code in somebody else's palette is a
//! small lie about what luxe looks like. This writes a TextMate theme from the same JSON the TUI
//! loads, so the code on the page is colored by the product's own tokens.
//!
//! It is also the palette's hardest test. A theme with six accents can hide a weak one behind a
//! statusline; a syntax theme puts all of them on screen at once, adjacent, in small type.
//!
//! The mapping is the TUI's own, [`luxe_tui::highlight::SCOPES`], so a code block on the page and
//! one in the terminal are colored the same way; why each role takes its token is said there.
//!
//! ```sh
//! cargo run -p luxe-tui --example shiki_theme -- luxe > web/src/assets/generated/shiki-luxe.json
//! ```

use luxe_core::theme::{ColorValue, ThemeRegistry};
use ratatui::style::Modifier;
use std::collections::HashMap;

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "luxe".into());
    let registry = ThemeRegistry::new();
    let resolved =
        registry.resolve(&name).unwrap_or_else(|| panic!("{name} is not a built-in theme"));

    println!("{}", theme_json(&name, &resolved));
}

/// The TextMate document Shiki loads.
fn theme_json(name: &str, theme: &HashMap<String, ColorValue>) -> String {
    let hex = |token: &str| -> String {
        match theme.get(token) {
            Some(ColorValue::Rgb(red, green, blue)) => format!("#{red:02x}{green:02x}{blue:02x}"),
            // Every token this maps is RGB in luxe's own themes; anything else would be a theme
            // this generator was not written for, and inventing a color would hide that.
            _ => panic!("{name}.{token} is not an RGB color"),
        }
    };

    let mut rules = String::new();
    for (selectors, role) in luxe_tui::highlight::SCOPES {
        let (token, modifier) = role.token();
        let list = selectors
            .split(',')
            .map(|scope| format!("\"{}\"", scope.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let font = [(Modifier::BOLD, "bold"), (Modifier::ITALIC, "italic")]
            .iter()
            .filter(|(flag, _)| modifier.contains(*flag))
            .map(|(_, word)| *word)
            .collect::<Vec<_>>()
            .join(" ");
        let font = match font.is_empty() {
            true => String::new(),
            false => format!(", \"fontStyle\": \"{font}\""),
        };
        rules.push_str(&format!(
            "    {{ \"scope\": [{list}], \"settings\": {{ \"foreground\": \"{}\"{font} }} }},\n",
            hex(token)
        ));
    }
    let rules = rules.trim_end().trim_end_matches(',');

    format!(
        // `settings` rather than `tokenColors`: both are accepted, but the former is the raw
        // TextMate shape Shiki's own type is written against, so the config typechecks without a cast.
        "{{\n  \"name\": \"{name}\",\n  \"type\": \"{}\",\n  \"colors\": {{\n    \
         \"editor.background\": \"{}\",\n    \"editor.foreground\": \"{}\"\n  }},\n  \
         \"settings\": [\n{rules}\n  ]\n}}",
        if name.contains("light") { "light" } else { "dark" },
        hex("background"),
        hex("text"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(name: &str) -> HashMap<String, ColorValue> {
        ThemeRegistry::new().resolve(name).expect("a built-in")
    }

    /// The document parses, and says which mode it is for — Shiki picks the CSS variable prefix
    /// from that, so a light theme labeled dark inverts the whole page.
    #[test]
    fn the_theme_is_valid_json_of_the_right_mode() {
        {
            let (name, mode) = ("luxe", "dark");
            let parsed: serde_json::Value =
                serde_json::from_str(&theme_json(name, &theme(name))).expect("valid json");
            assert_eq!(parsed["name"], name);
            assert_eq!(parsed["type"], mode);
            assert!(parsed["settings"].as_array().is_some_and(|rules| rules.len() > 5));
        }
    }

    /// Every color on a code block clears the same floor the rest of the palette does. Syntax
    /// highlighting is the one surface that puts six accents together in small type, so a token
    /// that only just passes elsewhere fails here first.
    #[test]
    fn every_highlight_color_is_readable_on_the_code_background() {
        {
            let name = "luxe";
            let resolved = theme(name);
            let parsed: serde_json::Value =
                serde_json::from_str(&theme_json(name, &resolved)).expect("valid json");
            let background =
                parsed["colors"]["editor.background"].as_str().expect("a background").to_string();

            for rule in parsed["settings"].as_array().expect("rules") {
                let color = rule["settings"]["foreground"].as_str().expect("a color");
                let ratio = contrast(color, &background);
                assert!(ratio >= 4.5, "{name}: {color} is {ratio:.2}:1 on {background}");
            }
        }
    }

    /// `dim` is a rule color, not a text color, and putting it on punctuation is the classic way
    /// a syntax theme becomes unreadable.
    #[test]
    fn the_theme_never_reaches_for_dim() {
        {
            let name = "luxe";
            let resolved = theme(name);
            let ColorValue::Rgb(red, green, blue) = resolved["dim"] else { panic!("dim is rgb") };
            let dim = format!("#{red:02x}{green:02x}{blue:02x}");
            assert!(!theme_json(name, &resolved).contains(&dim), "{name} highlights with dim");
        }
    }

    fn luminance(hex: &str) -> f32 {
        let channel = |raw: u8| {
            let value = f32::from(raw) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex");
        0.2126 * channel(byte(1)) + 0.7152 * channel(byte(3)) + 0.0722 * channel(byte(5))
    }

    fn contrast(fore: &str, back: &str) -> f32 {
        let (a, b) = (luminance(fore), luminance(back));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}
