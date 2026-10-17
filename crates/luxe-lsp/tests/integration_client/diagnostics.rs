//! Diagnostics — pushed and pulled, through the client and through the tool.

use super::*;

/// Integration test that drives `get_diagnostics` against the did_open stub.
///
/// Verifies the settle-wait path: client sends didOpen, polls for publishDiagnostics,
/// parses the response, and returns diagnostics.
#[cfg(test)]
#[tokio::test]
async fn get_diagnostics_returns_stub_diagnostic() {
    // Write the did_open stub (initialize -> initialized -> didOpen -> publishDiagnostics).
    let scratch = tempfile::tempdir().expect("temp dir");
    let stub = scratch.path().join("did_open_stub.py");
    write_did_open_stub(&stub);

    // Initialize the client.
    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "did-open-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(3),
    });
    client.initialize().await.expect("initialize failed");

    // Call get_diagnostics — this sends didOpen and polls for publishDiagnostics.
    let diagnostics = client
        .get_diagnostics("file:///tmp/test.rs", "fn main() {}")
        .await
        .expect("get_diagnostics failed");

    // Assert the canned diagnostic is returned.
    assert_eq!(diagnostics.len(), 1, "expected exactly one diagnostic");
    assert_eq!(diagnostics[0].message, "Test diagnostic");
}

/// Integration test for `LspDiagnosticsTool` end-to-end against a stub server.
///
/// Creates a temp dir with Cargo.toml + a Rust file, writes a Python stub that mimics
/// rust-analyzer's behavior, and verifies the tool returns formatted diagnostics.
#[tokio::test]
async fn lsp_diagnostics_tool_e2e() {
    // Create a temp dir with Cargo.toml + a Rust file.
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::create_dir_all(tmp_dir.join("src")).unwrap();
    std::fs::write(
        tmp_dir.join("Cargo.toml"),
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(tmp_dir.join("src/main.rs"), "fn main() { println!(\"hello\"); }").unwrap();

    // Write a Python stub that mimics rust-analyzer's behavior.
    let stub = tmp_dir.join("rust-analyzer");
    let stub_script = r#"#!/usr/bin/env python3
import json, sys

def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    body = sys.stdin.buffer.read(n)
    return json.loads(body)

def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None: break
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"capabilities":{}}})
    elif method == "textDocument/didOpen":
        pass  # No response needed for notifications
    elif method == "textDocument/publishDiagnostics":
        pass  # No response needed for notifications
    elif method == "shutdown":
        write_msg({"jsonrpc":"2.0","id":mid,"result":None})
    elif method == "exit":
        break
"#;
    std::fs::write(&stub, stub_script).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Create a ToolContext pointing to the temp dir.
    use luxe_core::{ContentBlock, ToolContext};
    use tokio::sync::{mpsc, watch};
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let cx = ToolContext {
        cwd: tmp_dir.clone(),
        session_id: "test-session".to_string(),
        cancelled: std::sync::Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    };

    // Call the tool.
    use luxe_core::Tool;
    use luxe_lsp::LspDiagnosticsTool;
    let tool = LspDiagnosticsTool::new(service_for_stub(&stub));
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust"
    });
    let result = tool.run(args, &cx).await;

    // Assert the result is not an error.
    assert!(!result.is_error, "tool returned error: {:?}", result.content);

    assert_eq!(result.content, vec![ContentBlock::Text("No diagnostics for src/main.rs\n".into())]);

    let missing = tool.run(serde_json::json!({"file": "src/missing.rs"}), &cx).await;
    assert!(missing.is_error);
    let ContentBlock::Text(text) = &missing.content[0] else {
        panic!("expected Text block");
    };
    assert!(text.starts_with("src/missing.rs: read "), "{text}");
}

