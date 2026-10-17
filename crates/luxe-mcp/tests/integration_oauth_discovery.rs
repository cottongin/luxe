//! Signing in to a server that pre-registers its clients, against a mock shaped like Asana's.
//!
//! Two hermetic servers on loopback. The MCP host answers an unauthenticated request with a 401
//! pointing at its protected-resource metadata, serves that metadata at the RFC 9728 path — and,
//! at its root, the metadata of a *different* authorization server with dynamic registration, the
//! way Asana's host still describes the beta it retired. The authorization server is on another
//! origin and has no registration endpoint. Every request is recorded, so a test can say exactly
//! what was sent where. The test plays the browser; nothing here reaches a real server or opens
//! one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use luxe_core::mcp_client::{OAuthConfig, OAuthGrant};
use luxe_mcp::oauth::{OAuthClient, TokenState};
use luxe_mcp::token_store::{StoredToken, TokenStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CALLBACK: &str = "http://127.0.0.1:9/callback";

/// One request a mock received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: String,
}

/// How the MCP host describes itself.
#[derive(Clone, Copy, PartialEq)]
enum Host {
    /// Asana today: the 401 names the path document, and the root is the retired beta
    Asana,
    /// No pointer in the 401; the path document still exists
    NoPointer,
    /// The path document describes some other origin
    Spoofed,
    /// The 401 names a document somewhere no convention would look, and only there
    PointerOnly,
    /// No protected-resource metadata at all: the origin's own metadata, with registration
    Legacy,
    /// The same, with a registration endpoint that fails
    LegacyBrokenRegistration,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// A mock authorization server; returns its base URL. `issuer` overrides what it claims to be.
async fn spawn_authorization_server(log: Log, issuer: Option<String>) -> String {
    spawn_authorization_server_with(log, issuer, false).await
}

/// The same, optionally describing a token endpoint and nothing else, as a machine-to-machine
/// server does.
async fn spawn_authorization_server_with(
    log: Log,
    issuer: Option<String>,
    token_only: bool,
) -> String {
    serve(log, move |base, seen| {
        let issuer = issuer.clone().unwrap_or_else(|| base.to_string());
        match (seen.method.as_str(), seen.path.as_str()) {
            ("GET", "/.well-known/oauth-authorization-server") if token_only => ok(
                serde_json::json!({ "issuer": issuer, "token_endpoint": format!("{base}/-/oauth_token") }),
            ),
            ("GET", "/.well-known/oauth-authorization-server") => ok(serde_json::json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{base}/-/oauth_authorize"),
                "token_endpoint": format!("{base}/-/oauth_token"),
                "token_endpoint_auth_methods_supported": ["client_secret_post"],
                "code_challenge_methods_supported": ["S256"],
            })),
            // A client the server no longer knows, refused at any grant
            ("POST", "/-/oauth_token") if seen.body.contains("client_id=revoked") => Reply {
                status: "401 Unauthorized",
                headers: Vec::new(),
                body: r#"{"error":"invalid_client","error_description":"Unknown client"}"#.into(),
            },
            // A revoked grant, answered the way a real server answers one
            ("POST", "/-/oauth_token") if seen.body.contains("refresh_token=RT-revoked") => Reply {
                status: "400 Bad Request",
                headers: Vec::new(),
                body: r#"{"error":"invalid_grant","error_description":"The refresh token is invalid"}"#
                    .into(),
            },
            ("POST", "/-/oauth_token") => ok(serde_json::json!({
                "access_token": "AT-fresh", "token_type": "Bearer",
                "expires_in": 3600, "refresh_token": "RT-fresh",
            })),
            _ => not_found(),
        }
    })
    .await
}

