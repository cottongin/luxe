use super::*;

fn theme(pairs: &[(&str, ColorValue)]) -> HashMap<String, ColorValue> {
    pairs.iter().map(|(token, color)| (token.to_string(), color.clone())).collect()
}

/// The break this catches: a renamed field or a lost icon in the manifest, which iOS reads once,
/// when the app is added, and never asks about again.
#[test]
fn the_manifest_is_pinned() {
    let expected = serde_json::json!({
        "id": "/",
        "name": "Luxe",
        "short_name": "Luxe",
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": "#12100e",
        "theme_color": "#12100e",
        "icons": [
            { "src": "/icons/192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "/icons/512.png", "sizes": "512x512", "type": "image/png" },
        ],
    });
    assert_eq!(manifest_json("#12100e", None), expected);
}

/// A named bridge's app is named for its computer, and the home screen label is the computer
/// alone, since the icon already says Luxe.
///
/// The break this catches: two computers' apps both suggested as "Luxe", so the phone shows two
/// identical icons, or the full name crowding the home screen's short label.
#[test]
fn a_named_bridge_names_its_app_for_the_computer() {
    let named = manifest_json("#12100e", Some("desk"));
    assert_eq!(
        (named["name"].as_str(), named["short_name"].as_str()),
        (Some("Luxe \u{00b7} desk"), Some("desk"))
    );
}

/// The break this catches: the theme ignored, so the icon is the default brass whatever you
/// picked; or plate and letter crossed.
#[test]
fn the_tile_takes_the_themes_background_and_accent() {
    let rgb =
        theme(&[("background", ColorValue::Rgb(1, 2, 3)), ("accent", ColorValue::Rgb(4, 5, 6))]);
    assert_eq!(tile_colors(&rgb), TileColors { plate: [1, 2, 3], ink: [4, 5, 6] });
}

/// The break this catches: an ANSI theme drawn in the last-resort black and white, or only one
/// of the two colors falling back, rather than the built-in theme's.
#[test]
fn a_color_that_isnt_rgb_falls_back_to_the_built_in_theme() {
    let builtin = tile_colors(&HashMap::new());
    let ansi = theme(&[("background", ColorValue::Ansi(0)), ("accent", ColorValue::Rgb(4, 5, 6))]);
    assert_eq!(tile_colors(&ansi), TileColors { plate: builtin.plate, ink: [4, 5, 6] });
    assert_ne!(builtin.plate, [0; 3], "the built-in theme's own plate, not the last resort");
}

/// The break this catches: the status bar's style the same for every theme, so a light one gets
/// white text on its own light header.
#[test]
fn a_light_theme_keeps_the_status_bar_dark_text() {
    let light = theme(&[("background", ColorValue::Rgb(250, 250, 245))]);
    let dark = theme(&[("background", ColorValue::Rgb(18, 16, 14))]);
    assert_eq!(status_bar_style(&light), "default");
    assert_eq!(status_bar_style(&dark), "black-translucent");
}
