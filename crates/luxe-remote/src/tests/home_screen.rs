//! What a phone fetches to keep the app on its home screen.

use std::sync::{Arc, Mutex};

use super::*;

/// A renderer that draws a stand-in icon and remembers the sides it was asked for.
#[derive(Default)]
struct Sketch {
    asked: Mutex<Vec<u32>>,
}

impl Renderer for Sketch {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        text.into()
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn home_icon(&self, side: u32, _colors: TileColors) -> Option<Vec<u8>> {
        self.asked.lock().unwrap().push(side);
        Some(b"\x89PNG stand-in".to_vec())
    }
}

/// A bridge named for its computer serves the page and the manifest named for it, so a phone
/// paired with two computers keeps two apps it can tell apart.
///
/// The break this catches: the name set on the bridge but never reaching the routes that read it,
/// so both computers' apps are added as "luxe".
#[tokio::test]
async fn a_named_bridge_serves_an_app_named_for_its_computer() {
    let bridge = harness_with(|bridge| bridge.named("desk")).await;
    let page = bridge.get("/").send().await.unwrap().text().await.unwrap();
    assert!(page.contains(r#"<meta name="luxe-machine" content="desk">"#), "{page}");
    let manifest = bridge.get("/manifest.webmanifest").send().await.unwrap();
    let manifest: serde_json::Value = manifest.json().await.unwrap();
    assert_eq!(manifest["short_name"], "desk");
}

/// The page links the manifest and the icon, and the manifest asks to open on its own.
///
/// The break this catches: the links or the manifest's `display` dropped, so the home screen
/// keeps a bookmark that opens in Safari with its bars, under a screenshot for an icon.
#[tokio::test]
async fn the_page_and_manifest_make_a_home_screen_app() {
    let bridge = harness().await;
    let page = bridge.get("/").send().await.unwrap().text().await.unwrap();
    assert!(page.contains("<link rel=\"manifest\" href=\"/manifest.webmanifest\">"), "{page}");
    assert!(page.contains("<link rel=\"apple-touch-icon\" href=\"/icons/180.png\">"), "{page}");

    let manifest = bridge.get("/manifest.webmanifest").send().await.unwrap();
    assert_eq!(manifest.headers()["content-type"], "application/manifest+json");
    let manifest: serde_json::Value = manifest.json().await.unwrap();
    assert_eq!(manifest["display"], "standalone");
}

/// The manifest and icon come to the tailnet login alone, without the paired-device cookie.
///
/// The break this catches: the two routed behind the device check, where a manifest fetched
/// without credentials (as browsers fetch one) is turned away and the app installs bare.
#[tokio::test]
async fn the_manifest_and_icon_need_no_paired_device() {
    let sketch = Arc::new(Sketch::default());
    let bridge = harness_rendering(sketch.clone()).await;
    assert_eq!(bridge.get("/manifest.webmanifest").send().await.unwrap().status(), 200);
    let icon = bridge.get("/icons/180.png").send().await.unwrap();
    assert_eq!(icon.status(), 200);
    assert_eq!(icon.headers()["content-type"], "image/png");
    assert_eq!(*sketch.asked.lock().unwrap(), [180]);
}

/// Only the sizes the page and manifest name are drawn.
///
/// The break this catches: any size drawn on request, so one URL asks the bridge for a bitmap of
/// gigabytes.
#[tokio::test]
async fn an_icon_is_drawn_only_at_the_sizes_offered() {
    let sketch = Arc::new(Sketch::default());
    let bridge = harness_rendering(sketch.clone()).await;
    for file in ["100000.png", "181.png", "180.svg", "180"] {
        let response = bridge.get(&format!("/icons/{file}")).send().await.unwrap();
        assert_eq!(response.status(), 404, "{file}");
    }
    assert!(sketch.asked.lock().unwrap().is_empty());
}

/// A renderer with no art answers no icon, rather than an empty image.
///
/// The break this catches: a 200 with no bytes, which iOS keeps as a blank icon.
#[tokio::test]
async fn a_renderer_without_art_has_no_icon() {
    let bridge = harness().await;
    assert_eq!(bridge.get("/icons/180.png").send().await.unwrap().status(), 404);
}