/// A mock MCP host whose metadata points at `authorization_server`; returns its base URL.
async fn spawn_mcp_host(log: Log, authorization_server: String, host: Host) -> String {
    serve(log, move |base, seen| {
        let described = match host {
            Host::Spoofed => "https://elsewhere.example/v2/mcp".to_string(),
            _ => format!("{base}/v2/mcp"),
        };
        let path_document = serde_json::json!({
            "resource": described,
            "authorization_servers": [authorization_server.clone()],
        });
        let legacy = matches!(host, Host::Legacy | Host::LegacyBrokenRegistration);
        match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/v2/mcp") => {
                let challenge = match host {
                    Host::NoPointer | Host::Legacy | Host::LegacyBrokenRegistration => {
                        "Bearer realm=\"mock\"".to_string()
                    }
                    Host::PointerOnly => {
                        format!("Bearer resource_metadata=\"{base}/meta/unusual\"")
                    }
                    _ => format!(
                        "Bearer realm=\"mock\", resource_metadata=\"{base}/.well-known/oauth-protected-resource/v2\""
                    ),
                };
                Reply { status: "401 Unauthorized", headers: vec![("WWW-Authenticate", challenge)], body: "{}".into() }
            }
            ("GET", "/meta/unusual") if host == Host::PointerOnly => ok(path_document),
            ("GET", path)
                if (legacy || host == Host::PointerOnly)
                    && path.starts_with("/.well-known/oauth-protected-resource") =>
            {
                not_found()
            }
            ("POST", "/register") if host == Host::LegacyBrokenRegistration => Reply {
                status: "500 Internal Server Error",
                headers: Vec::new(),
                body: "{}".into(),
            },
            ("POST", "/register") => ok(serde_json::json!({ "client_id": "registered-id" })),
            ("GET", "/.well-known/oauth-protected-resource/v2")
            | ("GET", "/.well-known/oauth-protected-resource/v2/mcp") => ok(path_document),
            // The trap: the root describes the host itself, with a registering authorization server
            ("GET", "/.well-known/oauth-protected-resource") => ok(serde_json::json!({
                "resource": base.to_string(),
                "authorization_servers": [base.to_string()],
            })),
            ("GET", "/.well-known/oauth-authorization-server") => ok(serde_json::json!({
                "issuer": base.to_string(),
                "authorization_endpoint": format!("{base}/retired-authorize"),
                "token_endpoint": format!("{base}/retired-token"),
                "registration_endpoint": format!("{base}/register"),
            })),
            _ => not_found(),
        }
    })
    .await
}

struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn ok(body: serde_json::Value) -> Reply {
    Reply { status: "200 OK", headers: Vec::new(), body: body.to_string() }
}

fn not_found() -> Reply {
    Reply { status: "404 Not Found", headers: Vec::new(), body: "{}".into() }
}

/// Serves `route` on a fresh loopback port, recording every request into `log`.
async fn serve(log: Log, route: impl Fn(&str, &Seen) -> Reply + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let route = Arc::new(route);
    let served = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let (log, route, base) = (log.clone(), route.clone(), served.clone());
            tokio::spawn(async move {
                let Some(seen) = read_request(&mut socket).await else { return };
                log.lock().unwrap().push(seen.clone());
                let reply = route(&base, &seen);
                let mut response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in reply.headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&reply.body);
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    base
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut first = head.lines().next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.split('?').next()?.to_string();
    let length = head
        .lines()
        .find_map(|line| {
            line.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string())
        })
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await.ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..head_end + length]).to_string();
    Some(Seen { method, path, body })
}