/// Losing a server warning, its severity, or its position must not read as a clean file.
#[tokio::test]
async fn lsp_diagnostics_tool_formats_server_diagnostics() {
    // Create a temp dir with Cargo.toml + a Rust file.
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::create_dir_all(tmp_dir.join("src")).unwrap();
    std::fs::write(
        tmp_dir.join("Cargo.toml"),
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(tmp_dir.join("src/main.rs"), "fn main() { println!(\"hello\"); }").unwrap();

    // Write a Python stub that mimics rust-analyzer's behavior.
    let stub = tmp_dir.join("rust-analyzer");
    let stub_script = r#"#!/usr/bin/env python3
import json, sys

def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    body = sys.stdin.buffer.read(n)
    return json.loads(body)

def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None: break
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"capabilities":{}}})
    elif method == "textDocument/didOpen":
        write_msg({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
            "uri":msg["params"]["textDocument"]["uri"],
            "diagnostics":[{"severity":2,"range":{"start":{"line":2,"character":3},"end":{"line":2,"character":4}},"message":"STUB_DIAGNOSTIC"}]}})
    elif method == "shutdown":
        write_msg({"jsonrpc":"2.0","id":mid,"result":None})
    elif method == "exit":
        break
"#;
    std::fs::write(&stub, stub_script).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    use luxe_lsp::LspDiagnosticsTool;
    let tool = LspDiagnosticsTool::new(service_for_stub(&stub));

    // Create a ToolContext pointing to the temp dir.
    use luxe_core::{ContentBlock, ToolContext};
    use tokio::sync::{mpsc, watch};
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let cx = ToolContext {
        cwd: tmp_dir.clone(),
        session_id: "test-session".to_string(),
        cancelled: std::sync::Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    };

    // Call the tool.
    use luxe_core::Tool;
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust"
    });
    let result = tool.run(args, &cx).await;

    // Assert the result is not an error.
    assert!(!result.is_error, "tool returned error: {:?}", result.content);

    assert_eq!(
        result.content,
        vec![ContentBlock::Text(
            "Diagnostics for src/main.rs (1 found):\n1. [WARN] 3:4: STUB_DIAGNOSTIC\n".into()
        )]
    );
}

/// A server that answers `textDocument/diagnostic` and never publishes.
///
/// The shape TypeScript 7's native LSP ships: it advertises `diagnosticProvider` — but only when the
/// client says it can ask — and then says nothing at all about an open file. The stub mirrors both
/// halves, so a client that stops declaring the capability sees a push-only server and finds
/// nothing, which is exactly the silence this is here to catch.
///
/// Three answers in order, driving the three cases a pull read has: a `full` report, an `unchanged`
/// one that means "my last answer still stands", and a `full` report that clears it.
fn write_pull_stub(path: &std::path::Path) {
    let script = r#"
import json, sys

def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    return json.loads(sys.stdin.buffer.read(n))

def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

def item(message):
    return {
        "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 8}},
        "severity": 1, "source": "stub", "message": message,
    }

asked = 0
while True:
    msg = read_msg()
    if msg is None: break
    method, mid = msg.get("method"), msg.get("id")
    if method == "initialize":
        caps = msg["params"]["capabilities"]["textDocument"]
        pull = "diagnostic" in caps
        server = {"diagnosticProvider": {"identifier": "stub"}} if pull else {}
        write_msg({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": server}})
    elif method == "textDocument/diagnostic":
        asked += 1
        if asked == 1:
            write_msg({"jsonrpc": "2.0", "id": mid,
                       "result": {"kind": "full", "resultId": "r1", "items": [item("pulled once")]}})
        elif asked == 2:
            assert msg["params"].get("previousResultId") == "r1", msg["params"]
            write_msg({"jsonrpc": "2.0", "id": mid,
                       "result": {"kind": "unchanged", "resultId": "r2"}})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid,
                       "result": {"kind": "full", "resultId": "r3", "items": []}})
    elif method == "shutdown":
        write_msg({"jsonrpc": "2.0", "id": mid, "result": None})
    elif method == "exit":
        break
"#;
    std::fs::write(path, script).expect("write pull stub");
}

