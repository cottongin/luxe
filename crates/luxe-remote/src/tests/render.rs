//! `/api/render` as the app calls it.

use super::*;

fn render_request(bridge: &Harness, cookie: Option<&str>) -> reqwest::RequestBuilder {
    let request = reqwest::Client::new().post(bridge.url("/api/render"));
    let request = request.header("Tailscale-User-Login", OWNER);
    match cookie {
        Some(cookie) => request.header("cookie", cookie),
        None => request,
    }
}

/// The break this catches: the route left outside the device check, so the owner's tailnet login
/// alone gets the host to do work for an unpaired browser.
#[tokio::test]
async fn rendering_needs_a_paired_device() {
    let bridge = harness().await;
    let pieces = serde_json::json!({ "pieces": [{ "kind": "answer", "text": "hi" }] });
    let response = render_request(&bridge, None).json(&pieces).send().await.unwrap();
    assert_eq!(response.status(), 401);
}

/// Each piece comes back in the order it was asked for, by the bridge's renderer.
///
/// The break this catches: answers out of order, so the app puts one message's HTML under
/// another; or a renderer that doesn't escape, which the plain one a bridge starts with must.
#[tokio::test]
async fn pieces_come_back_in_order() {
    let bridge = harness().await;
    let cookie = bridge.pair().await;
    let pieces = serde_json::json!({ "pieces": [
        { "kind": "answer", "text": "**one** <i>" },
        { "kind": "tool", "name": "bash" },
        { "kind": "user", "content": [{ "type": "text", "content": "three" }] },
    ]});
    let response = render_request(&bridge, Some(&cookie)).json(&pieces).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let rendered: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        rendered,
        serde_json::json!({ "html": [
            "<p class=\"plain\">**one** &lt;i&gt;</p>",
            "",
            "<p class=\"words\">three</p>",
        ]})
    );
}

/// The break this catches: the route left at axum's 2 MiB default, so a large diff, which comes
/// back to be rendered as JSON several times its text's size, is refused and never shows.
#[tokio::test]
async fn a_large_diff_can_be_rendered() {
    let bridge = harness().await;
    let cookie = bridge.pair().await;
    let line = serde_json::json!({ "old_line": null, "new_line": 1, "kind": "addition", "text": "x".repeat(80) });
    let hunk = serde_json::json!({ "id": "hunk:a", "old_range": { "start": 0, "count": 0 },
        "new_range": { "start": 1, "count": 30000 }, "heading": null, "symbol": null,
        "lines": vec![line; 30_000], "noise": "semantic" });
    let file = serde_json::json!({ "id": "file:a", "old_path": null, "new_path": "big.txt",
        "status": "added", "language": null, "additions": 30000, "deletions": 0, "hunks": [hunk],
        "binary": false, "generated": false, "non_text": null });
    let piece = serde_json::json!({ "kind": "diff", "diff": { "path": "big.txt", "file": file, "untracked": false } });
    let body = serde_json::json!({ "pieces": [piece] });
    assert!(body.to_string().len() > 3 * 1024 * 1024, "the request outgrows the default");
    let response = render_request(&bridge, Some(&cookie)).json(&body).send().await.unwrap();
    assert_eq!(response.status(), 200);
}
