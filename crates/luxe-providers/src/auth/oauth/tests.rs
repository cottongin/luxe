#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn pasted_input_is_accepted_in_every_shape_a_provider_produces() {
    let full = parse_pasted_code("http://localhost:1455/auth/callback?code=abc&state=xyz")
        .expect("full URL");
    assert_eq!((full.code.as_str(), full.state.as_deref()), ("abc", Some("xyz")));

    let query = parse_pasted_code("code=abc&state=xyz").expect("bare query");
    assert_eq!((query.code.as_str(), query.state.as_deref()), ("abc", Some("xyz")));

    let hashed = parse_pasted_code("abc#xyz").expect("code#state");
    assert_eq!((hashed.code.as_str(), hashed.state.as_deref()), ("abc", Some("xyz")));

    let bare = parse_pasted_code("  abc  ").expect("bare code");
    assert_eq!((bare.code.as_str(), bare.state), ("abc", None));

    assert!(parse_pasted_code("   ").is_none());
}

#[tokio::test]
async fn the_loopback_server_receives_a_real_redirect() {
    let mut server = CallbackServer::bind(0, "/auth/callback", Some("state-1".into()))
        .await
        .expect("binds an ephemeral port");
    let uri = server.redirect_uri();
    assert!(uri.starts_with("http://localhost:"), "providers register localhost: {uri}");

    let url = format!("{uri}?code=the-code&state=state-1");
    let request = tokio::spawn(async move { crate::http::shared_client().get(&url).send().await });

    let callback = server.wait().await.expect("delivered").expect("accepted");
    assert_eq!(callback.code, "the-code");
    let response = request.await.expect("join").expect("http ok");
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap_or_default().contains("Signed in"));
}

/// Any page the browser has open can make it request `localhost`, so a forged error or another
/// sign-in's answer must not end this one; and a connection that says nothing must not hold the
/// real redirect behind it.
#[tokio::test]
async fn only_this_sign_ins_answer_ends_it() {
    let mut server =
        CallbackServer::bind(0, "/auth/callback", Some("state-1".into())).await.unwrap();
    let uri = server.redirect_uri();
    let port = reqwest::Url::parse(&uri).unwrap().port().unwrap();
    let _silent = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let browser = tokio::spawn(async move {
        let client = crate::http::shared_client();
        for forged in ["?error=access_denied", "?code=evil&state=other"] {
            let response = client.get(format!("{uri}{forged}")).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 400, "{forged}");
        }
        client.get(format!("{uri}?code=the-code&state=state-1")).send().await.unwrap()
    });
    let callback = tokio::time::timeout(std::time::Duration::from_secs(10), server.wait())
        .await
        .expect("not stalled")
        .expect("delivered")
        .expect("accepted");
    assert_eq!(callback.code, "the-code");
    assert!(browser.await.unwrap().status().is_success());
}

/// The provider's own words are repeated on the page, escaped.
#[tokio::test]
async fn a_providers_error_is_escaped_on_the_page() {
    let mut server = CallbackServer::bind(0, "/cb", None).await.unwrap();
    let url = format!("{}?error=x&error_description=%3Cb%3Eno%3C/b%3E", server.redirect_uri());
    let request = tokio::spawn(async move { crate::http::shared_client().get(&url).send().await });
    let refused = server.wait().await.expect("delivered").expect_err("refused");
    assert_eq!(refused, "<b>no</b>");
    let page = request.await.unwrap().unwrap().text().await.unwrap();
    assert!(page.contains("&lt;b&gt;no&lt;/b&gt;") && !page.contains("<b>no"), "{page}");
}

#[tokio::test]
async fn the_poller_stops_on_success_and_honors_slow_down() {
    tokio::time::pause();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = attempts.clone();
    let pacing = DevicePacing { interval_secs: 1, expires_in_secs: 900, wait_first: false };
    let value = poll_device_code(pacing, || {
        let counter = counter.clone();
        async move {
            let seen = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match seen {
                0 => Poll::Pending,
                1 => Poll::SlowDown(Some(2)),
                _ => Poll::Done("token"),
            }
        }
    })
    .await
    .expect("authorized");
    assert_eq!(value, "token");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn the_poller_gives_up_when_the_code_expires() {
    tokio::time::pause();
    let pacing = DevicePacing { interval_secs: 5, expires_in_secs: 10, wait_first: true };
    let error = poll_device_code(pacing, || async { Poll::Pending::<()> })
        .await
        .expect_err("must time out");
    assert!(matches!(error, AuthError::Timeout(_)), "got {error:?}");
    assert!(error.to_string().contains("expired"));
}

#[tokio::test]
async fn a_slow_down_timeout_names_the_usual_culprit() {
    tokio::time::pause();
    let pacing = DevicePacing { interval_secs: 5, expires_in_secs: 12, wait_first: false };
    let error = poll_device_code(pacing, || async { Poll::SlowDown::<()>(None) })
        .await
        .expect_err("must time out");
    assert!(error.to_string().contains("clock"), "unhelpful: {error}");
}

#[tokio::test]
async fn a_terminal_poll_failure_surfaces_the_providers_message() {
    let pacing = DevicePacing { interval_secs: 1, expires_in_secs: 60, wait_first: false };
    let error = poll_device_code(pacing, || async { Poll::Failed::<()>("access_denied".into()) })
        .await
        .expect_err("must fail");
    assert!(matches!(error, AuthError::Rejected(message) if message == "access_denied"));
}

#[test]
fn jwt_payloads_decode_without_verification() {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"sub":"u1","https://api.openai.com/auth":{"chatgpt_account_id":"acct"}}"#);
    let token = format!("header.{payload}.signature");
    let claims = decode_jwt_payload(&token).expect("decodes");
    assert_eq!(claims["https://api.openai.com/auth"]["chatgpt_account_id"], "acct");
    assert!(decode_jwt_payload("not-a-jwt").is_none());
    assert!(decode_jwt_payload("a.!!!.c").is_none());
}

#[test]
fn error_bodies_yield_the_most_useful_message() {
    let described = serde_json::json!({"error": "invalid_grant", "error_description": "expired"});
    assert_eq!(provider_error_message(&described).as_deref(), Some("expired"));
    assert_eq!(provider_error_code(&described).as_deref(), Some("invalid_grant"));

    let nested = serde_json::json!({"error": {"code": "slow_down", "message": "back off"}});
    assert_eq!(provider_error_message(&nested).as_deref(), Some("back off"));
    assert_eq!(provider_error_code(&nested).as_deref(), Some("slow_down"));

    assert!(provider_error_message(&serde_json::json!({})).is_none());
}

#[test]
fn expiry_keeps_a_margin_but_never_lands_in_the_past() {
    let now = super::super::credential::now_ms();
    let hour = expiry_from_expires_in(3600);
    assert!(hour > now, "must be in the future");
    assert!(hour < now + 3600 * 1000, "must reserve a refresh margin");
    // A very short-lived token still expires in the future rather than immediately.
    assert!(expiry_from_expires_in(10) > now);
}
