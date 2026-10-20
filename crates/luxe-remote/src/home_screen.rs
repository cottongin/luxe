//! Home screen — what a phone needs to keep the app as an icon: a manifest, and the icon.
//!
//! Added to an iPhone's home screen, the app opens on its own, without Safari's bars, as the
//! manifest's `display: standalone` asks. Both are drawn in your theme, as `/theme.css` is, and
//! like it they need only the tailnet login: a browser fetches a manifest without cookies, so a
//! paired-device check would turn it away, and neither says anything a page of the app doesn't.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use luxe_core::theme::{ColorValue, ThemeRegistry};

use crate::render::TileColors;
use crate::routes::Bridged;

/// The sides an icon is drawn at: iOS's home screen, and the two sizes a manifest lists.
const ICON_SIDES: [u32; 3] = [180, 192, 512];

/// `/manifest.webmanifest`: the app as a home screen keeps it, named for the computer it reaches.
pub(crate) async fn manifest(State(bridged): State<Arc<Bridged>>) -> Response {
    let colors = theme_colors().await;
    let body = manifest_json(&hex(colors.plate), bridged.machine.as_deref()).to_string();
    let mut response =
        ([(header::CONTENT_TYPE, "application/manifest+json")], body).into_response();
    no_cache(&mut response);
    response
}

/// `/icons/{side}.png`: the tile, at one of [`ICON_SIDES`].
pub(crate) async fn icon(
    State(bridged): State<Arc<Bridged>>,
    Path(file): Path<String>,
) -> Response {
    let Some(side) = icon_side(&file) else {
        return (StatusCode::NOT_FOUND, format!("no icon {file}")).into_response();
    };
    let colors = theme_colors().await;
    let renderer = bridged.renderer.clone();
    let drawn = tokio::task::spawn_blocking(move || renderer.home_icon(side, colors)).await;
    let Ok(Some(png)) = drawn else {
        return (StatusCode::NOT_FOUND, "this bridge draws no icon").into_response();
    };
    let mut response = ([(header::CONTENT_TYPE, "image/png")], png).into_response();
    no_cache(&mut response);
    response
}

/// The side `file` names, when it's one this bridge draws: `180.png` and the like.
fn icon_side(file: &str) -> Option<u32> {
    let side = file.strip_suffix(".png")?.parse().ok()?;
    ICON_SIDES.contains(&side).then_some(side)
}

/// The manifest for a theme whose background is `background`, for the computer `machine`. The
/// icon already says luxe, so a named one's home screen label is the computer: an app for each of
/// two computers tells itself apart.
fn manifest_json(background: &str, machine: Option<&str>) -> serde_json::Value {
    let icon = |side: u32| {
        let (src, sizes) = (format!("/icons/{side}.png"), format!("{side}x{side}"));
        serde_json::json!({ "src": src, "sizes": sizes, "type": "image/png" })
    };
    serde_json::json!({
        "id": "/",
        "name": app_name(machine),
        "short_name": machine.unwrap_or("luxe"),
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": background,
        "theme_color": background,
        "icons": [icon(192), icon(512)],
    })
}

/// The app's name: `luxe · desk` for the computer `desk`, or plain luxe unnamed.
pub(crate) fn app_name(machine: Option<&str>) -> String {
    machine.map_or_else(|| "luxe".to_owned(), |machine| format!("luxe \u{00b7} {machine}"))
}

/// The active theme's tile colors, read off the async threads like the stylesheet.
async fn theme_colors() -> TileColors {
    let theme = tokio::task::spawn_blocking(crate::app::active_theme).await;
    tile_colors(&theme.unwrap_or_default())
}

/// The plate and letter `theme` gives the tile. A color that isn't RGB (an ANSI palette, the
/// terminal's default) can't be drawn, so it falls back to the built-in theme's.
fn tile_colors(theme: &HashMap<String, ColorValue>) -> TileColors {
    let builtin =
        ThemeRegistry::new().resolve(luxe_core::config::DEFAULT_THEME).unwrap_or_default();
    let pick = |token: &str, last: [u8; 3]| {
        rgb(theme, token).or_else(|| rgb(&builtin, token)).unwrap_or(last)
    };
    TileColors { plate: pick("background", [0; 3]), ink: pick("accent", [0xff; 3]) }
}

fn rgb(theme: &HashMap<String, ColorValue>, token: &str) -> Option<[u8; 3]> {
    match theme.get(token)? {
        ColorValue::Rgb(red, green, blue) => Some([*red, *green, *blue]),
        _ => None,
    }
}

fn hex([red, green, blue]: [u8; 3]) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// How iOS draws the status bar over the app from the home screen: see-through, with white
/// text over the app's own header, unless the theme is light enough to want dark text.
pub(crate) fn status_bar_style(theme: &HashMap<String, ColorValue>) -> &'static str {
    if luxe_core::theme::css::is_light(theme.get("background")) {
        "default"
    } else {
        "black-translucent"
    }
}

fn no_cache(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
}

#[cfg(test)]
mod tests;
