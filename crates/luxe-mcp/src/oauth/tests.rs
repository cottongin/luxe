use super::discovery::{
    auth_param, authorization_server_candidates, names_the_server, origin_of,
    protected_resource_candidates,
};
use super::endpoints::is_loopback;
use super::*;
use crate::token_store::StoredToken;

fn url(text: &str) -> url::Url {
    url::Url::parse(text).expect("a URL")
}

/// RFC 9728 puts the document between the host and the path, and the path form is tried before
/// the root — which is the whole of the Asana case: its root describes a retired server.
#[test]
fn protected_resource_is_looked_for_at_the_path_first() {
    assert_eq!(
        protected_resource_candidates(&url("https://mcp.asana.com/v2/mcp")),
        [
            "https://mcp.asana.com/.well-known/oauth-protected-resource/v2/mcp",
            "https://mcp.asana.com/.well-known/oauth-protected-resource",
        ]
    );
    assert_eq!(
        protected_resource_candidates(&url("http://127.0.0.1:9000/")),
        ["http://127.0.0.1:9000/.well-known/oauth-protected-resource"],
        "a server at the root has one place to look, with its port"
    );
}

#[test]
fn authorization_server_metadata_follows_rfc_8414_then_openid() {
    assert_eq!(
        authorization_server_candidates(&url("https://app.asana.com")),
        [
            "https://app.asana.com/.well-known/oauth-authorization-server",
            "https://app.asana.com/.well-known/openid-configuration",
        ]
    );
    assert_eq!(
        authorization_server_candidates(&url("https://id.example.com/tenant/7")),
        [
            "https://id.example.com/.well-known/oauth-authorization-server/tenant/7",
            "https://id.example.com/tenant/7/.well-known/openid-configuration",
            "https://id.example.com/.well-known/openid-configuration/tenant/7",
        ]
    );
}

/// A document may describe the server or a resource above it, never a different one.
#[test]
fn a_resource_must_name_the_server_it_was_fetched_for() {
    let server = url("https://mcp.asana.com/v2/mcp");
    for resource in
        ["https://mcp.asana.com/v2/mcp", "https://mcp.asana.com/v2", "https://mcp.asana.com"]
    {
        assert!(names_the_server(resource, &server), "{resource}");
    }
    for resource in [
        "https://evil.example.com/v2/mcp",
        "http://mcp.asana.com/v2/mcp",
        "https://mcp.asana.com:8443/v2/mcp",
        "https://mcp.asana.com/v2/mcp/deeper",
        "https://mcp.asana.com/v",
        "not a url",
        "",
    ] {
        assert!(!names_the_server(resource, &server), "{resource}");
    }
}

