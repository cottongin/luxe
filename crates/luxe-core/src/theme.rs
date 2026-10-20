//! Theme system — semantic color tokens for the TUI.
//!
//! Themes define a set of named tokens (accent, border, success, error, md_heading, …)
//! that widgets read from instead of hardcoding colors. This keeps every widget decoupled
//! from the active palette and makes live switching trivial.
//!
//! Color values may be hex (`#rrggbb`), ANSI index (`242`), or empty (terminal default).
//! Themes can reference other tokens via `vars` + alias resolution (resolved at load).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub mod css;

/// A parsed color value: RGB, ANSI-indexed, or terminal default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorValue {
    Rgb(u8, u8, u8),
    Ansi(u8),
    Default,
}

impl ColorValue {
    /// Parse a color string: hex (`#rrggbb`), decimal ANSI index (`0-255`), or empty.
    ///
    /// Empty string → `Default`. Garbage returns `Err`.
    pub fn parse(s: &str) -> Result<Self, ThemeError> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(ColorValue::Default);
        }
        if let Some(hex) = s.strip_prefix('#') {
            match hex.len() {
                6 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).map_err(ThemeError::Hex)?;
                    let g = u8::from_str_radix(&hex[2..4], 16).map_err(ThemeError::Hex)?;
                    let b = u8::from_str_radix(&hex[4..6], 16).map_err(ThemeError::Hex)?;
                    Ok(ColorValue::Rgb(r, g, b))
                }
                3 => {
                    let r = u8::from_str_radix(&hex[0..1], 16).map_err(ThemeError::Hex)?;
                    let g = u8::from_str_radix(&hex[1..2], 16).map_err(ThemeError::Hex)?;
                    let b = u8::from_str_radix(&hex[2..3], 16).map_err(ThemeError::Hex)?;
                    // Expand short hex: f → ff
                    let expand = |v: u8| v * 0x11;
                    Ok(ColorValue::Rgb(expand(r), expand(g), expand(b)))
                }
                _ => Err(ThemeError::InvalidHex(hex.to_string())),
            }
        } else if let Ok(idx) = s.parse::<u8>() {
            Ok(ColorValue::Ansi(idx))
        } else {
            Err(ThemeError::UnknownFormat(s.to_string()))
        }
    }

    /// Resolve an alias: look up `name` in the vars/colors map, returning the resolved value.
    ///
    /// A color may name another token, followed until something parses. A chain longer than the
    /// colors map has visited a token twice, so it is [`ThemeError::AliasCycle`] rather than a
    /// stack overflow. A name in neither map, or a var behind an alias that does not parse, is
    /// [`ThemeError::AliasNotFound`].
    pub fn resolve_alias<'a>(
        name: &str,
        vars: &'a HashMap<String, String>,
        colors: &'a HashMap<String, String>,
    ) -> Result<ColorValue, ThemeError> {
        let not_found = || ThemeError::AliasNotFound(name.to_string());
        let mut current = name;
        for hop in 0..=colors.len() {
            if let Some(raw) = vars.get(current) {
                return ColorValue::parse(raw).map_err(|error| match hop {
                    0 => error,
                    _ => not_found(),
                });
            }
            let raw = colors.get(current).ok_or_else(not_found)?;
            if let Ok(value) = ColorValue::parse(raw) {
                return Ok(value);
            }
            current = raw;
        }
        Err(ThemeError::AliasCycle(name.to_string()))
    }
}

/// Where the user's own themes live: `themes/` in luxe's config directory.
pub fn user_themes_dir() -> std::path::PathBuf {
    crate::user_dirs::luxe_config_dir().join("themes")
}

/// Theme error kinds.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("invalid hex color: {0}")]
    InvalidHex(String),
    #[error("hex parse error: {0}")]
    Hex(std::num::ParseIntError),
    #[error("unknown color format: {0}")]
    UnknownFormat(String),
    #[error("alias not found: {0}")]
    AliasNotFound(String),
    #[error("alias loops back on itself: {0}")]
    AliasCycle(String),
    #[error("JSON parse error: {0}")]
    Json(serde_json::Error),
    #[error("theme file read error: {0}")]
    Io(std::io::Error),
}

crate::error_cause::from_cause!(ThemeError {
    std::num::ParseIntError => Hex,
    serde_json::Error => Json,
    std::io::Error => Io,
});

