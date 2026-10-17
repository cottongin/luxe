//! End-to-end proxy test against a real stdio MCP server (a tiny Python echo server).
//!
//! Exercises the whole Slice-A path: discovery via cached metadata, lazy connect on first call,
//! and a permission-gated `tools/call`. Skipped when `python3` is unavailable.

use std::sync::Arc;

use luxe_core::mcp_client::{McpServerConfig, McpTransport};
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use luxe_mcp::{McpProxyTool, ServerRegistry};
use tokio::sync::{mpsc, watch};

const ECHO_SERVER: &str = r#"
import sys, json
def send(m): sys.stdout.write(json.dumps(m)+"\n"); sys.stdout.flush()
TOOLS=[{"name":"echo","description":"Echo a message back.","inputSchema":{"type":"object","properties":{"message":{"type":"string"}}}}]
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line); mid=msg.get("id"); method=msg.get("method")
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"demo","version":"1"}}})
    elif method=="notifications/initialized": pass
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":TOOLS}})
    elif method=="tools/call":
        a=msg.get("params",{}).get("arguments",{})
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: "+str(a.get("message",""))}],"isError":False}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"method not found"}})
"#;

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().is_ok()
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
async fn proxy_discovers_and_calls_a_real_stdio_server() {
    if !python_available() {
        eprintln!("python3 not available; skipping MCP proxy e2e");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("echo_server.py");
    std::fs::write(&script, ECHO_SERVER).unwrap();
    let cache = dir.path().join("mcp-metadata.json");

    let config = McpServerConfig {
        id: "demo".into(),
        name: "Demo".into(),
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    };
    let manager = Arc::new(ServerRegistry::new(vec![config], Some(cache.clone())));
    let proxy = McpProxyTool::new(manager);

    // search → finds the tool from a freshly-listed (then cached) metadata snapshot.
    let hits = text_of(&proxy.run(serde_json::json!({ "search": "echo" }), &cx()).await);
    assert!(hits.contains("demo/echo"), "search should list the tool: {hits}");

    // describe → shows parameters without calling.
    let desc = text_of(&proxy.run(serde_json::json!({ "tool": "demo/echo" }), &cx()).await);
    assert!(desc.contains("Parameters") && desc.contains("message"), "describe: {desc}");

    // call → lazily connects and returns the server's result as data.
    let called = proxy
        .run(serde_json::json!({ "tool": "demo/echo", "args": "{\"message\":\"hi\"}" }), &cx())
        .await;
    assert!(!called.is_error);
    assert!(text_of(&called).contains("echo: hi"), "call result: {}", text_of(&called));

    // The metadata cache was persisted, so discovery survives without a live connection.
    assert!(cache.exists(), "metadata cache should be written");
    let cached: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    assert!(cached.get("demo").is_some(), "cache keyed by server id");
}