#[test]
fn the_metadata_pointer_is_read_from_the_challenge() {
    let asana = r#"Bearer realm="Asana MCP", resource_metadata="https://mcp.asana.com/.well-known/oauth-protected-resource/v2", error="invalid_request""#;
    assert_eq!(
        auth_param(asana, "resource_metadata").as_deref(),
        Some("https://mcp.asana.com/.well-known/oauth-protected-resource/v2")
    );
    assert_eq!(
        auth_param("Bearer resource_metadata=https://x.example/m, realm=y", "resource_metadata")
            .as_deref(),
        Some("https://x.example/m"),
        "an unquoted value ends at the next parameter"
    );
    assert_eq!(auth_param(r#"Bearer realm="x""#, "resource_metadata"), None);
    assert_eq!(auth_param(r#"Bearer resource_metadata="""#, "resource_metadata"), None);
}

#[test]
fn loopback_is_the_three_hosts_a_browser_can_hand_a_code_to() {
    for host in ["http://127.0.0.1:1/", "http://localhost:1/", "http://[::1]:1/"] {
        assert!(is_loopback(&url(host)), "{host}");
    }
    for host in ["http://127.0.0.2:1/", "http://example.com:1/", "http://0.0.0.0:1/"] {
        assert!(!is_loopback(&url(host)), "{host}");
    }
    assert_eq!(origin_of(&url("http://[::1]:8765/x")).as_deref(), Some("http://[::1]:8765"));
}

/// What the tab says is what happened, and what a provider wrote in an error cannot become markup.
#[test]
fn the_landing_page_says_what_happened() {
    let (status, _, signed_in) = landing("asana", &Ok(()));
    assert_eq!(status, 200);
    assert!(signed_in.contains("signed in to asana"), "{signed_in}");
    let (status, title, refused) = landing(
        "asana",
        &Err(OAuthError::Refused("the server refused: <script>x</script>".into())),
    );
    assert_eq!(status, 400);
    assert!(refused.contains("could not sign in"), "{refused}");
    let page = luxe_core::oauth_loopback::page(title, &refused, "#b45309");
    assert!(page.contains("&lt;script&gt;") && !page.contains("<script>"), "{page}");
}

/// https anywhere, plain http only on this machine, and nothing else.
#[test]
fn only_https_or_loopback_http_is_secure() {
    for ok in [
        "https://auth.example.com/t",
        "http://127.0.0.1:9/t",
        "http://localhost/t",
        "http://[::1]:8/t",
    ] {
        assert!(require_secure(ok, "the token endpoint").is_ok(), "{ok}");
    }
    for bad in ["http://auth.example.com/t", "ftp://auth.example.com/t", "not a url"] {
        let error = require_secure(bad, "the token endpoint").unwrap_err();
        assert!(matches!(error, OAuthError::Insecure(_)), "{error:?}");
        assert!(error.to_string().contains("the token endpoint"), "{error}");
    }
}

/// A loopback server answering every authorization-server metadata path with `doc`, and nothing
/// else; returns its base URL.
async fn serve_origin_metadata(doc: impl Fn(&str) -> serde_json::Value + Send + 'static) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = base.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 4096];
            let read = socket.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let (status, body) = if request.contains("/.well-known/oauth-authorization-server") {
                ("200 OK", doc(&served).to_string())
            } else {
                ("404 Not Found", "{}".to_string())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    base
}

/// The origin fallback gets the checks discovery gets: a document must name its issuer, and an
/// endpoint in it must be secure.
#[tokio::test]
async fn the_origin_fallback_is_checked_like_discovery() {
    for (doc, refusal) in [
        (
            (|base: &str| serde_json::json!({"issuer": base, "token_endpoint": "http://evil.example/t"}))
                as fn(&str) -> serde_json::Value,
            "not https",
        ),
        (
            |base: &str| serde_json::json!({"token_endpoint": format!("{base}/t")}),
            "names no issuer",
        ),
    ] {
        let base = serve_origin_metadata(doc).await;
        let client =
            OAuthClient::new("legacy".into(), format!("{base}/v2/mcp"), OAuthConfig::default());
        let error = client.auth_start("http://127.0.0.1:9/callback").await.unwrap_err().to_string();
        assert!(error.contains(refusal), "{error}");
    }
}

/// A server reached over plain http off this machine has nothing it says trusted: its metadata
/// could be anyone's, so sign-in stops before discovery.
#[tokio::test]
async fn a_plain_http_server_is_not_signed_in_to() {
    let client = OAuthClient::new(
        "plain".into(),
        "http://mcp.example.test/v2/mcp".into(),
        OAuthConfig::default(),
    );
    let error = client.auth_start("http://127.0.0.1:9/callback").await.unwrap_err().to_string();
    assert!(error.contains("not https"), "{error}");
}

/// A sign-in stored before endpoints were checked is checked on refresh: its refresh token does
/// not go to a plain-http endpoint.
#[tokio::test]
async fn a_stored_plain_http_token_endpoint_is_not_refreshed_against() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path().join("t.json"));
    let token = StoredToken {
        access_token: "AT-old".into(),
        refresh_token: Some("RT-old".into()),
        expires_at_secs: Some(1),
        client_id: Some("1234".into()),
        client_secret: None,
        token_endpoint: Some("http://evil.example/t".into()),
        resource: Some("https://mcp.example.test/v2/mcp".into()),
    };
    store.put("old", token).unwrap();
    let client = OAuthClient::new(
        "old".into(),
        "https://mcp.example.test/v2/mcp".into(),
        OAuthConfig::default(),
    )
    .with_store(store);
    let TokenState::Unavailable(reason) = client.access_token().await else {
        panic!("a plain-http endpoint must not be refreshed against");
    };
    assert!(reason.contains("not https"), "{reason}");
}

/// Two calls that find the token expired refresh it once: the second waits for the first and reads
/// what it stored. A server rotating refresh tokens would refuse the second use of the old one and
/// send the reader to sign in again for nothing.
#[tokio::test]
async fn two_concurrent_refreshes_hit_the_endpoint_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let refreshes = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = refreshes.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 4096];
            let read = socket.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            // the old refresh token is good once, as a rotating server has it
            let body = if request.contains("refresh_token=RT-1")
                && counted.fetch_add(1, Ordering::SeqCst) == 0
            {
                r#"{"access_token":"AT-2","token_type":"Bearer","expires_in":3600,"refresh_token":"RT-2"}"#
            } else {
                r#"{"error":"invalid_grant"}"#
            };
            let status = if body.contains("AT-2") { "200 OK" } else { "400 Bad Request" };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path().join("t.json"));
    let token = StoredToken {
        access_token: "AT-1".into(),
        refresh_token: Some("RT-1".into()),
        expires_at_secs: Some(1),
        client_id: Some("1234".into()),
        client_secret: None,
        token_endpoint: Some(format!("{base}/token")),
        resource: Some("https://mcp.example.test/v2/mcp".into()),
    };
    store.put("rotating", token).unwrap();
    let client = OAuthClient::new(
        "rotating".into(),
        "https://mcp.example.test/v2/mcp".into(),
        OAuthConfig::default(),
    )
    .with_store(store);
    let (one, two) = tokio::join!(client.access_token(), client.access_token());
    for state in [one, two] {
        let TokenState::Have(token) = state else { panic!("a refresh was refused") };
        assert_eq!(token, "AT-2");
    }
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
}
