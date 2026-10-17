//! OAuth e2e against a hermetic mock that serves `.well-known`, `/token`, and an auth-gated `/mcp`.
//!
//! Covers the two non-browser paths fully: client-credentials (automatic, through the proxy) and
//! the authorization-code token exchange (`auth_start` → simulated redirect → `auth_complete`).

use std::sync::Arc;

use luxe_core::mcp_client::{McpAuth, McpServerConfig, McpTransport, OAuthConfig, OAuthGrant};
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use luxe_mcp::oauth::OAuthClient;
use luxe_mcp::token_store::TokenStore;
use luxe_mcp::{McpProxyTool, ServerRegistry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

const ACCESS_TOKEN: &str = "AT-secret";

/// Mock server: `.well-known` discovery, `/token` (any grant → the same token), and `/mcp`
/// requiring `Authorization: Bearer AT-secret`. Returns its base URL.
async fn spawn_mock() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let base_for_task = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let base = base_for_task.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = find(&buf, b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let first = head.lines().next().unwrap_or("");
                let mut parts = first.split_whitespace();
                let _method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");
                let clen = header(&head, "content-length")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let auth = header(&head, "authorization").unwrap_or_default();
                while buf.len() < head_end + clen {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body =
                    String::from_utf8_lossy(&buf[head_end..(head_end + clen).min(buf.len())])
                        .to_string();

                let (status, ctype, payload) = if path.starts_with("/.well-known/") {
                    (
                        "200 OK",
                        "application/json",
                        serde_json::json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                            "registration_endpoint": format!("{base}/register"),
                        })
                        .to_string(),
                    )
                } else if path == "/token" {
                    (
                        "200 OK",
                        "application/json",
                        serde_json::json!({
                            "access_token": ACCESS_TOKEN,
                            "token_type": "Bearer",
                            "expires_in": 3600,
                            "refresh_token": "RT",
                        })
                        .to_string(),
                    )
                } else if path == "/mcp" {
                    if auth != format!("Bearer {ACCESS_TOKEN}") {
                        ("401 Unauthorized", "application/json", "{}".to_string())
                    } else {
                        let msg: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or_default();
                        let id = msg.get("id").cloned();
                        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
                        match id {
                            None => {
                                let _ = socket
                                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                                    .await;
                                return;
                            }
                            Some(id) => {
                                let result = match method {
                                    "initialize" => {
                                        serde_json::json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } })
                                    }
                                    "tools/list" => {
                                        serde_json::json!({ "tools": [ { "name": "ping", "description": "Ping.", "inputSchema": { "type": "object" } } ] })
                                    }
                                    "tools/call" => {
                                        serde_json::json!({ "content": [ { "type": "text", "text": "pong" } ], "isError": false })
                                    }
                                    _ => serde_json::json!({}),
                                };
                                (
                                    "200 OK",
                                    "application/json",
                                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
                                )
                            }
                        }
                    }
                } else {
                    ("404 Not Found", "application/json", "{}".to_string())
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    base
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}
fn header(head: &str, name: &str) -> Option<String> {
    head.lines()
        .find(|l| l.to_lowercase().starts_with(&format!("{name}:")))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}
fn cx() -> ToolContext {
    let (event_tx, _rx) = mpsc::channel(8);
    let (_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: std::path::PathBuf::from("."),
        session_id: "t".into(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}
fn text_of(r: &ToolResultData) -> String {
    r.content
        .iter()
        .filter_map(|b| match b {
            luxe_core::message::ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn client_credentials_flows_through_the_proxy() {
    // A token reached through the proxy is persisted by the default store, which resolves under
    // `$XDG_STATE_HOME` and takes over one left under `$XDG_CONFIG_HOME` — the *real* ones unless
    // this test moves both. Left alone, running the suite overwrites the reader's token store with
    // this mock's token, and a migration could move their real one into a temporary directory. Both
    // are redirected here, and asserted below so the redirect can't rot.
    let config_home = tempfile::tempdir().unwrap();
    let state_home = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", config_home.path());
    std::env::set_var("XDG_STATE_HOME", state_home.path());
    // either override would win over both, and point the store back at the real directories
    std::env::remove_var("LUXE_CONFIG_DIR");
    std::env::remove_var("LUXE_STATE_DIR");

    let base = spawn_mock().await;
    let config = McpServerConfig {
        id: "api".into(),
        name: "Api".into(),
        transport: McpTransport::HttpSse { url: format!("{base}/mcp") },
        enabled: true,
        auth: Some(McpAuth::OAuth(OAuthConfig {
            grant_type: OAuthGrant::ClientCredentials,
            client_id: Some("cid".into()),
            client_secret: Some("sec".into()),
            token_endpoint: Some(format!("{base}/token")),
            ..Default::default()
        })),
        ..Default::default()
    };
    let manager = Arc::new(ServerRegistry::new(vec![config], None));
    let proxy = McpProxyTool::new(manager);

    // Discovery-free client-credentials: the proxy call fetches a token then calls the tool.
    let called = proxy.run(serde_json::json!({ "tool": "api/ping", "args": "{}" }), &cx()).await;
    assert!(!called.is_error, "client_credentials call failed: {}", text_of(&called));
    assert!(text_of(&called).contains("pong"));

    let stored = state_home.path().join("luxe").join("mcp-auth.json");
    assert!(
        stored.exists(),
        "the token went somewhere other than the redirected state dir — which means it went to a \
         real one"
    );
    assert!(
        !config_home.path().join("luxe").join("mcp-auth.json").exists(),
        "and never to the config dir, which is where dotfile repositories live"
    );
}

#[tokio::test]
async fn authorization_code_exchange_stores_a_token() {
    let base = spawn_mock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path().join("mcp-auth.json"));
    let client = OAuthClient::new(
        "web".into(),
        format!("{base}/mcp"),
        OAuthConfig {
            grant_type: OAuthGrant::AuthorizationCode,
            client_id: Some("cid".into()),
            authorization_endpoint: Some(format!("{base}/authorize")),
            token_endpoint: Some(format!("{base}/token")),
            ..Default::default()
        },
    )
    .with_store(store);

    // Begin the flow → the authorize URL carries PKCE + state.
    let url = client.auth_start("http://127.0.0.1:9/callback").await.expect("auth_start");
    assert!(url.contains("code_challenge=") && url.contains("code_challenge_method=S256"));
    let parsed = url::Url::parse(&url).unwrap();
    let state =
        parsed.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned()).unwrap();

    // A mismatched state must be rejected (CSRF guard).
    assert!(client.auth_complete("http://127.0.0.1:9/callback?code=c&state=wrong").await.is_err());

    // Begin again (the first attempt consumed the pending state), then complete with the real state.
    let url = client.auth_start("http://127.0.0.1:9/callback").await.expect("auth_start");
    let state = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .unwrap_or(state);
    let redirect = format!("http://127.0.0.1:9/callback?code=testcode&state={state}");
    client.auth_complete(&redirect).await.expect("auth_complete");

    // Reading a fresh store confirms the exchanged token persisted.
    let stored =
        TokenStore::new(dir.path().join("mcp-auth.json")).get("web").expect("token stored");
    assert_eq!(stored.access_token, ACCESS_TOKEN);
}