/// The built-in themes, embedded. luxe's own leads: it is what a first launch resolves to.
///
/// The rest are community favorites, and Omarchy's dark themes, so a desktop running one gets the
/// hand-checked version rather than one derived from its palette on the fly (see [`crate::omarchy`]).
/// Those came out of that same derivation and were then read over, which is why they agree with
/// what a session following the desktop would have drawn.
const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("luxe", include_str!("themes/luxe.json")),
    ("nord", include_str!("themes/nord.json")),
    ("dracula", include_str!("themes/dracula.json")),
    ("gruvbox", include_str!("themes/gruvbox.json")),
    ("tokyo-night", include_str!("themes/tokyo-night.json")),
    ("kanagawa", include_str!("themes/kanagawa.json")),
    ("rose-pine", include_str!("themes/rose-pine.json")),
    ("catppuccin", include_str!("themes/catppuccin.json")),
    ("everforest", include_str!("themes/everforest.json")),
    ("github", include_str!("themes/github.json")),
    ("one-dark", include_str!("themes/one-dark.json")),
    ("ayu", include_str!("themes/ayu.json")),
    ("monokai", include_str!("themes/monokai.json")),
    ("aura", include_str!("themes/aura.json")),
    ("zenburn", include_str!("themes/zenburn.json")),
    ("jetbrains", include_str!("themes/jetbrains.json")),
    ("noctis-bordo", include_str!("themes/noctis-bordo.json")),
    ("panda", include_str!("themes/panda.json")),
    ("vesper", include_str!("themes/vesper.json")),
    ("vscode", include_str!("themes/vscode.json")),
    ("xcode", include_str!("themes/xcode.json")),
    ("ethereal", include_str!("themes/ethereal.json")),
    ("hackerman", include_str!("themes/hackerman.json")),
    ("last-horizon", include_str!("themes/last-horizon.json")),
    ("lumon", include_str!("themes/lumon.json")),
    ("matte-black", include_str!("themes/matte-black.json")),
    ("miasma", include_str!("themes/miasma.json")),
    ("osaka-jade", include_str!("themes/osaka-jade.json")),
    ("retro-82", include_str!("themes/retro-82.json")),
    ("ristretto", include_str!("themes/ristretto.json")),
    ("solitude", include_str!("themes/solitude.json")),
    ("vantablack", include_str!("themes/vantablack.json")),
];

/// Built-in theme registry. Every entry must cover `Theme::ALL_TOKENS` (asserted in tests).
pub fn builtin_themes() -> Vec<Theme> {
    // A malformed embedded theme is a compile-time asset bug, not runtime input — panicking
    // here is the intended "this can't happen in a shipped binary" invariant.
    BUILTIN_THEMES
        .iter()
        .map(|(name, json)| {
            serde_json::from_str::<Theme>(json)
                .unwrap_or_else(|e| panic!("{name} built-in theme must parse: {e}"))
        })
        .collect()
}

/// Get all built-in theme names.
pub fn builtin_theme_names() -> Vec<String> {
    builtin_themes().into_iter().map(|t| t.name).collect()
}

/// A complete theme: a set of named color tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    /// Theme name (e.g. "nord", "catppuccin-mocha"). A file on disk is named for the file instead.
    #[serde(default)]
    pub name: String,
    /// Named base colors available for aliasing (raw hex/ANSI strings).
    ///
    /// Optional, as the published schema has always said — requiring it rejected a hand-written
    /// theme that only set colors.
    #[serde(default)]
    pub vars: HashMap<String, String>,
    /// Token → raw color string (hex, ANSI, or alias).
    pub colors: HashMap<String, String>,
}