/// A form body's value for `name`.
fn form(body: &str, name: &str) -> Option<String> {
    url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

fn query(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url).ok()?.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

/// A client configured the way Asana needs one: pre-registered, secret in an environment variable.
fn asana_client(host: &str, store: TokenStore, redirect: Option<String>) -> OAuthClient {
    OAuthClient::new(
        "asana".into(),
        format!("{host}/v2/mcp"),
        OAuthConfig {
            grant_type: OAuthGrant::AuthorizationCode,
            client_id: Some("1234".into()),
            client_secret: Some("${LUXE_TEST_ASANA_SECRET}".into()),
            redirect_uri: redirect,
            ..Default::default()
        },
    )
    .with_store(store)
}

struct World {
    host: String,
    authorization: String,
    log: Log,
    _dir: tempfile::TempDir,
    store_path: std::path::PathBuf,
}

async fn world(host: Host) -> World {
    world_with(host).await
}

async fn world_with(host: Host) -> World {
    std::env::set_var("LUXE_TEST_ASANA_SECRET", "s3cret-from-env");
    let log: Log = Arc::default();
    let authorization = spawn_authorization_server(log.clone(), None).await;
    let host = spawn_mcp_host(log.clone(), authorization.clone(), host).await;
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("mcp-auth.json");
    World { host, authorization, log, _dir: dir, store_path }
}

impl World {
    fn client(&self, redirect: Option<String>) -> OAuthClient {
        asana_client(&self.host, TokenStore::new(self.store_path.clone()), redirect)
    }

    fn seen(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }
}

/// The pointer in the 401 leads to the real authorization server, not the retired one the root
/// describes — which is exactly what reading the root first got wrong.
#[tokio::test]
async fn discovery_follows_the_pointer_past_the_retired_server() {
    let world = world(Host::Asana).await;
    let url = world.client(None).auth_start("http://127.0.0.1:9/callback").await.expect("starts");
    assert!(url.starts_with(&format!("{}/-/oauth_authorize?", world.authorization)), "{url}");
    assert_eq!(query(&url, "resource"), Some(format!("{}/v2/mcp", world.host)));
    assert_eq!(query(&url, "client_id").as_deref(), Some("1234"));
    assert_eq!(query(&url, "scope"), None, "Asana errors on any scope");
    let seen = world.seen();
    assert!(!seen.iter().any(|s| s.path == "/register" || s.path.starts_with("/retired")));
    assert!(
        !seen.iter().any(|s| s.path == "/.well-known/oauth-protected-resource"),
        "the root, which describes the retired server, is never read: {seen:?}"
    );
}

/// Without a pointer, the RFC 9728 path form is still read before the root.
#[tokio::test]
async fn without_a_pointer_the_path_form_is_read_before_the_root() {
    let world = world(Host::NoPointer).await;
    let url = world.client(None).auth_start("http://127.0.0.1:9/callback").await.expect("starts");
    assert!(url.starts_with(&format!("{}/-/oauth_authorize?", world.authorization)), "{url}");
    let paths: Vec<String> = world.seen().into_iter().map(|s| s.path).collect();
    assert!(
        paths.contains(&"/.well-known/oauth-protected-resource/v2/mcp".to_string()),
        "{paths:?}"
    );
    assert!(!paths.contains(&"/.well-known/oauth-protected-resource".to_string()), "{paths:?}");
}

/// Metadata that describes another origin is refused rather than used.
#[tokio::test]
async fn metadata_for_another_server_is_refused() {
    let world = world(Host::Spoofed).await;
    let error = world.client(None).auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("refusing to send a token"), "{error}");
}

/// An authorization server that claims to be someone else is refused, and so is one that will
/// not describe itself — with no fallback to the host's own metadata, which is the wrong server.
#[tokio::test]
async fn an_authorization_server_must_be_the_one_named() {
    let log: Log = Arc::default();
    let impostor =
        spawn_authorization_server(log.clone(), Some("https://someone.else".into())).await;
    let host = spawn_mcp_host(log.clone(), impostor, Host::Asana).await;
    let dir = tempfile::tempdir().unwrap();
    let client = asana_client(&host, TokenStore::new(dir.path().join("t.json")), None);
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("refusing it"), "{error}");

    // Nothing listens here, so it describes nothing
    let silent = spawn_mcp_host(log.clone(), "http://127.0.0.1:9".into(), Host::Asana).await;
    let client = asana_client(&silent, TokenStore::new(dir.path().join("u.json")), None);
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("publishes no authorization server metadata"), "{error}");
    assert!(!log
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.path.starts_with("/retired") || s.path == "/register"));
}

