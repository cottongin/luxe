//! omarchy — the desktop's theme, read where Omarchy keeps it and turned into one of luxe's.
//!
//! [Omarchy](https://omarchy.org) themes a whole desktop from one file, `colors.toml`, and every app
//! it themes gets its colors from there. luxe follows it three ways, most specific first:
//!
//! - A `luxe.json` in the applied theme — shipped by the theme, or rendered by a
//!   `~/.config/omarchy/themed/luxe.json.tpl` template — is used as it is. That is the theme
//!   author's own answer, so nothing here second-guesses it.
//! - A built-in theme of the same name, when it is the same kind of theme: Omarchy's `rose-pine` is
//!   Rosé Pine Dawn, which is light, and luxe's is the dark main variant, so a shared name is not
//!   enough on its own.
//! - Otherwise the palette itself, derived from `colors.toml` with the same fallbacks Omarchy's own
//!   `omarchy-theme-color` applies, so a theme that only sets a few keys comes out whole.
//!
//! Where it looks moved in Omarchy 4: the applied theme is state now, under
//! `~/.local/state/omarchy/current`, rather than configuration under `~/.config/omarchy/current`.
//! Both are read, newest first, so either release is followed.
//!
//! Only reads files. Nothing here runs Omarchy's scripts: parsing a key/value file is cheaper than a
//! process, and a theme picker should never depend on a shell being there.

use crate::theme::{Theme, ThemeRegistry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The name the desktop's theme is registered under, and the value that asks for it in config.
pub const DESKTOP_THEME: &str = "omarchy";

/// The theme Omarchy has applied: its name, and the directory holding its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopTheme {
    /// The theme's slug, as Omarchy names it (`tokyo-night`, `rose-pine`)
    pub name: String,
    /// The applied copy of the theme, holding `colors.toml` and whatever it or a template added
    pub dir: PathBuf,
}

impl DesktopTheme {
    /// The theme Omarchy has applied on this machine, if it is running Omarchy.
    pub fn current() -> Option<Self> {
        Self::current_in(&crate::user_dirs::state_dir(), &crate::user_dirs::config_dir())
    }

    /// [`DesktopTheme::current`], with the state and config directories given.
    ///
    /// Omarchy 4 keeps the applied theme under `state/omarchy/current`; earlier releases kept it
    /// under `config/omarchy/current`. The first with a `theme.name` wins.
    pub fn current_in(state: &Path, config: &Path) -> Option<Self> {
        [state, config].into_iter().find_map(|base| {
            let current = base.join("omarchy").join("current");
            let name = std::fs::read_to_string(current.join("theme.name")).ok()?;
            let name = name.trim().to_lowercase();
            (!name.is_empty()).then(|| Self { name, dir: current.join("theme") })
        })
    }

    /// A change stamp for the applied theme: its name and when its palette last moved.
    ///
    /// Cheap enough to take every couple of seconds, which is how a running session notices the
    /// desktop being re-themed under it.
    pub fn stamp(&self) -> (String, Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
        let modified = |file: &str| {
            std::fs::metadata(self.dir.join(file)).and_then(|meta| meta.modified()).ok()
        };
        (self.name.clone(), modified("colors.toml"), modified("luxe.json"))
    }

    /// The luxe theme for this desktop theme, by the rules in the module docs.
    ///
    /// `builtins` decides the second rule, so a built-in is matched by its registered name only —
    /// not by a retired alias, or a light Omarchy theme could land on a dark one.
    pub fn theme(&self, builtins: &ThemeRegistry) -> Option<Theme> {
        if let Ok(theme) = Theme::load_file(&self.dir.join("luxe.json")) {
            let theme = match builtins.get(crate::config::DEFAULT_THEME) {
                Some(base) => theme.merged_over(base),
                None => theme,
            };
            return Some(Theme { name: DESKTOP_THEME.to_owned(), ..theme });
        }
        let palette = std::fs::read_to_string(self.dir.join("colors.toml"))
            .ok()
            .map(|text| Palette::parse(&text, self.dir.join("light.mode").exists()));
        // Every built-in is drawn for a dark canvas, so only a dark desktop can share one
        let dark = palette.as_ref().is_none_or(|palette| !palette.light);
        if dark && builtins.names().contains(&self.name) {
            if let Some(theme) = builtins.get(&self.name) {
                return Some(Theme { name: DESKTOP_THEME.to_owned(), ..theme.clone() });
            }
        }
        palette.map(|palette| palette.theme(DESKTOP_THEME))
    }
}

/// An Omarchy `colors.toml`, resolved the way `omarchy-theme-color` resolves it.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    colors: HashMap<String, Rgb>,
    /// Whether the theme is light: its `mode`, a `light.mode` file, or a bright background
    pub light: bool,
}

/// One color, as `(red, green, blue)`.
type Rgb = (u8, u8, u8);

