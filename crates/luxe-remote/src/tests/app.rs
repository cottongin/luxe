//! The app's files as a phone fetches them through the bridge.

use super::*;

/// The page comes with its policy, and a module with its type.
///
/// The break this catches: serving the page without the policy, or a module as plain text, which
/// a browser refuses to run, so the app is a blank page.
#[tokio::test]
async fn the_app_is_served_with_its_policy_and_types() {
    let bridge = harness().await;
    let page = bridge.get("/").send().await.unwrap();
    assert_eq!(page.status(), 200);
    let policy = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(policy.contains("'sha256-"), "the import map isn't allowed: {policy}");
    assert!(page.text().await.unwrap().contains("type=\"importmap\""));

    let module = bridge.get("/app/vendor/preact.mjs").send().await.unwrap();
    assert_eq!(module.status(), 200);
    assert_eq!(module.headers()["content-type"], "text/javascript; charset=utf-8");
}

/// The break this catches: the worker served only under `/app/`, where it would cover nothing but
/// that folder and never see a notification tapped on the app's page, or served as plain text,
/// which a browser refuses to register.
#[tokio::test]
async fn the_service_worker_is_served_from_the_top() {
    let bridge = harness().await;
    let worker = bridge.get("/service-worker.js").send().await.unwrap();
    assert_eq!(worker.status(), 200);
    assert_eq!(worker.headers()["content-type"], "text/javascript; charset=utf-8");
    assert!(worker.text().await.unwrap().contains("notificationclick"));
}

/// A file the phone already has is answered with 304, and a changed one in full.
///
/// The break this catches: ignoring `If-None-Match`, so every launch downloads the whole app over
/// the phone's connection; or answering 304 to any tag, so a phone keeps an old app.
#[tokio::test]
async fn a_file_the_phone_has_is_not_sent_again() {
    let bridge = harness().await;
    let first = bridge.get("/app/main.js").send().await.unwrap();
    let tag = first.headers()["etag"].to_str().unwrap().to_string();

    let again = bridge.get("/app/main.js").header("if-none-match", &tag).send().await.unwrap();
    assert_eq!(again.status(), 304);
    let stale = bridge.get("/app/main.js").header("if-none-match", "\"old\"").send().await.unwrap();
    assert_eq!(stale.status(), 200);
}

/// A path that climbs out of the app's folder finds nothing.
///
/// The break this catches: a debug build reading `app/` from disk resolving `..`, so a phone can
/// read the bridge's source or anything beside it.
#[tokio::test]
async fn app_paths_stay_in_the_app() {
    let bridge = harness().await;
    for path in
        ["/app/..%2Fsrc%2Flib.rs", "/app/%2e%2e/Cargo.toml", "/app/vendor/..%2F..%2FCargo.toml"]
    {
        let response = bridge.get(path).send().await.unwrap();
        assert_eq!(response.status(), 404, "{path} answered {}", response.status());
    }
}

/// The app's files need the owner's tailnet login like everything else.
///
/// The break this catches: routing the files outside the login check, which shows the app to
/// anyone who can reach the port.
#[tokio::test]
async fn the_app_needs_the_owners_login() {
    let bridge = harness().await;
    let response = reqwest::get(bridge.url("/app/main.js")).await.unwrap();
    assert_eq!(response.status(), 401);
}

/// The theme is a stylesheet of the active theme's properties, never cached as it stands.
///
/// The break this catches: a missing route or the wrong type, which the browser drops, so the app
/// draws in its fallbacks whatever theme you picked; or a cached copy, so a new theme never shows.
#[tokio::test]
async fn the_theme_is_served_as_a_stylesheet() {
    let bridge = harness().await;
    let response = bridge.get("/theme.css").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/css; charset=utf-8");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    let css = response.text().await.unwrap();
    assert!(css.starts_with(":root {") && css.contains("color-scheme:"), "{css}");
}
