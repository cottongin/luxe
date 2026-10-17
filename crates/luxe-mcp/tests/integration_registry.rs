//! The registry starts only the server a call names, and a cold server once however many calls
//! arrive together. Python stdio servers that note each start in a marker file; skipped without
//! `python3`.

use std::sync::Arc;

use luxe_core::mcp_client::{McpServerConfig, McpTransport};
use luxe_mcp::ServerRegistry;

/// An echo server that appends a line to `$MARKER` each time it starts.
const COUNTED_SERVER: &str = r#"
import sys, json, os
open(os.environ["MARKER"], "a").write("started\n")
def send(m): sys.stdout.write(json.dumps(m)+"\n"); sys.stdout.flush()
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line); mid=msg.get("id"); method=msg.get("method")
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"n","version":"1"}}})
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo.","inputSchema":{"type":"object"}}]}})
    elif method=="tools/call":
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"ok"}],"isError":False}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"no"}})
"#;

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().is_ok()
}

fn server(dir: &std::path::Path, id: &str) -> McpServerConfig {
    let script = dir.join("server.py");
    std::fs::write(&script, COUNTED_SERVER).unwrap();
    let marker = dir.join(format!("{id}.started"));
    McpServerConfig {
        id: id.into(),
        name: id.into(),
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: None,
            env: [("MARKER".to_string(), marker.to_string_lossy().into_owned())].into(),
        },
        enabled: true,
        ..Default::default()
    }
}

fn starts(dir: &std::path::Path, id: &str) -> usize {
    std::fs::read_to_string(dir.join(format!("{id}.started")))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// Calling a tool on the second server must not start the first just to list its tools.
#[tokio::test]
async fn calling_one_server_never_starts_another() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let registry =
        ServerRegistry::new(vec![server(dir.path(), "first"), server(dir.path(), "second")], None);
    registry.call("second/echo", serde_json::json!({})).await.expect("the call runs");
    assert_eq!(starts(dir.path(), "second"), 1);
    assert_eq!(starts(dir.path(), "first"), 0, "the first server was started for nothing");
}

/// Two calls arriving together at a cold server start it once, and both are answered.
#[tokio::test]
async fn two_cold_calls_start_one_server() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(ServerRegistry::new(vec![server(dir.path(), "cold")], None));
    let (one, two) = tokio::join!(
        registry.call("cold/echo", serde_json::json!({})),
        registry.call("cold/echo", serde_json::json!({}))
    );
    assert!(one.is_ok() && two.is_ok(), "{one:?} {two:?}");
    assert_eq!(starts(dir.path(), "cold"), 1);
}

/// A bare tool name that two servers both offer is ambiguous, and says so rather than picking one.
#[tokio::test]
async fn an_ambiguous_bare_name_is_reported() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let registry =
        ServerRegistry::new(vec![server(dir.path(), "a"), server(dir.path(), "b")], None);
    let error = registry.call("echo", serde_json::json!({})).await.unwrap_err();
    assert!(matches!(error, luxe_mcp::RegistryError::Ambiguous { .. }), "{error:?}");
    let error = error.to_string();
    assert!(error.contains("a/echo") && error.contains("b/echo"), "{error}");
}
