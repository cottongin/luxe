//! App — the phone app's files, served from the binary.
//!
//! Plain HTML, CSS and ES modules from `app/`, with nothing built: what is in the repository is what
//! a phone runs. A release build embeds the folder and a debug build reads it from disk, so editing
//! the app takes a reload.
//!
//! The app shows what a model wrote, so the page carries a Content-Security-Policy that runs only
//! the app's own scripts. The import map is the one inline script, and the policy names it by hash,
//! computed from the page as served so the two can't drift apart.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use luxe_core::config::{DockConfig, DockMode, DockPosition, StatuslineStyle};
use luxe_core::theme::ColorValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use crate::routes::Bridged;

#[derive(rust_embed::Embed)]
#[folder = "app/"]
pub(crate) struct AppFiles;

/// The status bar style the page is written with; the page's own, for a dark theme.
const DARK_STATUS_BAR: &str = "content=\"black-translucent\"";

/// The page's `<html>` tag as written; the served page adds where the dock starts and how
/// headings are cut.
const HTML_TAG: &str = "<html lang=\"en\">";

/// The page's title as written, and the home screen name iOS suggests; a named bridge puts the
/// computer in both.
const PAGE_TITLE: &str = "<title>Luxe</title>";
const HOME_SCREEN_TITLE: &str = "name=\"apple-mobile-web-app-title\" content=\"Luxe\"";

/// What the page takes from your own config: a project's can't reach here.
#[derive(Default)]
struct PageLook {
    theme: HashMap<String, ColorValue>,
    dock: DockConfig,
    statusline: StatuslineStyle,
}

fn page_look() -> PageLook {
    let config = luxe_core::config::Config::load_global();
    let theme = config.resolved_theme(&luxe_core::theme::ThemeRegistry::installed());
    PageLook { theme, dock: config.dock, statusline: config.statusline.style() }
}

/// `/`: the app's page, with the status bar style your theme wants, named for the computer.
pub(crate) async fn index(State(bridged): State<Arc<Bridged>>) -> Response {
    let Some(page) = AppFiles::get("index.html") else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "the app is missing from this build")
            .into_response();
    };
    let look = tokio::task::spawn_blocking(page_look).await.unwrap_or_default();
    let page = themed_page(&String::from_utf8_lossy(&page.data), &look);
    let page = named_page(&page, bridged.machine.as_deref());
    let policy = content_security_policy(&page);
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response();
    insert(response.headers_mut(), header::CONTENT_SECURITY_POLICY, &policy);
    insert(response.headers_mut(), header::CACHE_CONTROL, "no-cache");
    response
}

/// The page as `look` wants it: the status bar style iOS reads when the app is added, the dock's
/// mode and side, which the app starts from as the TUI starts from them, and whether headings are
/// cut into slabs, which `statusline.style` decides for the TUI's.
fn themed_page(page: &str, look: &PageLook) -> String {
    let style = crate::home_screen::status_bar_style(&look.theme);
    page.replacen(DARK_STATUS_BAR, &format!("content=\"{style}\""), 1).replacen(
        HTML_TAG,
        &html_tag(look),
        1,
    )
}

/// The page named for the computer `machine`: its title, the name iOS suggests when it's added to
/// the home screen, and a `luxe-machine` meta the app reads to say which computer it reaches.
/// Unnamed, it stays plain Luxe.
fn named_page(page: &str, machine: Option<&str>) -> String {
    let Some(machine) = machine else {
        return page.to_owned();
    };
    let name = crate::render::escape(machine);
    let title = crate::render::escape(&crate::home_screen::app_name(Some(machine)));
    page.replacen(
        PAGE_TITLE,
        &format!("<title>{title}</title>\n<meta name=\"luxe-machine\" content=\"{name}\">"),
        1,
    )
    .replacen(
        HOME_SCREEN_TITLE,
        &format!("name=\"apple-mobile-web-app-title\" content=\"{name}\""),
        1,
    )
}

