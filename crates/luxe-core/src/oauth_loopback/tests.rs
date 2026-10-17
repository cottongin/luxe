use super::*;

/// Send `GET <target>` to `port` on `host` and return the response.
async fn send(host: &str, port: u16, target: &str) -> String {
    let mut socket = TcpStream::connect(format!("{host}:{port}")).await.unwrap();
    let request = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    let _ = socket.read_to_string(&mut response).await;
    response
}

fn port_of(callback: &LoopbackCallback) -> u16 {
    url::Url::parse(callback.redirect_uri()).unwrap().port().unwrap()
}

#[test]
fn pkce_challenge_is_the_s256_of_the_verifier() {
    let first = pkce();
    assert_eq!(first.challenge, base64_url(&Sha256::digest(first.verifier.as_bytes())));
    assert!(!first.verifier.contains(['=', '+', '/']), "url-safe and unpadded");
    assert_ne!(first.verifier, pkce().verifier);
    assert_ne!(random_state(), random_state());
}

/// A connection that opens and says nothing — a preconnect, a port scan — is dropped after the
/// read deadline instead of holding the real redirect behind it until the sign-in times out.
#[tokio::test]
async fn a_silent_connection_does_not_hold_up_the_redirect() {
    let callback = LoopbackCallback::bind("http://127.0.0.1:0/cb").await.unwrap();
    let port = port_of(&callback);
    let _silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let browser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        send("127.0.0.1", port, "/cb?code=abc&state=s").await
    });
    let arrival = tokio::time::timeout(Duration::from_secs(10), callback.wait(Some("s")))
        .await
        .expect("the silent connection did not stall the listener")
        .unwrap();
    assert_eq!(arrival.param("code"), Some("abc"));
    arrival.answer(200, "Signed in", "done").await;
    assert!(browser.await.unwrap().starts_with("HTTP/1.1 200"));
}

/// A redirect carrying another sign-in's `state` is answered 400 and ignored, and the right one
/// still completes; a stray path is answered 404.
#[tokio::test]
async fn a_wrong_state_is_refused_and_the_listener_keeps_waiting() {
    let callback = LoopbackCallback::bind("http://127.0.0.1:0/cb").await.unwrap();
    let port = port_of(&callback);
    let browser = tokio::spawn(async move {
        let stray = send("127.0.0.1", port, "/favicon.ico").await;
        let wrong = send("127.0.0.1", port, "/cb?code=evil&state=wrong").await;
        let forged = send("127.0.0.1", port, "/cb?error=access_denied").await;
        let right = send("127.0.0.1", port, "/cb?code=abc&state=s").await;
        (stray, wrong, forged, right)
    });
    let arrival = callback.wait(Some("s")).await.unwrap();
    assert_eq!(arrival.param("code"), Some("abc"));
    arrival.answer(200, "Signed in", "done").await;
    let (stray, wrong, forged, right) = browser.await.unwrap();
    assert!(stray.starts_with("HTTP/1.1 404"), "{stray}");
    assert!(wrong.starts_with("HTTP/1.1 400"), "{wrong}");
    assert!(forged.starts_with("HTTP/1.1 400"), "an error without our state is not ours: {forged}");
    assert!(right.starts_with("HTTP/1.1 200"), "{right}");
}

/// What a request carried is escaped before a page repeats it.
#[tokio::test]
async fn an_error_description_is_escaped_in_the_page() {
    let callback = LoopbackCallback::bind("http://127.0.0.1:0/cb").await.unwrap();
    let port = port_of(&callback);
    let target = "/cb?error=x&error_description=%3Cscript%3Ealert(1)%3C/script%3E";
    let browser = tokio::spawn(async move { send("127.0.0.1", port, target).await });
    let arrival = callback.wait(None).await.unwrap();
    let message = format!("Authorization failed: {}", arrival.error().unwrap());
    arrival.answer(400, "Luxe couldn't finish the sign-in", &message).await;
    let page = browser.await.unwrap();
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{page}");
    assert!(!page.contains("<script>"), "{page}");
}

/// `localhost` may resolve to either family, so both are listened on, at one port.
#[tokio::test]
async fn localhost_is_heard_on_both_address_families() {
    if std::net::TcpListener::bind("[::1]:0").is_err() {
        return; // no IPv6 loopback on this machine
    }
    let callback = LoopbackCallback::bind("http://localhost:0/auth/callback").await.unwrap();
    assert!(
        callback.redirect_uri().starts_with("http://localhost:"),
        "{}",
        callback.redirect_uri()
    );
    let port = port_of(&callback);
    let browser = tokio::spawn(async move { send("[::1]", port, "/auth/callback?code=v6").await });
    let arrival = callback.wait(None).await.unwrap();
    assert_eq!(arrival.param("code"), Some("v6"));
    arrival.answer(200, "Signed in", "done").await;
    assert!(browser.await.unwrap().starts_with("HTTP/1.1 200"));
}

/// Only an http address on this machine, with a port, can take a redirect.
#[tokio::test]
async fn a_redirect_off_this_machine_or_without_a_port_is_refused() {
    for (uri, reason) in [
        ("https://127.0.0.1:8765/callback", "must be http"),
        ("http://example.com:8765/callback", "must be http"),
        ("http://127.0.0.1/callback", "explicit port"),
        ("not a url", "not a URL"),
    ] {
        let error = LoopbackCallback::bind(uri).await.err().expect(uri);
        assert!(error.to_string().contains(reason), "{uri}: {error}");
    }
}

/// A configured redirect is the address listened on, exactly; port `0` takes a free one and the
/// redirect URI names it.
#[tokio::test]
async fn a_configured_redirect_is_the_address_listened_on() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let configured = format!("http://127.0.0.1:{free}/oauth/done");
    let callback = LoopbackCallback::bind(&configured).await.unwrap();
    assert_eq!(callback.redirect_uri(), configured);
    assert_eq!(callback.path, "/oauth/done");
    assert_eq!(callback.listeners[0].local_addr().unwrap().port(), free);

    // written exactly as configured, since a provider compares the registered string
    let bare = format!("http://127.0.0.1:{}", free + 1);
    if let Ok(callback) = LoopbackCallback::bind(&bare).await {
        assert_eq!(callback.redirect_uri(), bare);
    }

    let fresh = LoopbackCallback::bind("http://127.0.0.1:0/callback").await.unwrap();
    let port = fresh.listeners[0].local_addr().unwrap().port();
    assert_eq!(fresh.redirect_uri(), format!("http://127.0.0.1:{port}/callback"));
}

/// A port something else holds is an error, even when the other address family is free: a browser
/// trying that family first would hand the code to whoever holds it.
#[tokio::test]
async fn a_port_held_on_one_family_is_refused() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let error = LoopbackCallback::bind(&format!("http://localhost:{port}/cb")).await.err();
    assert!(matches!(error, Some(LoopbackError::InUse { .. })), "{error:?}");
}
