use super::*;
use crate::mcp_client::McpTransport;

#[tokio::test]
async fn connect_rejects_http_transport() {
    let config = McpServerConfig {
        id: "remote".to_string(),
        name: "Remote".to_string(),
        transport: McpTransport::HttpSse { url: "http://localhost/mcp".to_string() },
        enabled: true,
        ..Default::default()
    };
    assert!(matches!(McpStdioClient::connect(&config).await, Err(McpStdioError::NotStdio(_))));
}

/// End-to-end against a tiny Python MCP server that supports initialize + tools/list +
/// tools/call over newline-delimited JSON-RPC. Skipped when python3 is unavailable.
#[tokio::test]
async fn stdio_end_to_end_with_python_server() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not available; skipping MCP stdio e2e");
        return;
    }

    let script = r#"
import sys, json
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"t","version":"1"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        args = msg.get("params",{}).get("arguments",{})
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"got:"+json.dumps(args)}],"isError":False}})
    else:
        if mid is not None:
            send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"method not found"}})
"#;

    let config = McpServerConfig {
        id: "py".to_string(),
        name: "Py".to_string(),
        transport: McpTransport::Stdio {
            command: "python3".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    };

    let client = McpStdioClient::connect(&config).await.expect("connect");
    let tools = client.list_tools().await.expect("list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let result = client.call_tool("echo", json!({"x": 1})).await.expect("call");
    assert!(result.success);
    assert!(result.output.unwrap().contains("got:"));
}

/// Spawn a real child process and verify that per-server env vars reach it.
#[tokio::test]
async fn env_vars_are_passed_to_child() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not available; skipping env var child test");
        return;
    }

    // A minimal MCP server that echoes back the LUXE_TEST_TOKEN env var in every tools/call.
    let script = r#"
import sys, json, os
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"t","version":"1"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"getenv","description":"getenv","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        token = os.environ.get("LUXE_TEST_TOKEN", "NOT_SET")
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"token="+token}],"isError":False}})
    else:
        if mid is not None:
            send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"method not found"}})
"#;

    // Set a luxe-level env var that the value references.
    std::env::set_var("LUXE_TEST_REAL_TOKEN", "supersecret");

    let mut env = std::collections::HashMap::new();
    // The value uses `${VAR}` interpolation against Luxe's env.
    env.insert("LUXE_TEST_TOKEN".to_string(), "${LUXE_TEST_REAL_TOKEN}".to_string());

    let config = McpServerConfig {
        id: "envtest".to_string(),
        name: "EnvTest".to_string(),
        transport: McpTransport::Stdio {
            command: "python3".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env,
        },
        enabled: true,
        ..Default::default()
    };

    let client = McpStdioClient::connect(&config).await.expect("connect");
    let result = client.call_tool("getenv", json!({})).await.expect("call");
    assert!(result.success, "call failed: {:?}", result.error);
    let output = result.output.unwrap();
    assert_eq!(output, "token=supersecret", "env var not propagated: {output}");

    std::env::remove_var("LUXE_TEST_REAL_TOKEN");
}

/// A server that writes a line past the bound is refused, not buffered without limit.
///
/// The break this catches: `read_line` grew its buffer for as long as an untrusted server kept
/// writing without a newline.
#[tokio::test]
async fn a_line_past_the_bound_is_refused() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys
sys.stdin.readline()
sys.stdout.write("x" * (65 * 1024 * 1024)); sys.stdout.flush()
sys.stdin.read()
"#;
    let config = python_server("long", script);
    let connect = McpStdioClient::connect(&config);
    let refused = tokio::time::timeout(std::time::Duration::from_secs(10), connect).await;
    let error = refused.expect("refused, not buffered forever").err().expect("refused");
    assert!(error.to_string().contains("longer than"), "{error}");
}

/// A reply of a couple of megabytes is read, not refused.
///
/// The break this catches: lines were bounded at 1 MiB, so a tool result carrying one screenshot as
/// base64 — one JSON line — failed on a server that had always worked. The HTTP transport never had
/// the bound, and language servers' frames get 64 MiB.
#[tokio::test]
async fn a_reply_of_a_couple_of_megabytes_is_read() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys, json
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})
    elif msg.get("method") == "tools/call":
        big = "A" * (3 * 1024 * 1024)
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"content":[{"type":"text","text":big}]}})
"#;
    let client = McpStdioClient::connect(&python_server("big", script)).await.expect("connects");
    let result = client.call_tool("shot", json!({})).await.expect("the reply is read");
    assert!(result.success && result.output.is_some_and(|text| text.starts_with("AAAA")));
}

