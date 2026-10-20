use super::*;

#[test]
fn parse_hex_full() {
    assert_eq!(ColorValue::parse("#2e3440").unwrap(), ColorValue::Rgb(46, 52, 64));
}

#[test]
fn parse_hex_short() {
    assert_eq!(ColorValue::parse("#abc").unwrap(), ColorValue::Rgb(170, 187, 204));
}

#[test]
fn parse_ansi() {
    assert_eq!(ColorValue::parse("242").unwrap(), ColorValue::Ansi(242));
}

#[test]
fn parse_empty_is_default() {
    assert_eq!(ColorValue::parse("").unwrap(), ColorValue::Default);
    assert_eq!(ColorValue::parse("  ").unwrap(), ColorValue::Default);
}

#[test]
fn parse_invalid_hex() {
    assert!(ColorValue::parse("#xyz").is_err());
    assert!(ColorValue::parse("#12345").is_err());
    assert!(ColorValue::parse("#1234567").is_err());
}

#[test]
fn parse_unknown_format() {
    assert!(ColorValue::parse("not_a_color").is_err());
}

#[test]
fn resolve_alias_var() {
    let mut vars = HashMap::new();
    vars.insert("base00".to_string(), "#2e3440".to_string());
    let colors = HashMap::new();
    assert_eq!(
        ColorValue::resolve_alias("base00", &vars, &colors).unwrap(),
        ColorValue::Rgb(46, 52, 64)
    );
}

#[test]
fn resolve_alias_token() {
    let vars = HashMap::new();
    let mut colors = HashMap::new();
    colors.insert("accent".to_string(), "#2e3440".to_string());
    colors.insert("border".to_string(), "accent".to_string());
    assert_eq!(
        ColorValue::resolve_alias("border", &vars, &colors).unwrap(),
        ColorValue::Rgb(46, 52, 64)
    );
}

#[test]
fn resolve_alias_not_found() {
    let vars = HashMap::new();
    let colors = HashMap::new();
    assert!(ColorValue::resolve_alias("ghost", &vars, &colors).is_err());
}

/// A theme whose aliases loop is an error for those tokens, and the rest of it still resolves.
///
/// The break this catches: alias resolution recursed on the raw value, so `"text": "text"` or a
/// two-token loop overflowed the stack and aborted the process on every launch.
#[test]
fn an_alias_cycle_is_an_error_not_a_crash() {
    let colors: HashMap<String, String> =
        [("text", "text"), ("a", "b"), ("b", "a"), ("accent", "#2e3440"), ("border", "accent")]
            .into_iter()
            .map(|(token, raw)| (token.to_string(), raw.to_string()))
            .collect();
    let vars = HashMap::new();
    for token in ["text", "a", "b"] {
        let resolved = ColorValue::resolve_alias(token, &vars, &colors);
        assert!(matches!(resolved, Err(ThemeError::AliasCycle(ref name)) if name == token));
    }
    let theme = Theme { name: "loops".into(), vars, colors };
    let resolved = theme.resolve_aliases();
    assert_eq!(resolved.get("border"), Some(&ColorValue::Rgb(46, 52, 64)));
    assert!(!resolved.contains_key("text") && !resolved.contains_key("a"));
}

/// An alias through a var that does not parse is that alias not found, as it always was; the
/// var's own token reports the parse error.
#[test]
fn a_bad_var_behind_an_alias_is_the_alias_not_found() {
    let vars: HashMap<String, String> = [("base".to_string(), "nope".to_string())].into();
    let colors: HashMap<String, String> = [("border".to_string(), "base".to_string())].into();
    let behind = ColorValue::resolve_alias("border", &vars, &colors);
    assert!(matches!(behind, Err(ThemeError::AliasNotFound(ref name)) if name == "border"));
    let direct = ColorValue::resolve_alias("base", &vars, &colors);
    assert!(matches!(direct, Err(ThemeError::UnknownFormat(_))));
}

#[test]
fn theme_load_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nord.json");
    std::fs::write(
        &path,
        r##"{
                "name": "nord",
                "vars": {},
                "colors": {
                    "accent": "#5e81ac",
                    "text": "#d8dee9",
                    "border": ""
                }
            }"##,
    )
    .unwrap();

    let theme = Theme::load_file(&path).unwrap();
    assert_eq!(theme.name, "nord");
    assert_eq!(theme.colors.len(), 3);
}

#[test]
fn theme_merged_over_fills_missing() {
    let base = Theme {
        name: "base".to_string(),
        vars: HashMap::new(),
        colors: [("accent".to_string(), "#000000".to_string())].into_iter().collect(),
    };
    let override_theme = Theme {
        name: "override".to_string(),
        vars: HashMap::new(),
        colors: [("text".to_string(), "#ffffff".to_string())].into_iter().collect(),
    };

    let merged = override_theme.merged_over(&base);
    assert_eq!(merged.colors.len(), 2);
    assert_eq!(merged.colors.get("accent").unwrap(), &"#000000".to_string());
    assert_eq!(merged.colors.get("text").unwrap(), &"#ffffff".to_string());
}