impl Theme {
    /// All tokens that a complete theme should define.
    pub const ALL_TOKENS: &'static [&'static str] = &[
        "accent",
        "accentSecondary",
        "info",
        "border",
        "borderAccent",
        "borderMuted",
        "success",
        "error",
        "warning",
        "muted",
        "dim",
        "text",
        "thinkingText",
        "thinkingOff",
        "thinkingMinimal",
        "thinkingLow",
        "thinkingMedium",
        "thinkingHigh",
        "thinkingXHigh",
        "thinkingUltra",
        "pasteIndicatorBg",
        "selectedBg",
        "diffAddBg",
        "diffDelBg",
        "background",
        "userMsgBg",
        "toolCallBg",
        "toolBg",
        "toolTitle",
        "toolOutput",
        "mdHeading",
        "mdLink",
        "mdLinkUrl",
        "mdCode",
        "mdCodeBlock",
        "mdCodeBlockBorder",
        "mdQuote",
    ];

    /// A complete theme from the thirteen palette vars every built-in is drawn from.
    ///
    /// The vars are `bg`, `bgRaised`, `bgHigh`, `fg`, `muted`, `dim`, `accent`, `accentSecondary`,
    /// `magenta`, `info`, `success`, `error` and `warning`; every token takes one of them by the
    /// same map the built-ins use, so a derived theme and a hand-written one agree on what each
    /// token is for. The diff washes are not vars and are left for the caller to set.
    pub fn from_vars<'a>(name: &str, vars: impl IntoIterator<Item = (&'a str, String)>) -> Self {
        Theme {
            name: name.to_owned(),
            vars: vars.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
            colors: STANDARD_COLORS
                .iter()
                .map(|(token, var)| ((*token).to_owned(), (*var).to_owned()))
                .collect(),
        }
    }

    /// Load a theme from a JSON file.
    pub fn load_file(path: &Path) -> Result<Self, ThemeError> {
        let contents = std::fs::read_to_string(path)?;
        let mut theme: Theme = serde_json::from_str(&contents)?;
        theme.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("custom").to_string();
        Ok(theme)
    }

    /// Resolve aliases in `colors` using `vars`.
    ///
    /// After resolution, `resolved_colors` maps each token to a `ColorValue`.
    pub fn resolve_aliases(&self) -> HashMap<String, ColorValue> {
        let mut resolved = HashMap::new();
        for (token, raw) in &self.colors {
            if let Ok(value) = ColorValue::parse(raw) {
                resolved.insert(token.clone(), value);
            } else {
                match ColorValue::resolve_alias(raw, &self.vars, &self.colors) {
                    Ok(value) => {
                        resolved.insert(token.clone(), value);
                    }
                    Err(error) => tracing::warn!(token, raw, %error, "unresolved color; skipping"),
                }
            }
        }
        resolved
    }

    /// Merge this theme over a base theme: missing tokens — and the vars they alias — fall back to
    /// `base`.
    pub fn merged_over(&self, base: &Theme) -> Self {
        let mut merged_colors = base.colors.clone();
        merged_colors.extend(self.colors.clone());
        let mut merged_vars = base.vars.clone();
        merged_vars.extend(self.vars.clone());
        Theme { name: self.name.clone(), vars: merged_vars, colors: merged_colors }
    }

    /// JSON Schema for `Theme` (for `$schema` in custom theme files).
    pub fn schema_json() -> &'static str {
        r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "luxe theme",
  "type": "object",
  "required": ["name", "colors"],
  "properties": {
    "name": { "type": "string" },
    "vars": {
      "type": "object",
      "description": "Named base colors available for aliasing in the colors map."
    },
    "colors": {
      "type": "object",
      "description": "Token → color value (hex #rrggbb, ANSI index 0-255, or alias to a var/token). Empty string = terminal default."
    }
  }
}"#
    }
}

/// Which palette var each token takes, as every built-in's `colors` map spells it out.
const STANDARD_COLORS: &[(&str, &str)] = &[
    ("accent", "accent"),
    ("accentSecondary", "accentSecondary"),
    ("info", "info"),
    ("border", "bgRaised"),
    ("borderAccent", "accent"),
    ("borderMuted", "bgHigh"),
    ("success", "success"),
    ("error", "error"),
    ("warning", "warning"),
    ("muted", "muted"),
    ("dim", "dim"),
    ("text", "fg"),
    ("thinkingText", "muted"),
    ("thinkingOff", "muted"),
    ("thinkingMinimal", "fg"),
    ("thinkingLow", "warning"),
    ("thinkingMedium", "success"),
    ("thinkingHigh", "accent"),
    ("thinkingXHigh", "accentSecondary"),
    ("thinkingUltra", "magenta"),
    ("pasteIndicatorBg", "bgHigh"),
    ("selectedBg", "bgHigh"),
    ("background", "bg"),
    ("userMsgBg", "bgRaised"),
    ("toolCallBg", "bgRaised"),
    ("toolBg", "bg"),
    ("toolTitle", "accent"),
    ("toolOutput", "fg"),
    ("mdHeading", "accent"),
    ("mdLink", "info"),
    ("mdLinkUrl", "muted"),
    ("mdCode", "warning"),
    ("mdCodeBlock", "bg"),
    ("mdCodeBlockBorder", "bgRaised"),
    ("mdQuote", "muted"),
];