impl Palette {
    /// Reads a `colors.toml`. `light_file` says whether a `light.mode` file sits beside it, which is
    /// how themes older than the `mode` key said they were light.
    ///
    /// Line by line, the way Omarchy reads it, rather than as TOML: a theme that Omarchy accepts
    /// must not be refused here because it is not quite TOML. Values that are not `#rrggbb` —
    /// gradients, `rgba()` borders — are not colors this needs, and are left out.
    pub fn parse(text: &str, light_file: bool) -> Self {
        let mut raw: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let key: String = key.chars().filter(|c| !matches!(c, '"' | '\'' | ' ')).collect();
            if key.is_empty() || key.starts_with('#') {
                continue;
            }
            let value = match value.find(['"', '\'']) {
                Some(open) => {
                    let rest = &value[open + 1..];
                    rest.find(['"', '\'']).map_or(rest, |close| &rest[..close])
                }
                // Unquoted is only trimmed, as Omarchy does: the `#` of a hex is not a comment
                None => value,
            };
            raw.insert(key, value.trim().to_owned());
        }
        Self::resolve(raw, light_file)
    }

    /// The fallback cascade, in the order `omarchy-theme-color` applies it.
    fn resolve(raw: HashMap<String, String>, light_file: bool) -> Self {
        let mut colors: HashMap<String, Rgb> =
            raw.iter().filter_map(|(key, value)| Some((key.clone(), hex(value)?))).collect();
        let mut alias = |key: &str, from: &str| {
            if !colors.contains_key(key) {
                if let Some(value) = colors.get(from).copied() {
                    colors.insert(key.to_owned(), value);
                }
            }
        };
        for (key, short) in [
            ("background", "bg"),
            ("foreground", "fg"),
            ("lighter_background", "lighter_bg"),
            ("dark_foreground", "dark_fg"),
            ("bright_foreground", "bright_fg"),
        ] {
            alias(key, short);
        }
        alias("background", "color0");
        alias("foreground", "color7");
        for (key, ansi) in [
            ("red", "color1"),
            ("green", "color2"),
            ("yellow", "color3"),
            ("blue", "color4"),
            ("magenta", "color5"),
            ("cyan", "color6"),
            ("bright_magenta", "color13"),
        ] {
            alias(key, ansi);
        }
        alias("magenta", "purple");
        alias("bright_magenta", "bright_purple");
        alias("muted", "color8");
        alias("dark_foreground", "muted");
        alias("orange", "yellow");
        // Omarchy's own derivation for a bright shade a theme left out: a fifth of the way to white
        if !colors.contains_key("bright_magenta") {
            if let Some(magenta) = colors.get("magenta").copied() {
                colors.insert("bright_magenta".into(), mix(magenta, (255, 255, 255), 0.2));
            }
        }

        let mode =
            raw.get("mode").or_else(|| raw.get("theme_type")).map(|mode| mode.to_lowercase());
        let light = match mode.as_deref() {
            Some("light") => true,
            Some("dark") => false,
            _ if light_file => true,
            // Omarchy's own test: the channels summed past half of white
            _ => colors
                .get("background")
                .is_some_and(|(r, g, b)| u16::from(*r) + u16::from(*g) + u16::from(*b) > 382),
        };
        Self { colors, light }
    }

    fn get(&self, key: &str) -> Option<Rgb> {
        self.colors.get(key).copied()
    }

    /// A complete luxe theme from this palette, called `name`.
    ///
    /// Omarchy names colors by hue and luxe by role, so this is the mapping between them: its
    /// `accent` stays the accent, red is an error, green a success, orange (or yellow) a warning,
    /// cyan information and magenta the second accent. The surfaces luxe needs and Omarchy does not
    /// name are mixed from the background toward the text, which keeps them in the theme's own hue.
    ///
    /// Every color that is read as text is held to 4.5:1 on the background, lifted toward the
    /// foreground until it is: a palette tuned for a wallpaper or a border can carry an accent too
    /// dark to read a word in, and a theme that is merely faithful and unreadable helps nobody.
    ///
    /// Errors, successes and warnings are signals, so they are chosen by hue rather than by name.
    /// Plenty of themes use the ANSI names as slots — a green-on-green theme's `red` is green, and
    /// one theme's `yellow` is its red — and a failed tool card tinted like a passing one says the
    /// opposite of what happened. Each takes the palette's own color of the right hue wherever the
    /// theme put it, and a standard one only when the theme has none.
    pub fn theme(&self, name: &str) -> Theme {
        let (bg, fg) = match (self.get("background"), self.get("foreground")) {
            (Some(bg), Some(fg)) => (bg, fg),
            (Some(bg), None) => {
                (bg, if self.light { (0x20, 0x20, 0x20) } else { (0xe0, 0xe0, 0xe0) })
            }
            (None, Some(fg)) => {
                (if self.light { (0xfa, 0xfa, 0xfa) } else { (0x16, 0x16, 0x16) }, fg)
            }
            (None, None) => ((0x16, 0x16, 0x16), (0xe0, 0xe0, 0xe0)),
        };
        let text = |key: &str, fallback: Rgb| readable(self.get(key).unwrap_or(fallback), bg, fg);
        let raised = self
            .get("lighter_background")
            .filter(|surface| *surface != bg && contrast(*surface, bg) < 1.6)
            .unwrap_or_else(|| mix(bg, fg, 0.07));
        let muted = self.get("dark_foreground").unwrap_or_else(|| mix(bg, fg, 0.6));
        let accent = self.get("accent").or_else(|| self.get("blue")).unwrap_or(fg);
        let green = self.signal("green", Signal::Success);
        let red = self.signal("red", Signal::Error);
        let warning = self.signal("orange", Signal::Warning);
        let vars = [
            ("bg", bg),
            ("bgRaised", raised),
            ("bgHigh", mix(bg, fg, 0.14)),
            ("fg", fg),
            ("muted", readable(muted, bg, fg)),
            ("dim", mix(bg, fg, 0.25)),
            ("accent", readable(accent, bg, fg)),
            ("accentSecondary", text("magenta", accent)),
            ("magenta", text("bright_magenta", accent)),
            ("info", text("cyan", accent)),
            ("success", readable(green, bg, fg)),
            ("error", readable(red, bg, fg)),
            ("warning", readable(warning, bg, fg)),
        ];
        let mut theme = Theme::from_vars(name, vars.map(|(key, color)| (key, to_hex(color))));
        // A wash of the hue, deep enough to tell apart and shallow enough to read code on
        let wash = if self.light { 0.16 } else { 0.2 };
        theme.colors.insert("diffAddBg".into(), to_hex(mix(bg, green, wash)));
        theme.colors.insert("diffDelBg".into(), to_hex(mix(bg, red, wash)));
        theme
    }
}

