use super::*;

/// A one-route-table HTTP server on loopback: `/hop` redirects to `/landed`, `/big` sends 64
/// bytes, anything else answers `landed`.
async fn serve() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 1024];
            let read = socket.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let response = if request.starts_with("GET /hop ") {
                "HTTP/1.1 302 Found\r\nLocation: /landed\r\nContent-Length: 0\r\n\r\n".into()
            } else if request.starts_with("GET /big ") {
                format!("HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\n{}", "x".repeat(64))
            } else if request.starts_with("GET /latin1 ") {
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\n";
                let _ = socket
                    .write_all(format!("{head}Content-Length: 4\r\n\r\ncaf").as_bytes())
                    .await;
                let _ = socket.write_all(&[0xE9]).await;
                continue;
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nlanded".to_string()
            };
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    port
}

/// A page on another host must not steer the fetch onto this machine; a local origin
/// redirecting to itself is the dev server it always was.
#[tokio::test]
async fn a_redirect_onto_this_machine_needs_a_local_origin() {
    let port = serve().await;
    let url = format!("http://127.0.0.1:{port}/hop");
    let local = Hops { origin_local: None, body_cap: MAX_BODY_BYTES };
    assert_eq!(follow(&url, "t", LocalAccess::Allowed, local).await.unwrap().body, "landed");
    let from_public = Hops { origin_local: Some(false), body_cap: MAX_BODY_BYTES };
    let error = follow(&url, "t", LocalAccess::Allowed, from_public).await.unwrap_err();
    assert!(error.to_string().contains("redirect"), "{error}");
}

/// Whether a hop may land on this machine is decided by where the fetch started: never from
/// elsewhere, whatever spelling of loopback the hop resolves to, and not when only one of its
/// addresses is loopback.
#[test]
fn a_hop_onto_this_machine_is_refused_unless_the_origin_was_here() {
    let addrs = |ips: &[&str]| -> Vec<SocketAddr> {
        ips.iter().map(|ip| SocketAddr::new(ip.parse().unwrap(), 80)).collect()
    };
    let public = addrs(&["93.184.216.34"]);
    assert!(!is_local(&public));
    assert!(is_local(&addrs(&["127.0.0.1", "::1"])));
    assert!(!is_local(&addrs(&["93.184.216.34", "127.0.0.1"])), "a mixed answer is not local");
    for hop in [&["127.0.0.1"][..], &["::ffff:127.0.0.1"], &["93.184.216.34", "127.0.0.1"]] {
        assert!(lands_here_from_elsewhere(false, &addrs(hop)), "{hop:?}");
        assert!(!lands_here_from_elsewhere(true, &addrs(hop)), "{hop:?}");
    }
    assert!(!lands_here_from_elsewhere(false, &public));
}

/// The body is decoded in the charset the server declared, as `text()` did before the cap.
#[tokio::test]
async fn a_declared_charset_is_honored() {
    let port = serve().await;
    let url = format!("http://127.0.0.1:{port}/latin1");
    let fetched = safe_get(&url, "t", LocalAccess::Allowed).await.unwrap();
    assert_eq!(fetched.body, "café");
    assert!(!fetched.truncated);
}

/// A body past the cap stops being read, and the text says it was cut.
#[tokio::test]
async fn a_body_past_the_cap_is_cut_and_says_so() {
    let port = serve().await;
    let url = format!("http://127.0.0.1:{port}/big");
    let hops = Hops { origin_local: None, body_cap: 16 };
    let fetched = follow(&url, "t", LocalAccess::Allowed, hops).await.unwrap();
    assert_eq!(fetched.body, "x".repeat(16));
    assert!(fetched.truncated);
}

/// One test, not four: the thing under test is a **process-global** variable and the harness runs
/// tests in parallel threads, so four of them setting and clearing the same name raced. The
/// failure read as a broken parser when it was really the neighboring test.
#[test]
fn web_timeout_takes_a_valid_override_and_ignores_the_rest() {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("LUXE_WEB_TIMEOUT_SECS", value),
                None => std::env::remove_var("LUXE_WEB_TIMEOUT_SECS"),
            }
        }
    }
    let _restore = Restore(std::env::var("LUXE_WEB_TIMEOUT_SECS").ok());

    std::env::remove_var("LUXE_WEB_TIMEOUT_SECS");
    assert_eq!(web_timeout(), std::time::Duration::from_secs(30), "default");

    std::env::set_var("LUXE_WEB_TIMEOUT_SECS", "60");
    assert_eq!(web_timeout(), std::time::Duration::from_secs(60), "honored");

    // A typo must not silently remove the bound, and neither must a zero.
    for bad in ["not_a_number", "0", "", "-5"] {
        std::env::set_var("LUXE_WEB_TIMEOUT_SECS", bad);
        assert_eq!(web_timeout(), std::time::Duration::from_secs(30), "{bad:?}");
    }
}

