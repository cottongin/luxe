//! End-to-end HTTP transport test against a hermetic raw-TCP mock MCP server.
//!
//! Validates the Streamable-HTTP happy path — `initialize` handshake, `Mcp-Session-Id` capture +
//! echo, bearer auth header, `tools/list`, `tools/call` — without any external server or HTTP
//! framework. The mock answers `application/json` (the non-SSE branch of the transport).

use std::sync::Arc;

use luxe_core::mcp_client::{McpAuth, McpServerConfig, McpTransport};
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use luxe_mcp::{McpProxyTool, ServerRegistry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

/// Spawn a mock MCP-over-HTTP server; returns its base URL. It answers JSON-RPC over HTTP/1.1,
/// issues a session id on initialize, and echoes the auth header + session id from a tool call.
async fn spawn_mock() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // Read headers.
                let head_end = loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let content_len = header_value(&head, "content-length")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let auth = header_value(&head, "authorization").unwrap_or_default();
                let session = header_value(&head, "mcp-session-id").unwrap_or_default();
                while buf.len() < head_end + content_len {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body =
                    String::from_utf8_lossy(&buf[head_end..head_end + content_len]).to_string();
                let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let id = msg.get("id").cloned();
                let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

                // Notifications carry no id — just acknowledge.
                let Some(id) = id else {
                    let _ = socket
                        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                };

                let result = match method {
                    "initialize" => serde_json::json!({
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "serverInfo": { "name": "mock", "version": "1" }
                    }),
                    "tools/list" => serde_json::json!({ "tools": [
                        { "name": "echo", "description": "Echo back.",
                          "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } } } }
                    ]}),
                    "tools/call" => {
                        let m = msg
                            .get("params")
                            .and_then(|p| p.get("arguments"))
                            .and_then(|a| a.get("message"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        serde_json::json!({ "content": [
                            { "type": "text", "text": format!("auth={auth} sess={session} msg={m}") }
                        ], "isError": false })
                    }
                    _ => serde_json::json!({}),
                };
                let rpc = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                let payload = serde_json::to_string(&rpc).unwrap();
                // Issue a session id on initialize (the client must echo it afterward).
                let extra = if method == "initialize" { "Mcp-Session-Id: sess-1\r\n" } else { "" };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/mcp")
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_value(head: &str, name: &str) -> Option<String> {
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
        session_id: "test".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn text_of(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|b| match b {
            luxe_core::message::ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn http_transport_discovers_and_calls_with_bearer_and_session() {
    let url = spawn_mock().await;
    let config = McpServerConfig {
        id: "srv".into(),
        name: "Srv".into(),
        transport: McpTransport::HttpSse { url },
        enabled: true,
        auth: Some(McpAuth::Bearer { token: Some("testtoken".into()), token_env: None }),
        ..Default::default()
    };
    let manager = Arc::new(ServerRegistry::new(vec![config], None));
    let proxy = McpProxyTool::new(manager);

    let hits = text_of(&proxy.run(serde_json::json!({ "search": "echo" }), &cx()).await);
    assert!(hits.contains("srv/echo"), "search over http server: {hits}");

    let called = proxy
        .run(serde_json::json!({ "tool": "srv/echo", "args": "{\"message\":\"hi\"}" }), &cx())
        .await;
    assert!(!called.is_error, "call errored: {}", text_of(&called));
    let body = text_of(&called);
    assert!(body.contains("msg=hi"), "tool ran: {body}");
    assert!(body.contains("auth=Bearer testtoken"), "bearer header sent: {body}");
    assert!(body.contains("sess=sess-1"), "session id echoed on later requests: {body}");
}

/// What the expiring mock saw: each request's method, session and protocol-version header.
type Seen = Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

/// A mock that negotiates `2025-03-26`, issues a new session per `initialize`, and answers 404 to
/// the first `tools/list` that carries the first session, as a server that expired it does.
async fn spawn_expiring_mock(seen: Seen) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sessions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let (seen, sessions) = (seen.clone(), sessions.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let len = header_value(&head, "content-length")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let msg: serde_json::Value =
                    serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or_default();
                let method = msg["method"].as_str().unwrap_or("").to_string();
                let session = header_value(&head, "mcp-session-id").unwrap_or_default();
                let version = match msg["params"]["protocolVersion"].as_str() {
                    Some(offered) => format!("offered {offered}"),
                    None => header_value(&head, "mcp-protocol-version").unwrap_or_default(),
                };
                seen.lock().unwrap().push((method.clone(), session.clone(), version));
                let Some(id) = msg.get("id").cloned() else {
                    let _ = socket.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                };
                if method == "tools/list" && session == "sess-1" {
                    let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let (result, extra) = match method.as_str() {
                    "initialize" => {
                        let n = sessions.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        let result = serde_json::json!({ "protocolVersion": "2025-03-26",
                            "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } });
                        (result, format!("Mcp-Session-Id: sess-{n}\r\n"))
                    }
                    _ => (serde_json::json!({ "tools": [] }), String::new()),
                };
                let rpc = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                let payload = serde_json::to_string(&rpc).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/mcp")
}

/// A session the server expired is started again, once, and the request goes through on the new
/// one; every request after `initialize` names the version the server agreed to.
///
/// The break this catches: a 404 for an expired session failed every later call until Luxe was
/// restarted, and the transport offered a version older than Streamable HTTP, ignored the one the
/// server answered with and never sent the header the spec asks for.
#[tokio::test]
async fn a_404_with_a_session_reinitializes_once() {
    let seen: Seen = Arc::default();
    let url = spawn_expiring_mock(seen.clone()).await;
    let config = McpServerConfig {
        id: "exp".into(),
        name: "Exp".into(),
        transport: McpTransport::HttpSse { url },
        enabled: true,
        ..Default::default()
    };
    let connection = luxe_mcp::http::HttpConnection::connect(&config).await.expect("connects");
    let tools = luxe_core::mcp_client::McpConnection::list_tools(&connection).await;
    assert!(tools.is_ok(), "the request went through on a new session: {tools:?}");

    let seen = seen.lock().unwrap().clone();
    let methods: Vec<&str> = seen.iter().map(|(method, ..)| method.as_str()).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "initialize",
            "notifications/initialized",
            "tools/list"
        ],
    );
    let offered = format!("offered {}", luxe_core::mcp_client::MCP_PROTOCOL_VERSION);
    assert_eq!(seen[0].2, offered, "initialize offers the version Luxe speaks");
    assert_eq!(seen[5], ("tools/list".into(), "sess-2".into(), "2025-03-26".into()));
}