/// A pull-only server's diagnostics reach the caller, and stay current across edits.
///
/// The defect this pins: a server that publishes nothing was reported as a clean file, which is
/// indistinguishable from a real one. TypeScript 7's LSP shipped that way for every `.ts` file.
#[tokio::test]
async fn a_server_that_only_answers_when_asked_is_asked() {
    let scratch = tempfile::tempdir().expect("temp dir");
    let stub = scratch.path().join("pull_stub.py");
    write_pull_stub(&stub);
    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "pull-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(5),
    });
    client.initialize().await.expect("initialize failed");

    let uri = "file:///tmp/pull.ts";
    let first = client.get_diagnostics(uri, "const a: number = 'x';").await.expect("first read");
    assert_eq!(first.len(), 1, "a pull-only server's diagnostics must reach the caller: {first:?}");
    assert_eq!(first[0].message, "pulled once");

    // An unchanged document is answered from the cache without asking again.
    let warm = client.get_diagnostics(uri, "const a: number = 'x';").await.expect("warm read");
    assert_eq!(warm.len(), 1, "the warm read reuses the answer: {warm:?}");

    // The document changed, and the server says its last answer still stands.
    let unchanged = client.get_diagnostics(uri, "const a: number = 'y';").await.expect("read");
    assert_eq!(unchanged.len(), 1, "`unchanged` keeps the previous items: {unchanged:?}");
    assert_eq!(unchanged[0].message, "pulled once");

    // And a fresh full report replaces them, rather than the first answer sticking forever.
    let cleared = client.get_diagnostics(uri, "const a: number = 1;").await.expect("read");
    assert!(cleared.is_empty(), "a later full report is the answer now: {cleared:?}");

    client.shutdown().await.ok();
}

/// A server that both publishes and offers to be asked is not asked.
///
/// rust-analyzer is that server, and its pushed set is cargo's — the one a reader means. Asking as
/// well would quietly swap it for a second opinion computed a different way, and cost a round trip
/// per file to do it — so the assertion is that the request was never sent, not merely that its
/// answer was discarded.
#[tokio::test]
async fn a_server_that_published_is_not_asked_again() {
    let scratch = tempfile::tempdir().expect("temp dir");
    let stub = scratch.path().join("push_and_pull_stub.py");
    let asked = scratch.path().join("was_asked");
    std::fs::write(
        &stub,
        r#"
import json, os, sys

def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    return json.loads(sys.stdin.buffer.read(n))

def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None: break
    method, mid = msg.get("method"), msg.get("id")
    if method == "initialize":
        write_msg({"jsonrpc": "2.0", "id": mid,
                   "result": {"capabilities": {"diagnosticProvider": {"identifier": "both"}}}})
    elif method == "textDocument/didOpen":
        write_msg({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                   "params": {"uri": msg["params"]["textDocument"]["uri"], "diagnostics": []}})
    elif method == "textDocument/diagnostic":
        open(sys.argv[1], "w").close()
        write_msg({"jsonrpc": "2.0", "id": mid, "result": {"kind": "full", "items": [{
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "severity": 1, "source": "stub", "message": "second opinion"}]}})
    elif method == "shutdown":
        write_msg({"jsonrpc": "2.0", "id": mid, "result": None})
    elif method == "exit":
        break
"#,
    )
    .expect("write stub");
    let mut client = LspClient::new(LspConfig {
        command: vec![
            "python3".to_string(),
            stub.to_string_lossy().to_string(),
            asked.to_string_lossy().to_string(),
        ],
        server_id: "push-and-pull-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(5),
    });
    client.initialize().await.expect("initialize failed");

    let found = client.get_diagnostics("file:///tmp/both.rs", "fn main() {}").await.expect("read");
    assert!(found.is_empty(), "the published answer stands: {found:?}");
    assert!(!asked.exists(), "a server that published must not also be asked");

    client.shutdown().await.ok();
}