/// A color a reader takes as a verdict rather than as decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Error,
    Warning,
    Success,
}

impl Signal {
    /// Whether a hue, in degrees, reads as this signal.
    fn fits(self, hue: f32) -> bool {
        match self {
            Self::Error => !(25.0..335.0).contains(&hue),
            Self::Warning => (25.0..=65.0).contains(&hue),
            Self::Success => (75.0..=165.0).contains(&hue),
        }
    }

    /// The color to use when a palette has nothing of this hue.
    fn standard(self) -> Rgb {
        match self {
            Self::Error => (0xe5, 0x53, 0x4b),
            Self::Warning => (0xd4, 0xa7, 0x2c),
            Self::Success => (0x57, 0xab, 0x5a),
        }
    }
}

/// Where a palette might keep a color, after the key a role asks for first.
const HUED_KEYS: &[&str] = &[
    "red",
    "orange",
    "yellow",
    "green",
    "cyan",
    "blue",
    "magenta",
    "bright_red",
    "bright_yellow",
    "bright_green",
    "bright_cyan",
    "bright_blue",
    "bright_magenta",
    "accent",
];

impl Palette {
    /// The palette's color for `signal`: `key` when its hue fits, else the first key that does,
    /// else the standard one.
    fn signal(&self, key: &str, signal: Signal) -> Rgb {
        std::iter::once(key)
            .chain(HUED_KEYS.iter().copied())
            .filter_map(|key| self.get(key))
            .find(|color| {
                let (hue, saturation) = hue_saturation(*color);
                saturation >= 0.25 && signal.fits(hue)
            })
            .unwrap_or_else(|| signal.standard())
    }
}

/// HSL hue in degrees and saturation, 0 to 1.
fn hue_saturation((r, g, b): Rgb) -> (f32, f32) {
    let [r, g, b] = [r, g, b].map(|channel| f32::from(channel) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let lightness = (max + min) / 2.0;
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (hue, saturation)
}

/// `#rrggbb`, or `None` for anything else a `colors.toml` carries.
fn hex(value: &str) -> Option<Rgb> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

fn to_hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `from` moved `amount` of the way to `to`, rounded the way Omarchy's `mix_color` rounds.
fn mix(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let channel = |a: u8, b: u8| {
        (f32::from(a) * (1.0 - amount) + f32::from(b) * amount + 0.5).clamp(0.0, 255.0) as u8
    };
    (channel(from.0, to.0), channel(from.1, to.1), channel(from.2, to.2))
}

/// WCAG relative luminance.
fn luminance((r, g, b): Rgb) -> f32 {
    let linear = |channel: u8| {
        let value = f32::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG contrast ratio, 1 to 21.
fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The floor every text color is held to.
const TEXT_CONTRAST: f32 = 4.5;

/// `color`, or the least lift toward `fg` that makes it readable on `bg`.
fn readable(color: Rgb, bg: Rgb, fg: Rgb) -> Rgb {
    (0..=20)
        .map(|step| mix(color, fg, step as f32 / 20.0))
        .find(|candidate| contrast(*candidate, bg) >= TEXT_CONTRAST)
        .unwrap_or(fg)
}

#[cfg(test)]
mod tests;