fn html_tag(look: &PageLook) -> String {
    let (mode, side) = dock_attributes(&look.dock);
    let headings = match look.statusline {
        StatuslineStyle::Flat => "flat",
        StatuslineStyle::Slab | StatuslineStyle::Unknown => "slab",
    };
    format!(
        "<html lang=\"en\" data-dock-mode=\"{mode}\" data-dock-side=\"{side}\" \
         data-headings=\"{headings}\">"
    )
}

fn dock_attributes(dock: &DockConfig) -> (&'static str, &'static str) {
    let mode = match dock.mode() {
        DockMode::Shown => "shown",
        DockMode::Hidden => "hidden",
        DockMode::Auto | DockMode::Unknown => "auto",
    };
    let side = match dock.position() {
        DockPosition::Right => "right",
        DockPosition::Left | DockPosition::Unknown => "left",
    };
    (mode, side)
}

/// `/app/{*path}`: one of the app's files, answered with 304 when the phone already has it.
pub(crate) async fn file(Path(path): Path<String>, request: HeaderMap) -> Response {
    let Some(file) = AppFiles::get(&path) else {
        return (StatusCode::NOT_FOUND, format!("no app file {path}")).into_response();
    };
    let tag = format!("\"{}\"", base16ct::lower::encode_string(&file.metadata.sha256_hash()));
    let cached =
        request.get(header::IF_NONE_MATCH).is_some_and(|sent| sent.as_bytes() == tag.as_bytes());
    let mut response =
        if cached { StatusCode::NOT_MODIFIED.into_response() } else { file.data.into_response() };
    let headers = response.headers_mut();
    insert(headers, header::CONTENT_TYPE, content_type(&path));
    insert(headers, header::ETAG, &tag);
    // revalidate on every load, which a 304 makes cheap, so a phone never runs a stale app
    insert(headers, header::CACHE_CONTROL, "no-cache");
    insert(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    response
}

/// `/service-worker.js`: the app's service worker, served from the top so it covers the whole app
/// (a worker reaches only as far as the folder it's served from).
pub(crate) async fn service_worker(request: HeaderMap) -> Response {
    file(Path("worker/service-worker.js".into()), request).await
}

/// `/theme.css`: the active theme's colors, read on each load so the phone follows a new theme.
pub(crate) async fn theme() -> Response {
    let css = tokio::task::spawn_blocking(active_theme_css).await.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not read the theme");
        String::new()
    });
    let mut response = ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css).into_response();
    insert(response.headers_mut(), header::CACHE_CONTROL, "no-cache");
    response
}

fn active_theme_css() -> String {
    luxe_core::theme::css::stylesheet(&active_theme())
}

/// The theme the terminal would be drawn in from your own config: a project's can't reach here.
pub(crate) fn active_theme() -> HashMap<String, ColorValue> {
    let registry = luxe_core::theme::ThemeRegistry::installed();
    luxe_core::config::Config::load_global().resolved_theme(&registry)
}

fn insert(headers: &mut HeaderMap, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

/// The type a file is served as, by its extension. Only what `app/` holds.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "text/plain; charset=utf-8",
    }
}

/// The page's policy: the app's own files, its import map by hash, and nothing from elsewhere.
pub(crate) fn content_security_policy(page: &str) -> String {
    let import_map = inline_script(page, "<script type=\"importmap\">").unwrap_or_default();
    let digest = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(import_map));
    format!(
        "default-src 'none'; script-src 'self' 'sha256-{digest}'; style-src 'self'; \
         img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; manifest-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    )
}

/// The text of the inline script that opens with `opening`, exactly as the browser hashes it.
fn inline_script<'a>(page: &'a str, opening: &str) -> Option<&'a str> {
    let start = page.find(opening)? + opening.len();
    let length = page[start..].find("</script>")?;
    Some(&page[start..start + length])
}

#[cfg(test)]
mod tests;