/// The whole login, with the test as the browser: a stray request is answered and ignored, the
/// redirect completes it, the exchange carries the expanded secret, the resource and the verifier,
/// and the stored token remembers where it came from — without the configured secret.
#[tokio::test]
async fn a_login_completes_through_the_registered_redirect() {
    let world = world(Host::Asana).await;
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let client = world.client(Some(redirect.clone()));
    let (sender, receiver) = tokio::sync::oneshot::channel::<(String, String)>();
    let browser = tokio::spawn(async move {
        let (url, redirect) = receiver.await.unwrap();
        let http = reqwest::Client::new();
        let origin = redirect.trim_end_matches("/callback");
        let stray = http.get(format!("{origin}/favicon.ico")).send().await.unwrap();
        let state = query(&url, "state").unwrap();
        // no answer, another sign-in's answer, and a forged refusal: none of them ends this one
        for stray in [format!("state={state}"), "code=evil&state=wrong".into(), "error=no".into()] {
            let answer = http.get(format!("{redirect}?{stray}")).send().await.unwrap();
            assert_eq!(answer.status().as_u16(), 400, "{stray}");
        }
        let landing =
            http.get(format!("{redirect}?code=the-code&state={state}")).send().await.unwrap();
        (stray.status().as_u16(), landing.text().await.unwrap())
    });
    let mut sender = Some(sender);
    client
        .login_with(
            |url, shown| {
                assert_eq!(shown, redirect, "the registered redirect is the one used");
                sender.take().unwrap().send((url.to_string(), shown.to_string())).unwrap();
            },
            Duration::from_secs(10),
        )
        .await
        .expect("login completes");
    let (stray, landing) = browser.await.unwrap();
    assert_eq!(stray, 404, "a stray request is not the redirect");
    assert!(landing.contains("signed in to asana"), "{landing}");

    let exchange =
        world.seen().into_iter().find(|s| s.path == "/-/oauth_token").expect("exchanged");
    assert_eq!(form(&exchange.body, "client_secret").as_deref(), Some("s3cret-from-env"));
    assert_eq!(form(&exchange.body, "resource"), Some(format!("{}/v2/mcp", world.host)));
    assert_eq!(form(&exchange.body, "redirect_uri").as_deref(), Some(redirect.as_str()));
    assert!(form(&exchange.body, "code_verifier").is_some());

    let stored = TokenStore::new(world.store_path.clone()).get("asana").expect("stored");
    assert_eq!(stored.access_token, "AT-fresh");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let expires = stored.expires_at_secs.expect("an expiry");
    assert!((now + 3500..=now + 3700).contains(&expires), "an hour from now: {expires} vs {now}");
    assert!(!stored.is_expired());
    assert_eq!(stored.client_secret, None, "the configured secret stays in the config");
    assert_eq!(stored.token_endpoint, Some(format!("{}/-/oauth_token", world.authorization)));
    assert_eq!(stored.resource, Some(format!("{}/v2/mcp", world.host)));
    let written = std::fs::read_to_string(&world.store_path).unwrap();
    assert!(!written.contains("s3cret-from-env"), "nor is it written beside the tokens");
}