/// A line past the bound ends the connection: what is left of it in the pipe would be read as
/// the next reply.
#[tokio::test]
async fn a_line_past_the_bound_closes_the_connection() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys, json
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})
    elif msg.get("method") == "tools/list":
        sys.stdout.write("x" * (65 * 1024 * 1024)); sys.stdout.flush()
"#;
    let client = McpStdioClient::connect(&python_server("long", script)).await.expect("connects");
    assert!(client.list_tools().await.is_err());
    assert!(crate::mcp_client::McpConnection::is_closed(&client), "a fresh server comes next");
}

/// Stderr that is not UTF-8 is read past, so the server keeps answering; and one very long stderr
/// line costs the error a bounded tail, not the whole line.
///
/// The break this catches: the drain stopped at the first byte that was not UTF-8, closing the
/// pipe under a server whose next write then failed, and kept each line whole.
#[tokio::test]
async fn any_stderr_is_drained_and_kept_short() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys, json
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    sys.stderr.buffer.write(b"\xff\xfe not utf-8\n" * 1000); sys.stderr.flush()
    if msg.get("method") == "initialize":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})
    elif msg.get("method") == "tools/list":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"tools":[]}})
    elif msg.get("method") == "tools/call":
        sys.stderr.write("y" * (3 * 1024 * 1024)); sys.stderr.flush()
        sys.exit(1)
"#;
    let client = McpStdioClient::connect(&python_server("noisy", script)).await.expect("connects");
    assert!(client.list_tools().await.is_ok(), "still answering after bytes that are not text");
    let Err(error) = client.call_tool("x", json!({})).await else { panic!("it exits") };
    assert!(error.to_string().len() < 30_000, "a bounded tail: {}", error.to_string().len());
}

/// `initialize` offers the protocol version Luxe speaks.
#[tokio::test]
async fn initialize_offers_the_current_protocol_version() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys, json
offered = ""
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        offered = msg["params"]["protocolVersion"]
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})
    elif msg.get("method") == "tools/call":
        send({"jsonrpc":"2.0","id":msg["id"],"result":{"content":[{"type":"text","text":offered}]}})
"#;
    let client = McpStdioClient::connect(&python_server("v", script)).await.expect("connects");
    let result = client.call_tool("version", json!({})).await.expect("answers");
    assert_eq!(result.output.as_deref(), Some(crate::mcp_client::MCP_PROTOCOL_VERSION));
}

/// A server that dies in the handshake says why: its last stderr lines ride on the error.
///
/// The break this catches: stderr went to the null device, so a server that could not start
/// reported only "server closed the connection".
#[tokio::test]
async fn a_server_that_dies_says_what_it_printed() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let script = r#"
import sys
for n in range(30):
    sys.stderr.write(f"line {n}\n")
sys.stderr.write("missing API key\n"); sys.stderr.flush()
sys.exit(1)
"#;
    let error =
        McpStdioClient::connect(&python_server("dies", script)).await.err().expect("closed");
    let said = error.to_string();
    assert!(said.contains("missing API key"), "{said}");
    assert!(!said.contains("line 0\n"), "only the last lines: {said}");
}

fn python_server(id: &str, script: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        name: id.to_string(),
        transport: McpTransport::Stdio {
            command: "python3".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    }
}

/// A caller of a connection branches on the kind of failure, whichever transport it came from,
/// and still shows the text this transport wrote.
#[test]
fn a_stdio_error_is_sorted_by_what_went_wrong_and_keeps_its_text() {
    use crate::mcp_client::McpError;

    let rpc = McpError::from(McpStdioError::Rpc("boom".into()));
    assert!(matches!(&rpc, McpError::Rpc(text) if text == "server returned an error: boom"));
    let protocol = McpError::from(McpStdioError::Protocol("a message longer than 9 bytes".into()));
    assert!(matches!(protocol, McpError::Protocol(_)));
    let closed = McpError::from(McpStdioError::Closed("it died".into()));
    assert!(matches!(&closed, McpError::Closed(text) if text.contains("it last said:\nit died")));
    let spawn = McpError::from(McpStdioError::Spawn("no such file".into()));
    assert!(matches!(&spawn, McpError::Transport(text) if text == "spawn failed: no such file"));
}
