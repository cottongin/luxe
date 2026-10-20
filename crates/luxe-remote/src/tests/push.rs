//! A device turning its notifications on and off, and what reaches it once they're on.

use super::*;
use crate::push::Recording;

fn from_safari() -> serde_json::Value {
    serde_json::from_str(crate::push::FROM_SAFARI).unwrap()
}

/// A request to `/api/push` from the device with `cookie`.
fn push(bridge: &Harness, method: reqwest::Method, cookie: &str) -> reqwest::RequestBuilder {
    let request = reqwest::Client::new().request(method, bridge.url("/api/push"));
    request.header("Tailscale-User-Login", OWNER).header("cookie", cookie)
}

async fn status(bridge: &Harness, cookie: &str) -> serde_json::Value {
    let response = push(bridge, reqwest::Method::GET, cookie).send().await.unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

/// The break this catches: the push routes outside the paired-device check, so anything on this
/// machine that sets the login header can point the bridge's notifications somewhere.
#[tokio::test]
async fn only_a_paired_device_reaches_its_notifications() {
    let bridge = harness().await;
    for method in [reqwest::Method::GET, reqwest::Method::PUT, reqwest::Method::DELETE] {
        let response = push(&bridge, method.clone(), "luxe_device=nope").send().await.unwrap();
        assert_eq!(response.status(), 401, "{method}");
    }
}

/// A device gets the key, subscribes, is told it has, and turns it off again.
///
/// The break this catches: any of the three routes not reaching the device book, or the status
/// reporting another device's subscription or none at all.
#[tokio::test]
async fn a_device_turns_its_notifications_on_and_off() {
    let bridge = harness().await;
    let cookie = bridge.pair().await;
    let before = status(&bridge, &cookie).await;
    assert_eq!(before["subscribed"], false);
    let key = before["key"].as_str().unwrap();
    assert_eq!(key.len(), 87, "65 bytes, base64url without padding: {key}");

    let on = push(&bridge, reqwest::Method::PUT, &cookie).json(&from_safari()).send().await;
    assert_eq!(on.unwrap().status(), 204);
    let on = serde_json::json!({ "key": key, "subscribed": true });
    assert_eq!(status(&bridge, &cookie).await, on);
    assert_eq!(status(&bridge, &bridge.pair().await).await["subscribed"], false, "another's");

    let off = push(&bridge, reqwest::Method::DELETE, &cookie).send().await.unwrap();
    assert_eq!(off.status(), 204);
    assert_eq!(status(&bridge, &cookie).await["subscribed"], false);
}

/// The break this catches: a subscription kept without the check, so a paired device can have
/// the bridge post to a machine on the local network.
#[tokio::test]
async fn a_subscription_to_somewhere_else_is_refused() {
    let bridge = harness().await;
    let cookie = bridge.pair().await;
    let mut elsewhere = from_safari();
    elsewhere["endpoint"] = "https://192.168.1.1/push".into();
    let response = push(&bridge, reqwest::Method::PUT, &cookie).json(&elsewhere).send().await;
    let response = response.unwrap();
    assert_eq!(response.status(), 400);
    assert!(response.text().await.unwrap().contains("192.168.1.1"));
    assert_eq!(status(&bridge, &cookie).await["subscribed"], false);
}

/// Waits up to a minute (of a paused clock, where one is) for `done`.
async fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let waited = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while !done() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
    waited.await.is_ok()
}

/// The break this catches: presence not taken on join, so a phone showing a session is buzzed
/// about it, or never let go, so it's never notified about that session again.
#[tokio::test]
async fn a_device_counts_as_looking_while_its_socket_is_open() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let (socket, _joined) = bridge.join(&mut session).await;
    let id = bridge.devices.devices()[0].id.clone();
    assert!(bridge.presence.is_here(&id, SESSION));
    drop(socket);
    assert!(eventually(|| !bridge.presence.is_here(&id, SESSION)).await, "still looking");
}

const PERMISSION: &str = r#"{"kind":"permission_requested","content":{"id":"perm:1","session_id":"session-1","tool":"bash","permission":"bash","summary":"Run: rm -rf build","always_patterns":["rm *"]}}"#;

/// The bridge joins the session once a device subscribes, and posts to that device when a
/// prompt is left waiting, titled with the computer the bridge is named for.
///
/// The break this catches: the notifications never started with the bridge, or started without
/// the device book, the courier or the computer's name it was given.
#[tokio::test(start_paused = true)]
async fn a_prompt_left_waiting_reaches_a_subscribed_device() {
    let courier = std::sync::Arc::new(Recording::taking());
    let posting = courier.clone();
    let bridge = harness_with(move |bridge| bridge.notifying_through(posting).named("desk")).await;
    let mut session = fake_session(bridge.runtime.path());
    let cookie = bridge.pair().await;
    let on = push(&bridge, reqwest::Method::PUT, &cookie).json(&from_safari()).send().await;
    assert_eq!(on.unwrap().status(), 204);

    let mut watcher = session.listener.accept().await.expect("the bridge joined to watch");
    watcher.stream.write_all(format!("{PERMISSION}\n").as_bytes()).await.unwrap();
    assert!(eventually(|| !courier.sent().is_empty()).await, "nothing was posted");
    assert_eq!(courier.sent(), [from_safari()["endpoint"].as_str().unwrap()]);
    assert!(courier.titles()[0].ends_with(" \u{00b7} desk"), "{:?}", courier.titles());
}