/// A refused consent ends the login with the provider's reason, and the tab says so.
#[tokio::test]
async fn a_refused_consent_ends_the_login_and_says_why() {
    let world = world(Host::Asana).await;
    let client = world.client(None);
    let (sender, receiver) = tokio::sync::oneshot::channel::<(String, String)>();
    let browser = tokio::spawn(async move {
        let (url, redirect) = receiver.await.unwrap();
        let state = query(&url, "state").unwrap();
        let landing = reqwest::get(format!(
            "{redirect}?error=access_denied&error_description=The+user+said+no&state={state}"
        ))
        .await
        .unwrap();
        landing.text().await.unwrap()
    });
    let mut sender = Some(sender);
    let error = client
        .login_with(
            |url, redirect| {
                sender.take().unwrap().send((url.to_string(), redirect.to_string())).unwrap()
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("access_denied") && error.contains("The user said no"), "{error}");
    let landing = browser.await.unwrap();
    assert!(
        landing.contains("could not sign in") && landing.contains("The user said no"),
        "{landing}"
    );
    assert!(!world.seen().iter().any(|s| s.path == "/-/oauth_token"), "no exchange was attempted");
}

/// A browser that never comes back ends the wait rather than holding the terminal forever.
#[tokio::test]
async fn a_login_nobody_finishes_times_out() {
    let world = world(Host::Asana).await;
    let waited = world.client(None).login_with(|_, _| {}, Duration::from_millis(300)).await;
    let error = waited.unwrap_err().to_string();
    assert!(error.contains("no answer from the browser"), "{error}");
}

/// A refresh goes back to the token endpoint and resource the token was issued for, with the
/// secret expanded — and runs no discovery to get there.
#[tokio::test]
async fn a_refresh_goes_back_where_the_token_came_from() {
    let world = world(Host::Asana).await;
    TokenStore::new(world.store_path.clone())
        .put(
            "asana",
            StoredToken {
                access_token: "AT-old".into(),
                refresh_token: Some("RT-old".into()),
                expires_at_secs: Some(1),
                client_id: Some("1234".into()),
                client_secret: None,
                token_endpoint: Some(format!("{}/-/oauth_token", world.authorization)),
                resource: Some(format!("{}/v2/mcp", world.host)),
            },
        )
        .unwrap();
    let TokenState::Have(token) = world.client(None).access_token().await else {
        panic!("the refresh should succeed");
    };
    assert_eq!(token, "AT-fresh");
    // the token a request will carry is remembered for redaction
    assert_eq!(luxe_core::redact::process_secrets().redact("got AT-fresh"), "got [REDACTED]");
    let seen = world.seen();
    assert_eq!(seen.len(), 1, "one request, no discovery: {seen:?}");
    assert_eq!(form(&seen[0].body, "grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(form(&seen[0].body, "client_secret").as_deref(), Some("s3cret-from-env"));
    assert_eq!(form(&seen[0].body, "resource"), Some(format!("{}/v2/mcp", world.host)));
}

/// A `!command` secret — a password manager, in practice — is run for the refresh and its first
/// line sent; for a server that is not from your own config it is refused, and no token is asked
/// for.
#[cfg(unix)]
#[tokio::test]
async fn a_command_secret_is_run_for_your_servers_and_refused_for_others() {
    let world = world(Host::Asana).await;
    TokenStore::new(world.store_path.clone())
        .put(
            "asana",
            StoredToken {
                access_token: "AT-old".into(),
                refresh_token: Some("RT-old".into()),
                expires_at_secs: Some(1),
                client_id: Some("1234".into()),
                client_secret: None,
                token_endpoint: Some(format!("{}/-/oauth_token", world.authorization)),
                resource: Some(format!("{}/v2/mcp", world.host)),
            },
        )
        .unwrap();
    let client = |yours: bool| {
        OAuthClient::new(
            "asana".into(),
            format!("{}/v2/mcp", world.host),
            OAuthConfig {
                client_id: Some("1234".into()),
                client_secret: Some("!printf 's3cret-from-pass\\nlogin: evan\\n'".into()),
                ..Default::default()
            },
        )
        .with_store(TokenStore::new(world.store_path.clone()))
        .with_secret_commands(yours)
    };

    let error = client(false).auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("is a command") && error.contains("mcp.json"), "{error}");
    assert!(!world.seen().iter().any(|s| s.path == "/-/oauth_token"), "no token was asked for");

    let TokenState::Have(token) = client(true).access_token().await else {
        panic!("the refresh should succeed");
    };
    assert_eq!(token, "AT-fresh");
    let exchange = world.seen().into_iter().find(|s| s.path == "/-/oauth_token").unwrap();
    assert_eq!(form(&exchange.body, "client_secret").as_deref(), Some("s3cret-from-pass"));
}

/// Stores an expired Asana sign-in whose refresh token is `refresh`.
fn expired_sign_in(world: &World, refresh: &str) {
    TokenStore::new(world.store_path.clone())
        .put(
            "asana",
            StoredToken {
                access_token: "AT-old".into(),
                refresh_token: Some(refresh.into()),
                expires_at_secs: Some(1),
                client_id: Some("1234".into()),
                client_secret: None,
                token_endpoint: Some(format!("{}/-/oauth_token", world.authorization)),
                resource: Some(format!("{}/v2/mcp", world.host)),
            },
        )
        .unwrap();
}

/// A refresh whose secret does not resolve says so, and does not send the reader to sign in again
/// — that would fail on the same missing variable. Nothing is asked of the server, and the stored
/// sign-in is kept for when the variable is back.
#[tokio::test]
async fn a_refresh_that_cannot_be_asked_says_why_and_not_sign_in() {
    let world = world(Host::Asana).await;
    expired_sign_in(&world, "RT-old");
    let client = OAuthClient::new(
        "asana".into(),
        format!("{}/v2/mcp", world.host),
        OAuthConfig {
            client_id: Some("1234".into()),
            client_secret: Some("${LUXE_TEST_REFRESH_SECRET_UNSET}".into()),
            ..Default::default()
        },
    )
    .with_store(TokenStore::new(world.store_path.clone()));
    let TokenState::Unavailable(said) = client.access_token().await else {
        panic!("a secret that did not resolve is not a sign-in to redo");
    };
    assert!(said.contains("could not refresh"), "{said}");
    assert!(said.contains("LUXE_TEST_REFRESH_SECRET_UNSET is not set"), "names it: {said}");
    assert!(!said.contains("luxe mcp login"), "{said}");
    assert!(world.seen().is_empty(), "nothing was asked of the server");
    let kept = TokenStore::new(world.store_path.clone()).get("asana").expect("still stored");
    assert_eq!(kept.refresh_token.as_deref(), Some("RT-old"));
}

/// A refresh the server refuses is the one case where signing in again is the fix, and the
/// message carries the server's own reason.
#[tokio::test]
async fn a_refused_refresh_sends_the_reader_to_sign_in_with_the_reason() {
    let world = world(Host::Asana).await;
    expired_sign_in(&world, "RT-revoked");
    let TokenState::NeedsLogin(said) = world.client(None).access_token().await else {
        panic!("a refused refresh needs a new sign-in");
    };
    assert!(said.contains("invalid_grant — The refresh token is invalid"), "{said}");
    assert!(said.contains("luxe mcp login --id asana"), "{said}");
}

/// A token request refused outside a refresh reports the server's reason as it did before the
/// two kinds of failure were told apart.
#[tokio::test]
async fn a_refused_token_request_keeps_the_servers_reason() {
    std::env::set_var("LUXE_TEST_ASANA_SECRET", "s3cret-from-env");
    let log: Log = Arc::default();
    let authorization = spawn_authorization_server_with(log.clone(), None, true).await;
    let host = spawn_mcp_host(log.clone(), authorization, Host::Asana).await;
    let dir = tempfile::tempdir().unwrap();
    let client = OAuthClient::new(
        "machine".into(),
        format!("{host}/v2/mcp"),
        OAuthConfig {
            grant_type: OAuthGrant::ClientCredentials,
            client_id: Some("revoked".into()),
            client_secret: Some("${LUXE_TEST_ASANA_SECRET}".into()),
            ..Default::default()
        },
    )
    .with_store(TokenStore::new(dir.path().join("t.json")));
    let TokenState::NeedsLogin(said) = client.access_token().await else {
        panic!("a refused client gets no token");
    };
    assert_eq!(said, "token endpoint error: invalid_client — Unknown client");
}

/// A remembered registration with no token is not a token, and the way out is named correctly.
#[tokio::test]
async fn an_empty_token_means_sign_in() {
    let world = world(Host::Asana).await;
    TokenStore::new(world.store_path.clone())
        .put(
            "asana",
            StoredToken {
                access_token: String::new(),
                refresh_token: None,
                expires_at_secs: None,
                client_id: Some("registered".into()),
                client_secret: None,
                token_endpoint: None,
                resource: None,
            },
        )
        .unwrap();
    let TokenState::NeedsLogin(said) = world.client(None).access_token().await else {
        panic!("an empty token must not be sent");
    };
    assert!(said.contains("luxe mcp login --id asana"), "{said}");
}

/// A secret that names an unset variable is an error saying so, not an empty secret sent along.
#[tokio::test]
async fn a_secret_naming_an_unset_variable_says_so() {
    let world = world(Host::Asana).await;
    let client = OAuthClient::new(
        "asana".into(),
        format!("{}/v2/mcp", world.host),
        OAuthConfig {
            client_id: Some("1234".into()),
            client_secret: Some("${LUXE_TEST_NOT_SET_ANYWHERE}".into()),
            ..Default::default()
        },
    )
    .with_store(TokenStore::new(world.store_path.clone()));
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("LUXE_TEST_NOT_SET_ANYWHERE is not set"), "{error}");
}

/// The server's own pointer is followed wherever it points, even where no convention would look.
#[tokio::test]
async fn the_pointer_is_followed_wherever_it_points() {
    let world = world(Host::PointerOnly).await;
    let url = world.client(None).auth_start("http://127.0.0.1:9/callback").await.expect("starts");
    assert!(url.starts_with(&format!("{}/-/oauth_authorize?", world.authorization)), "{url}");
    assert!(world.seen().iter().any(|s| s.path == "/meta/unusual"));
}

/// An authorization server reached over plain `http` anywhere but this machine is refused before
/// anything is fetched from it.
#[tokio::test]
async fn a_plain_http_authorization_server_is_refused() {
    let log: Log = Arc::default();
    let host = spawn_mcp_host(log.clone(), "http://auth.example.test".into(), Host::Asana).await;
    let dir = tempfile::tempdir().unwrap();
    let client = asana_client(&host, TokenStore::new(dir.path().join("t.json")), None);
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("is not https"), "{error}");
}