/// The distinction the guard is built on: somewhere else, versus this machine.
#[test]
fn addresses_split_into_public_loopback_and_off_limits() {
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "140.82.112.3",
        "2606:4700:4700::1111",
        "64:ff9b::808:808", // NAT64 of a public address is still public
        "2002:808:808::1",  // 6to4 likewise
    ] {
        assert_eq!(classify_ip(ip.parse().unwrap()), AddressClass::Public, "{ip}");
    }
    for ip in ["127.0.0.1", "127.1.2.3", "::1", "::ffff:127.0.0.1"] {
        assert_eq!(classify_ip(ip.parse().unwrap()), AddressClass::Loopback, "{ip}");
    }
    for ip in [
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254", // cloud metadata
        "100.64.0.1",      // CGNAT
        "0.0.0.0",
        "198.18.0.1", // benchmarking
        "240.0.0.1",  // reserved
        "fe80::1",
        "fc00::1",
        "::ffff:192.168.1.1", // the same private host, IPv4-mapped
        "64:ff9b::a9fe:a9fe", // NAT64 of the metadata endpoint
        "64:ff9b::c0a8:101",  // NAT64 of a LAN host
        "2002:a9fe:a9fe::1",  // 6to4 of the metadata endpoint
        "2001:0:4136:e378:8000:63bf:5601:5efe", // Teredo, client 169.254.161.1
        "::a9fe:a9fe",        // IPv4-compatible
        "fec0::1",            // site-local
        "2001:db8::1",        // documentation
    ] {
        assert_eq!(classify_ip(ip.parse().unwrap()), AddressClass::Blocked, "{ip}");
    }
}

/// Loopback is a *policy* question, and the answer is the caller's. Nothing else moves with it —
/// admitting a dev server on this machine must not admit the cloud metadata endpoint.
#[test]
fn only_loopback_follows_the_local_access_policy() {
    let cases = [
        ("1.1.1.1", true, true),
        ("127.0.0.1", true, false),
        ("::1", true, false),
        ("169.254.169.254", false, false),
        ("192.168.1.1", false, false),
    ];
    for (ip, when_allowed, when_denied) in cases {
        let class = classify_ip(ip.parse().unwrap());
        assert_eq!(class.is_reachable(LocalAccess::Allowed), when_allowed, "{ip} allowed");
        assert_eq!(class.is_reachable(LocalAccess::Denied), when_denied, "{ip} denied");
    }
}

#[tokio::test]
async fn rejects_non_http_schemes_whatever_the_local_policy() {
    for local in [LocalAccess::Allowed, LocalAccess::Denied] {
        for url in ["file:///etc/passwd", "ftp://example.com", "data:text/html,hi"] {
            assert!(
                resolve_validated(&Url::parse(url).unwrap(), local).await.is_err(),
                "{url} under {local:?}"
            );
        }
    }
}

/// The bug this split fixes: a dev server on this machine is the `screenshot`/`web_fetch` use
/// case, not an attack. It resolves and passes; it does not need an env var set before launch.
#[tokio::test]
async fn a_local_dev_server_resolves_when_local_access_is_allowed() {
    for url in ["http://127.0.0.1:4321/", "http://localhost:4321/", "http://[::1]:4321/"] {
        let addrs = resolve_validated(&Url::parse(url).unwrap(), LocalAccess::Allowed)
            .await
            .unwrap_or_else(|error| panic!("{url} must resolve: {error}"));
        assert!(!addrs.is_empty(), "{url}");
        assert!(addrs.iter().all(|addr| addr.ip().is_loopback()), "{url} stayed on loopback");
    }
}

#[tokio::test]
async fn refuses_local_origins_when_local_access_is_denied() {
    for url in ["http://127.0.0.1:8080/x", "http://localhost:8080/x", "http://foo.localhost/x"] {
        assert!(
            resolve_validated(&Url::parse(url).unwrap(), LocalAccess::Denied).await.is_err(),
            "{url}"
        );
    }
}

/// Metadata and private-LAN targets stay refused even for the tools that are allowed loopback.
/// This is the assertion that keeps the carve-out from quietly becoming a hole.
#[tokio::test]
async fn metadata_and_private_hosts_stay_refused_even_with_local_access() {
    for url in [
        "http://169.254.169.254/latest/meta-data",
        "http://192.168.0.5/admin",
        "http://10.0.0.1/",
        "http://172.16.5.5/",
        "http://100.64.0.1/",
    ] {
        let error = resolve_validated(&Url::parse(url).unwrap(), LocalAccess::Allowed)
            .await
            .expect_err(&format!("{url} must stay blocked"));
        let message = error.to_string();
        assert!(
            message.contains("not this machine"),
            "the refusal should say which boundary it is: {message}"
        );
    }
}