#[test]
fn theme_merged_over_override_wins() {
    let base = Theme {
        name: "base".to_string(),
        vars: HashMap::new(),
        colors: [("accent".to_string(), "#000000".to_string())].into_iter().collect(),
    };
    let override_theme = Theme {
        name: "override".to_string(),
        vars: HashMap::new(),
        colors: [("accent".to_string(), "#ffffff".to_string())].into_iter().collect(),
    };

    let merged = override_theme.merged_over(&base);
    assert_eq!(merged.colors.get("accent").unwrap(), &"#ffffff".to_string());
}

#[test]
fn schema_is_valid_json() {
    let schema: serde_json::Value = serde_json::from_str(Theme::schema_json()).unwrap();
    assert_eq!(schema["title"], "luxe theme");
}

/// The full set of shipped built-in theme ids.
const EXPECTED_BUILTINS: &[&str] = &[
    "luxe",
    "nord",
    "dracula",
    "gruvbox",
    "tokyo-night",
    "kanagawa",
    "rose-pine",
    "catppuccin",
    "everforest",
    "github",
    "one-dark",
    "ayu",
    "monokai",
    "aura",
    "zenburn",
    "jetbrains",
    "noctis-bordo",
    "panda",
    "vesper",
    "vscode",
    "xcode",
    "ethereal",
    "hackerman",
    "last-horizon",
    "lumon",
    "matte-black",
    "miasma",
    "osaka-jade",
    "retro-82",
    "ristretto",
    "solitude",
    "vantablack",
];

#[test]
fn builtin_themes_parse() {
    let themes = builtin_themes();
    assert_eq!(themes.len(), EXPECTED_BUILTINS.len());
    let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
    for expected in EXPECTED_BUILTINS {
        assert!(names.contains(expected), "missing built-in theme: {expected}");
    }
}

#[test]
fn builtin_theme_names_listed() {
    let names = builtin_theme_names();
    assert_eq!(names.len(), EXPECTED_BUILTINS.len());
    for expected in EXPECTED_BUILTINS {
        assert!(names.contains(&expected.to_string()), "missing: {expected}");
    }
}

#[test]
fn registry_loads_builtins() {
    let reg = ThemeRegistry::new();
    assert_eq!(reg.names().len(), EXPECTED_BUILTINS.len());
    for expected in EXPECTED_BUILTINS {
        assert!(reg.names().contains(&expected.to_string()), "missing: {expected}");
    }
}

/// A verdict has to read as one in every theme: an error is not the color of a success, and a
/// deleted line is not washed like an added one.
#[test]
fn every_builtin_keeps_its_verdicts_apart() {
    let reg = ThemeRegistry::new();
    for name in EXPECTED_BUILTINS {
        let resolved = reg.resolve(name).unwrap_or_else(|| panic!("{name} must resolve"));
        assert_ne!(resolved.get("error"), resolved.get("success"), "{name}");
        assert_ne!(resolved.get("error"), resolved.get("warning"), "{name}");
        assert_ne!(resolved.get("diffAddBg"), resolved.get("diffDelBg"), "{name}");
    }
}

#[test]
fn every_builtin_covers_all_tokens_and_resolves() {
    let reg = ThemeRegistry::new();
    for name in EXPECTED_BUILTINS {
        let resolved = reg.resolve(name).unwrap_or_else(|| panic!("{name} must resolve"));
        assert_eq!(resolved.len(), Theme::ALL_TOKENS.len(), "{name} does not cover every token");
        // Accent must be a real RGB color, not a dropped/empty alias.
        assert!(
            matches!(resolved.get("accent"), Some(ColorValue::Rgb(_, _, _))),
            "{name} accent did not resolve to an RGB color"
        );
    }
}