/// What an authorization server describes is checked as well as who it is: an endpoint a code or a
/// token would go to must be https or this machine, and a document must name its issuer.
#[tokio::test]
async fn an_authorization_servers_endpoints_must_be_secure_and_its_own() {
    for (issued, token, refusal) in [
        (true, "http://evil.example/t", "not https"),
        (false, "https://a.test/t", "names no issuer"),
    ] {
        let log: Log = Arc::default();
        let authorization = serve(log.clone(), move |base, seen| match seen.path.as_str() {
            "/.well-known/oauth-authorization-server" => ok(serde_json::json!({
                "issuer": issued.then_some(base),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": token,
            })),
            _ => not_found(),
        })
        .await;
        let host = spawn_mcp_host(log.clone(), authorization, Host::Asana).await;
        let dir = tempfile::tempdir().unwrap();
        let client = asana_client(&host, TokenStore::new(dir.path().join("t.json")), None);
        let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
        assert!(error.contains(refusal), "{error}");
    }
}

/// An endpoint written in the config gets the same check as a discovered one.
#[tokio::test]
async fn a_configured_plain_http_endpoint_is_refused() {
    let world = world(Host::Asana).await;
    let config = OAuthConfig {
        authorization_endpoint: Some("http://sign-in.example.test/authorize".into()),
        token_endpoint: Some("https://sign-in.example.test/token".into()),
        ..Default::default()
    };
    let client = OAuthClient::new("asana".into(), format!("{}/v2/mcp", world.host), config)
        .with_store(TokenStore::new(world.store_path.clone()));
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("not https"), "{error}");
}

