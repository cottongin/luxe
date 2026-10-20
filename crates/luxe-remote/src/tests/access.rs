use super::*;

/// Nothing answers a request that didn't come through Tailscale Serve, not even the start page.
///
/// The break this catches: checking the login only on session routes, so the bridge answers
/// anything on this machine that finds the port, and tells it what it is.
#[tokio::test]
async fn a_request_without_a_tailnet_login_is_refused() {
    let bridge = harness().await;
    for path in ["/", "/pair", "/api/sessions"] {
        let response = reqwest::get(bridge.url(path)).await.unwrap();
        assert_eq!(response.status(), 401, "{path} answered");
    }
}

/// A tailnet login that isn't on the allowlist is refused, though Serve vouched for it.
///
/// The break this catches: taking any login as enough, so someone you share a node or a tailnet
/// with reaches your sessions.
#[tokio::test]
async fn a_login_not_on_the_allowlist_is_refused() {
    let bridge = harness().await;
    let response = reqwest::Client::new()
        .get(bridge.url("/"))
        .header("Tailscale-User-Login", "sam@github")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}

/// A login matches in any case: Tailscale has reported the same account with its name capitalized
/// and in lowercase.
///
/// The break this catches: an exact comparison, which locks the owner out of their own bridge
/// when the header and `tailscale status` disagree on case.
#[tokio::test]
async fn a_login_matches_in_any_case() {
    let bridge = harness().await;
    let response = reqwest::Client::new()
        .get(bridge.url("/"))
        .header("Tailscale-User-Login", "Alex@GitHub")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

/// The owner's login opens the start page, and sessions only from a paired device.
///
/// The break this catches: letting the login alone reach sessions, when any process on this
/// machine can send the header straight to the loopback port.
#[tokio::test]
async fn sessions_need_a_paired_device() {
    let bridge = harness().await;
    assert_eq!(bridge.get("/").send().await.unwrap().status(), 200);
    let unpaired = bridge.get("/api/sessions").send().await.unwrap();
    assert_eq!(unpaired.status(), 401);
    let forged = bridge.get("/api/sessions").header("cookie", "luxe_device=guess").send().await;
    assert_eq!(forged.unwrap().status(), 401);
    let cookie = bridge.pair().await;
    let paired = bridge.get("/api/sessions").header("cookie", cookie).send().await.unwrap();
    assert_eq!(paired.status(), 200);
}

/// Pairing hands over a cookie no script can read, sent only over HTTPS and only by this site,
/// and the invitation it used is spent.
///
/// The break this catches: a cookie a page script can read or another site can send, or an
/// invitation that pairs again.
#[tokio::test]
async fn pairing_sets_a_guarded_cookie_and_spends_the_invitation() {
    let bridge = harness().await;
    let invitation = bridge.devices.invite().unwrap();
    let pair = || {
        reqwest::Client::new()
            .post(bridge.url("/api/pair"))
            .header("Tailscale-User-Login", OWNER)
            .json(&serde_json::json!({ "invitation": invitation, "name": "iPad" }))
            .send()
    };
    let first = pair().await.unwrap();
    let cookie = first.headers()["set-cookie"].to_str().unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict"] {
        assert!(cookie.contains(attribute), "{attribute} missing from {cookie}");
    }
    assert_eq!(pair().await.unwrap().status(), 400, "the invitation paired twice");
    let paired = bridge.devices.devices();
    assert_eq!((paired.len(), paired[0].login.as_str()), (1, OWNER));
}

/// A page on another site can't open a session socket with the device's cookie.
///
/// The break this catches: skipping the origin check on the upgrade, since a WebSocket isn't
/// held to the same-origin rule and the cookie rides along with it.
#[tokio::test]
async fn a_socket_from_another_site_is_refused() {
    let bridge = harness().await;
    let _session = fake_session(bridge.runtime.path());
    let cookie = bridge.pair().await;
    let mut request = bridge.socket_request(SESSION, &cookie);
    request.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
    let refused = tokio_tungstenite::connect_async(request).await.expect_err("cross-site");
    let tokio_tungstenite::tungstenite::Error::Http(response) = refused else {
        panic!("expected an HTTP refusal, got {refused:?}");
    };
    assert_eq!(response.status(), 403);

    let mut request = bridge.socket_request(SESSION, &cookie);
    let own_site = format!("https://{}", bridge.address);
    request.headers_mut().insert("origin", own_site.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_ok(), "its own page is refused");
}

/// Only Serve's own host passes for a page's origin; a missing one is not a page at all.
///
/// The break this catches: comparing against the wrong header, which refuses the app's own page
/// once Serve forwards the original host as `X-Forwarded-Host`.
#[test]
fn a_page_must_come_from_the_host_it_asked() {
    let headers = |pairs: &[(&'static str, &str)]| {
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, value.parse().unwrap());
        }
        headers
    };
    let ours = "https://home.tail0.ts.net";
    assert!(crate::access::same_site(&headers(&[])).is_ok());
    assert!(crate::access::same_site(&headers(&[("origin", ours), ("host", "home.tail0.ts.net")]))
        .is_ok());
    let forwarded =
        [("origin", ours), ("host", "127.0.0.1:4519"), ("x-forwarded-host", "home.tail0.ts.net")];
    assert!(crate::access::same_site(&headers(&forwarded)).is_ok());
    let elsewhere = [("origin", "https://evil.example"), ("host", "home.tail0.ts.net")];
    assert!(crate::access::same_site(&headers(&elsewhere)).is_err());
    assert!(crate::access::same_site(&headers(&[("origin", ours)])).is_err());
}
