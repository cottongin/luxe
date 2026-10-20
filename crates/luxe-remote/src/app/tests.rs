use super::*;

#[derive(serde::Deserialize)]
struct Vendored {
    file: String,
    version: String,
    sha256: String,
}

fn manifest() -> Vec<Vendored> {
    let manifest = AppFiles::get("vendor/manifest.json").expect("the vendor manifest");
    serde_json::from_slice(&manifest.data).expect("a readable manifest")
}

/// Every vendored file is the one its package published, and every one is listed.
///
/// The break this catches: a hand edit to a vendored file, which then ships under the upstream
/// version's name, or a file added beside them that nothing vouches for.
#[test]
fn vendored_files_match_their_manifest() {
    let listed = manifest();
    for entry in &listed {
        let file = AppFiles::get(&format!("vendor/{}", entry.file)).expect("a listed file");
        let digest = base16ct::lower::encode_string(&Sha256::digest(&file.data));
        assert_eq!(digest, entry.sha256, "{} isn't {}'s published file", entry.file, entry.version);
    }
    let modules =
        AppFiles::iter().filter(|path| path.starts_with("vendor/") && path.ends_with(".mjs"));
    for module in modules {
        let file = module.trim_start_matches("vendor/");
        assert!(listed.iter().any(|entry| entry.file == file), "{file} isn't in the manifest");
    }
}

/// Each font the stylesheet asks for is in the build, with its license beside it.
///
/// The break this catches: renaming a subset in `scripts/remote-fonts.sh` without the stylesheet,
/// so the phone quietly falls back to its own monospace face; or a font shipped without the OFL
/// text its license requires to travel with it.
#[test]
fn every_font_the_stylesheet_names_is_embedded() {
    let style = AppFiles::get("style.css").expect("the stylesheet");
    let style = String::from_utf8_lossy(&style.data);
    let named: Vec<&str> =
        style.split("url(\"").skip(1).filter_map(|rest| rest.split('"').next()).collect();
    assert!(named.len() >= 3, "the stylesheet names no fonts: {named:?}");
    for path in named {
        assert!(AppFiles::get(path).is_some(), "{path} is named but not embedded");
    }
    for license in ["fonts/OFL-JetBrainsMono.txt", "fonts/OFL-AdwaitaMono.txt"] {
        assert!(AppFiles::get(license).is_some(), "{license} is missing");
    }
}

/// The policy names the import map by the hash of exactly the bytes between its tags.
///
/// The break this catches: hashing the tags, trimmed text or the whole page, which the browser
/// doesn't match, so the import map is blocked and the app never starts. The hash is pinned from
/// `sha256 | base64` of the literal.
#[test]
fn the_policy_allows_the_import_map_by_its_hash() {
    let page = r#"<head><script type="importmap">{"imports":{"preact":"/app/vendor/preact.mjs"}}</script>
        <script type="module" src="/app/main.js"></script></head>"#;
    let policy = content_security_policy(page);
    assert!(
        policy.contains("script-src 'self' 'sha256-opQT9hhH9RSRDfDGg0J92nWa2thMilr5GvqVi+cOsyE='"),
        "{policy}"
    );
}

/// The policy keeps the page to its own origin.
///
/// The break this catches: a policy that lets a script, style or connection come from anywhere,
/// so markup a model slips into the transcript can load code that drives the session.
#[test]
fn the_policy_allows_nothing_from_elsewhere() {
    let policy = content_security_policy("");
    for directive in
        ["default-src 'none'", "style-src 'self'", "connect-src 'self'", "base-uri 'none'"]
    {
        assert!(policy.contains(directive), "{directive} is missing from {policy}");
    }
    assert!(!policy.contains("unsafe-inline") && !policy.contains('*'), "{policy}");
}

/// The page a light theme gets asks iOS for dark status bar text, and a dark one keeps its own.
///
/// The break this catches: the page's meta reworded so the swap no longer finds it, leaving
/// white status bar text over a light theme's header.
#[test]
fn the_status_bar_follows_the_theme() {
    let page = AppFiles::get("index.html").expect("the page");
    let page = String::from_utf8_lossy(&page.data);
    let meta =
        |style: &str| format!("name=\"apple-mobile-web-app-status-bar-style\" content=\"{style}\"");
    let look = |color| PageLook {
        theme: HashMap::from([("background".to_string(), color)]),
        ..PageLook::default()
    };
    let light = themed_page(&page, &look(ColorValue::Rgb(250, 250, 245)));
    assert!(light.contains(&meta("default")), "{light}");
    let dark = themed_page(&page, &look(ColorValue::Rgb(18, 16, 14)));
    assert!(dark.contains(&meta("black-translucent")), "{dark}");
}

/// The page carries the dock's mode and side from the config, and the TUI's defaults without one.
///
/// The break this catches: the page's `<html>` tag reworded so the swap no longer finds it, or a
/// config's side or mode dropped, so the phone's dock ignores where you keep the TUI's.
#[test]
fn the_page_says_where_the_dock_starts() {
    let page = AppFiles::get("index.html").expect("the page");
    let page = String::from_utf8_lossy(&page.data);
    let plain = themed_page(&page, &PageLook::default());
    assert!(plain.contains(r#"data-dock-mode="auto" data-dock-side="left""#), "{plain}");
    let dock = DockConfig { mode: Some(DockMode::Hidden), position: Some(DockPosition::Right) };
    let configured = themed_page(&page, &PageLook { dock, ..PageLook::default() });
    assert!(configured.contains(r#"data-dock-mode="hidden" data-dock-side="right""#));
    let dock = DockConfig { mode: Some(DockMode::Shown), position: None };
    let shown = themed_page(&page, &PageLook { dock, ..PageLook::default() });
    assert!(shown.contains(r#"data-dock-mode="shown""#));
}

/// The page cuts headings into slabs, as the TUI does, unless `statusline.style` is flat.
///
/// The break this catches: a flat config ignored, so the phone draws slabs the TUI doesn't, or the
/// default made flat.
#[test]
fn the_page_says_how_headings_are_cut() {
    let page = AppFiles::get("index.html").expect("the page");
    let page = String::from_utf8_lossy(&page.data);
    let plain = themed_page(&page, &PageLook::default());
    assert!(plain.contains(r#"data-headings="slab""#), "{plain}");
    let flat = PageLook { statusline: StatuslineStyle::Flat, ..PageLook::default() };
    assert!(themed_page(&page, &flat).contains(r#"data-headings="flat""#));
}

/// A named bridge's page carries the computer in its title, the name iOS suggests for the home
/// screen, and the meta the app reads; an unnamed one is the page as written.
///
/// The break this catches: the page's title or home screen meta reworded so the swap finds
/// nothing, leaving two computers' apps both "Luxe", or a name put in unescaped.
#[test]
fn the_page_is_named_for_its_computer() {
    let page = AppFiles::get("index.html").expect("the page");
    let page = String::from_utf8_lossy(&page.data);
    let named = named_page(&page, Some("desk"));
    assert!(named.contains("<title>Luxe \u{00b7} desk</title>"), "{named}");
    assert!(named.contains(r#"name="apple-mobile-web-app-title" content="desk""#));
    assert!(named.contains(r#"<meta name="luxe-machine" content="desk">"#));
    assert!(named_page(&page, Some("a\"b")).contains(r#"content="a&quot;b""#));
    assert_eq!(named_page(&page, None), page);
}