/// A server with no protected-resource metadata still signs in the old way — its origin's
/// metadata and dynamic registration — and a registration that fails says so.
#[tokio::test]
async fn a_server_without_resource_metadata_registers_the_old_way() {
    let world = world(Host::Legacy).await;
    let client =
        OAuthClient::new("legacy".into(), format!("{}/v2/mcp", world.host), OAuthConfig::default())
            .with_store(TokenStore::new(world.store_path.clone()));
    let url = client.auth_start("http://127.0.0.1:9/callback").await.expect("starts");
    assert!(url.starts_with(&format!("{}/retired-authorize?", world.host)), "{url}");
    assert_eq!(query(&url, "client_id").as_deref(), Some("registered-id"));
    assert_eq!(query(&url, "resource"), Some(format!("{}/v2/mcp", world.host)), "the server URL");

    let broken = world_with(Host::LegacyBrokenRegistration).await;
    let client = OAuthClient::new(
        "legacy".into(),
        format!("{}/v2/mcp", broken.host),
        OAuthConfig::default(),
    )
    .with_store(TokenStore::new(broken.store_path.clone()));
    let error = client.auth_start(CALLBACK).await.unwrap_err().to_string();
    assert!(error.contains("registration returned HTTP 500"), "{error}");
}

/// An endpoint written in the config is used, and discovery fills in the rest.
#[tokio::test]
async fn a_configured_endpoint_is_used_and_discovery_fills_the_rest() {
    let world = world(Host::Asana).await;
    let client = OAuthClient::new(
        "asana".into(),
        format!("{}/v2/mcp", world.host),
        OAuthConfig {
            client_id: Some("1234".into()),
            client_secret: Some("${LUXE_TEST_ASANA_SECRET}".into()),
            authorization_endpoint: Some("https://sign-in.example.test/authorize".into()),
            ..Default::default()
        },
    )
    .with_store(TokenStore::new(world.store_path.clone()));
    let url = client.auth_start("http://127.0.0.1:9/callback").await.expect("starts");
    assert!(url.starts_with("https://sign-in.example.test/authorize?"), "{url}");
    assert!(world.seen().iter().any(|s| s.path == "/.well-known/oauth-authorization-server"));
}