/// WCAG relative luminance of a resolved token, or `None` when the terminal owns the color.
fn luminance(value: Option<&ColorValue>) -> Option<f32> {
    let ColorValue::Rgb(red, green, blue) = value? else { return None };
    let channel = |raw: &u8| {
        let raw = f32::from(*raw) / 255.0;
        if raw <= 0.04045 {
            raw / 12.92
        } else {
            ((raw + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue))
}

/// Contrast ratio between two resolved tokens, per WCAG 2.
fn contrast(fore: Option<&ColorValue>, back: Option<&ColorValue>) -> Option<f32> {
    let (fore, back) = (luminance(fore)?, luminance(back)?);
    let (lighter, darker) = if fore > back { (fore, back) } else { (back, fore) };
    Some((lighter + 0.05) / (darker + 0.05))
}

/// luxe's own themes hold a contrast floor, so a later tweak to a hue cannot quietly cost
/// legibility.
///
/// Only `luxe` is held to it: the other built-ins are faithful ports of palettes
/// people chose on purpose, and silently "fixing" someone else's Gruvbox would be the wrong
/// call. The floors are WCAG AA for text (4.5:1), with body text held far above it.
///
/// Checked against **every surface a theme paints**, not just the canvas. Text lands on raised
/// panels and selected rows too, and a token that clears the floor on the background can fail
/// on a surface two steps lighter — which is exactly what the website's audit caught first.
#[test]
fn luxe_themes_hold_their_contrast_floor() {
    const ACCENTS: &[&str] = &["accent", "accentSecondary", "info", "success", "error", "warning"];
    const SURFACES: &[&str] = &["background", "userMsgBg", "selectedBg"];
    let reg = ThemeRegistry::new();

    {
        let name = "luxe";
        let resolved = reg.resolve(name).unwrap_or_else(|| panic!("{name} must resolve"));
        for surface in SURFACES {
            let against = |token: &str| {
                contrast(resolved.get(token), resolved.get(*surface))
                    .unwrap_or_else(|| panic!("{name}.{token} must be an RGB color"))
            };
            let floor = |token: &str, minimum: f32| {
                let ratio = against(token);
                assert!(
                    ratio >= minimum,
                    "{name}.{token} on {surface} is {ratio:.2}:1, under {minimum}:1"
                );
            };

            floor("text", 7.0);
            floor("muted", 4.5);
            for token in ACCENTS {
                floor(token, 4.5);
            }
        }
    }
}

/// The semantic accents sit on one lightness plateau, so no single token shouts louder than its
/// neighbors — the thing that makes a palette read as designed rather than assembled.
#[test]
fn luxe_accents_share_a_lightness_plateau() {
    const ACCENTS: &[&str] = &["accent", "accentSecondary", "info", "success", "error", "warning"];
    let reg = ThemeRegistry::new();

    {
        let name = "luxe";
        let resolved = reg.resolve(name).unwrap_or_else(|| panic!("{name} must resolve"));
        let ratios: Vec<f32> = ACCENTS
            .iter()
            .map(|token| {
                contrast(resolved.get(*token), resolved.get("background"))
                    .unwrap_or_else(|| panic!("{name}.{token} must be an RGB color"))
            })
            .collect();
        let (low, high) = ratios
            .iter()
            .fold((f32::MAX, 0.0f32), |(low, high), ratio| (low.min(*ratio), high.max(*ratio)));
        assert!(
            high / low <= 1.7,
            "{name}: accents span {low:.2}:1 to {high:.2}:1 — one of them is shouting"
        );
    }
}

/// The map a derived theme is built with is the one the built-ins spell out, token for token.
#[test]
fn the_standard_token_map_is_the_built_ins_map() {
    let luxe: Theme = serde_json::from_str(BUILTIN_THEMES[0].1).expect("luxe");
    let mut built_in: Vec<(String, String)> =
        luxe.colors.into_iter().filter(|(token, _)| !token.starts_with("diff")).collect();
    built_in.sort();
    let mut standard: Vec<(String, String)> = STANDARD_COLORS
        .iter()
        .map(|(token, var)| ((*token).to_owned(), (*var).to_owned()))
        .collect();
    standard.sort();
    assert_eq!(standard, built_in);
    let complete = Theme::ALL_TOKENS.iter().filter(|token| !token.starts_with("diff"));
    assert_eq!(complete.count(), STANDARD_COLORS.len(), "and it covers every token");
}

#[test]
fn registry_get_returns_theme() {
    let reg = ThemeRegistry::new();
    let mocha = reg.get("catppuccin").expect("catppuccin must exist");
    assert_eq!(mocha.name, "catppuccin");
    assert!(!mocha.colors.is_empty());
}

/// Every name a built-in used to go by still works, and none is offered: `dark` keeps the colors
/// it always had, a retired light theme becomes the dark one it was paired with, and a picker
/// lists each theme once.
#[test]
fn a_retired_name_resolves_to_the_theme_it_became() {
    let reg = ThemeRegistry::new();
    for (old, now) in [
        ("dark", "catppuccin"),
        ("light", "catppuccin"),
        ("catppuccin-latte", "catppuccin"),
        ("luxe-light", "luxe"),
    ] {
        assert_eq!(reg.canonical(old), Some(now));
        assert_eq!(reg.get(old).map(|theme| theme.name.as_str()), Some(now));
        assert_eq!(reg.resolve(old), reg.resolve(now), "{old} resolves as {now}");
        assert!(!reg.names().contains(&old.to_string()), "{old} is not offered");
    }
    // Mocha's muted is the dimmer Overlay0, which is what made comments read as comments
    let mocha = reg.resolve("catppuccin").expect("catppuccin");
    assert_eq!(mocha.get("muted"), Some(&ColorValue::Rgb(0x6c, 0x70, 0x86)));
    assert_eq!(reg.canonical("no-such-theme"), None);
}

/// A theme file of a retired name is somebody's own theme, and wins over the alias.
#[test]
fn a_theme_file_outranks_a_retired_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("dark.json"), r##"{"name":"dark","vars":{},"colors":{}}"##)
        .expect("theme file");
    let reg = ThemeRegistry::with_themes_in(dir.path());
    assert_eq!(reg.canonical("dark"), Some("dark"));
    assert_eq!(reg.get("dark").map(|theme| theme.name.as_str()), Some("dark"));
}

#[test]
fn registry_resolve_returns_resolved_colors() {
    let reg = ThemeRegistry::new();
    let resolved = reg.resolve("catppuccin").expect("catppuccin must resolve");
    assert_eq!(resolved.len(), Theme::ALL_TOKENS.len());
    assert!(matches!(resolved.get("accent"), Some(ColorValue::Rgb(_, _, _))));
}

#[test]
fn nord_uses_the_reference_accent_palette() {
    let resolved = ThemeRegistry::new().resolve("nord").expect("nord theme must resolve");
    assert_eq!(resolved.len(), Theme::ALL_TOKENS.len());
    assert_eq!(resolved.get("accent"), Some(&ColorValue::Rgb(0x81, 0xa1, 0xc1)));
    assert_eq!(resolved.get("info"), Some(&ColorValue::Rgb(0x88, 0xc0, 0xd0)));
    assert_eq!(resolved.get("warning"), Some(&ColorValue::Rgb(0xeb, 0xcb, 0x8b)));
    assert_eq!(resolved.get("success"), Some(&ColorValue::Rgb(0xa3, 0xbe, 0x8c)));
    assert_eq!(resolved.get("accentSecondary"), Some(&ColorValue::Rgb(0xb4, 0x8e, 0xad)));
    assert_eq!(resolved.get("error"), Some(&ColorValue::Rgb(0xbf, 0x61, 0x6a)));
}

#[test]
fn registry_names_sorted() {
    let reg = ThemeRegistry::new();
    let names = reg.names();
    // Should be sorted alphabetically for stable picker order.
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // Should contain the built-in themes.
    assert!(names.contains(&"catppuccin".to_string()));
    assert!(!names.iter().any(|name| name.contains("light") || name.contains("latte")));
}

/// A theme in the user's directory is offered under its file name, laid over the default so a
/// file that changes one color is a whole theme, and a broken file is reported rather than
/// taking the good ones with it.
///
/// Nothing read that directory: the guide's `~/.config/luxe/themes/<name>.json` fell back to
/// the default with a warning only the log carried.
#[test]
fn the_themes_directory_is_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hotpink.json"),
        r##"{ "name": "ignored", "colors": { "accent": "#ff00aa" } }"##,
    )
    .unwrap();
    std::fs::write(dir.path().join("broken.json"), "{ not json").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a theme").unwrap();

    let registry = ThemeRegistry::with_themes_in(dir.path());
    let resolved = registry.resolve("hotpink").expect("the file's name is the theme's name");
    assert_eq!(resolved.get("accent"), Some(&ColorValue::Rgb(0xff, 0x00, 0xaa)));
    let default = registry.resolve(crate::config::DEFAULT_THEME).unwrap();
    assert_eq!(
        resolved.get("text"),
        default.get("text"),
        "what it leaves out comes from the default"
    );
    assert!(registry.get("broken").is_none());
    assert_eq!(registry.problems().len(), 1, "{:?}", registry.problems());
    assert!(registry.problems()[0].contains("broken.json"), "{:?}", registry.problems());
    assert!(registry.get("luxe").is_some(), "the built-ins are all still there");
}

#[test]
fn registry_load_custom() {
    use std::io::Write;
    let dir = std::env::temp_dir().join("luxe_test_themes");
    let _ = std::fs::create_dir_all(&dir);
    let theme_file = dir.join("test.json");
    let mut f = std::fs::File::create(&theme_file).unwrap();
    writeln!(f, "{{\"name\":\"test\",\"vars\":{{}},\"colors\":{{\"accent\":\"#ff0000\"}}}}")
        .unwrap();
    drop(f);

    let mut reg = ThemeRegistry::new();
    assert!(reg.load_custom(&theme_file).is_ok());
    assert!(reg.names().contains(&"test".to_string()));
    let resolved = reg.resolve("test").expect("custom theme must resolve");
    assert_eq!(resolved.get("accent"), Some(&ColorValue::Rgb(255, 0, 0)));

    let _ = std::fs::remove_dir_all(&dir);
}