/// Names a built-in used to go by, and the theme each one is now.
///
/// `dark` was Catppuccin Mocha under a generic name, so it is `catppuccin`. luxe is drawn for a
/// dark canvas, so a retired light theme maps to the dark one it was the counterpart of (a light
/// theme of your own is one file in `themes/`). A config that still names one gets that theme
/// rather than a warning and the default, and a theme file of any of these names is somebody's real
/// theme and wins. `xcode` is not here: it is a built-in, Xcode's Default Dark.
const RETIRED_NAMES: &[(&str, &str)] = &[
    ("dark", "catppuccin"),
    ("light", "catppuccin"),
    ("catppuccin-latte", "catppuccin"),
    ("luxe-light", "luxe"),
];

/// Theme registry: built-in themes + custom themes loaded from disk.
pub struct ThemeRegistry {
    themes: HashMap<String, Theme>,
    /// Custom theme files that could not be loaded, each with its reason.
    problems: Vec<String>,
}

impl ThemeRegistry {
    /// Create a new registry with all built-in themes loaded.
    pub fn new() -> Self {
        let mut themes = HashMap::new();
        for theme in builtin_themes() {
            themes.insert(theme.name.clone(), theme);
        }
        Self { themes, problems: Vec::new() }
    }

    /// The built-ins plus every theme in the user's `themes/` directory — what a session offers.
    ///
    /// [`ThemeRegistry::new`] stays built-ins only, for the generated site assets and the tests,
    /// which must not change with whatever happens to be in the machine's config. This reads a
    /// directory and the desktop's theme files, so a front-end calls it once and again only where
    /// a theme may have changed.
    ///
    /// On Omarchy it also carries the desktop's theme, as [`crate::omarchy::DESKTOP_THEME`].
    pub fn installed() -> Self {
        Self::with_themes_in(&user_themes_dir())
            .with_desktop(crate::omarchy::DesktopTheme::current().as_ref())
    }

    /// Adds the desktop's theme under [`crate::omarchy::DESKTOP_THEME`], when there is one.
    ///
    /// A theme file of that name is the reader's own and wins, like any other.
    pub fn with_desktop(mut self, desktop: Option<&crate::omarchy::DesktopTheme>) -> Self {
        let name = crate::omarchy::DESKTOP_THEME;
        if self.themes.contains_key(name) {
            return self;
        }
        if let Some(theme) = desktop.and_then(|desktop| desktop.theme(&self)) {
            self.themes.insert(name.to_owned(), theme);
        }
        self
    }

    /// The built-ins plus every `*.json` in `dir`, each named for its file.
    ///
    /// A file only has to carry what it changes: it is laid over the default theme, so the tokens
    /// it leaves out — and the vars they alias — come from there. One that cannot be read or
    /// parsed is recorded in [`ThemeRegistry::problems`] rather than taking the others with it.
    pub fn with_themes_in(dir: &Path) -> Self {
        let mut registry = Self::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return registry };
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        let base = registry.themes.get(crate::config::DEFAULT_THEME).cloned();
        for path in paths {
            match Theme::load_file(&path) {
                Ok(theme) => {
                    let theme = match &base {
                        Some(base) => theme.merged_over(base),
                        None => theme,
                    };
                    registry.themes.insert(theme.name.clone(), theme);
                }
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "ignoring an unreadable theme");
                    registry.problems.push(format!("{}: {error}", crate::paths::display(&path)));
                }
            }
        }
        registry
    }

    /// Custom theme files that failed to load, as `path: reason` — for telling the reader, since a
    /// theme that silently falls back looks like a theme that is not very different.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Load a custom theme from a JSON file and add it to the registry.
    pub fn load_custom(&mut self, path: &std::path::Path) -> Result<(), ThemeError> {
        let theme = Theme::load_file(path)?;
        self.themes.insert(theme.name.clone(), theme);
        Ok(())
    }

    /// Get a theme by name, or by a name a built-in used to go by.
    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.canonical(name).and_then(|name| self.themes.get(name))
    }

    /// The name `name` is registered under: itself, or what a retired built-in name became.
    pub fn canonical<'a>(&self, name: &'a str) -> Option<&'a str> {
        if self.themes.contains_key(name) {
            return Some(name);
        }
        RETIRED_NAMES.iter().find(|(old, _)| *old == name).map(|(_, now)| *now)
    }

    /// Get all theme names, sorted alphabetically for a stable picker order.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.themes.keys().cloned().collect();
        names.sort();
        names
    }

    /// Resolve all aliases in a theme and return the resolved color map.
    pub fn resolve(&self, name: &str) -> Option<HashMap<String, ColorValue>> {
        self.get(name).map(Theme::resolve_aliases)
    }
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