/// A machine-to-machine server that describes only a token endpoint is enough for client
/// credentials, found through its protected-resource metadata.
#[tokio::test]
async fn client_credentials_needs_only_a_token_endpoint() {
    std::env::set_var("LUXE_TEST_ASANA_SECRET", "s3cret-from-env");
    let log: Log = Arc::default();
    let authorization = spawn_authorization_server_with(log.clone(), None, true).await;
    let host = spawn_mcp_host(log.clone(), authorization, Host::Asana).await;
    let dir = tempfile::tempdir().unwrap();
    let client = OAuthClient::new(
        "machine".into(),
        format!("{host}/v2/mcp"),
        OAuthConfig {
            grant_type: OAuthGrant::ClientCredentials,
            client_id: Some("1234".into()),
            client_secret: Some("${LUXE_TEST_ASANA_SECRET}".into()),
            ..Default::default()
        },
    )
    .with_store(TokenStore::new(dir.path().join("t.json")));
    let TokenState::Have(token) = client.access_token().await else {
        panic!("client credentials should get a token");
    };
    assert_eq!(token, "AT-fresh");
    let request = log.lock().unwrap().iter().find(|s| s.path == "/-/oauth_token").cloned().unwrap();
    assert_eq!(form(&request.body, "grant_type").as_deref(), Some("client_credentials"));
    assert_eq!(form(&request.body, "client_secret").as_deref(), Some("s3cret-from-env"));
    assert_eq!(form(&request.body, "resource"), Some(format!("{host}/v2/mcp")));
}
