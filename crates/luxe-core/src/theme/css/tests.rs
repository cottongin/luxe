use super::*;
use crate::theme::ThemeRegistry;

fn theme(name: &str) -> HashMap<String, ColorValue> {
    ThemeRegistry::new().resolve(name).expect("a built-in theme")
}

/// Token names arrive in the shape a stylesheet expects.
///
/// The break this catches: a changed spelling, which renames every property the website and the
/// app style themselves with, leaving them on their fallbacks.
#[test]
fn token_names_become_kebab_case() {
    assert_eq!(css_name("accentSecondary"), "accent-secondary");
    assert_eq!(css_name("background"), "background");
    assert_eq!(css_name("diffAddBg"), "diff-add-bg");
}

/// Every token with a color is published, under its CSS name, with its hex.
///
/// The break this catches: publishing only a fixed list, so a token the app styles with, such as
/// a diff or thinking color, is missing and falls back to a color from no theme.
#[test]
fn the_stylesheet_publishes_every_colored_token() {
    let luxe = theme("luxe");
    let css = stylesheet(&luxe);
    for (token, value) in &luxe {
        let Some(color) = hex(value) else { continue };
        let line = format!("--luxe-{}: {color};", css_name(token));
        assert!(css.contains(&line), "{line} is missing");
    }
    assert!(css.starts_with(":root {\n"));
}

/// A light theme asks for light form controls, and a dark one for dark.
///
/// The break this catches: a fixed scheme, so a light theme gets dark scrollbars and inputs, or
/// the other way round.
#[test]
fn the_scheme_follows_the_background() {
    let light = HashMap::from([("background".to_string(), ColorValue::Rgb(250, 248, 245))]);
    let dark = HashMap::from([("background".to_string(), ColorValue::Rgb(18, 16, 14))]);
    assert!(stylesheet(&light).contains("color-scheme: light;"));
    assert!(stylesheet(&dark).contains("color-scheme: dark;"));
}

/// A token whose name could break out of the stylesheet is left out.
///
/// The break this catches: writing a user theme's key as it is, so a name with `;` or `}` adds
/// rules of its own to the page.
#[test]
fn only_plain_token_names_are_published() {
    let theme = HashMap::from([
        ("accent".to_string(), ColorValue::Rgb(1, 2, 3)),
        ("x:1}body{display:none".to_string(), ColorValue::Rgb(4, 5, 6)),
    ]);
    let css = stylesheet(&theme);
    assert!(css.contains("--luxe-accent: #010203;"));
    assert!(!css.contains("display"), "{css}");
}
