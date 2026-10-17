//! Lifecycle — a stub server answers a handshake, hangs, or publishes on open.

use super::*;

#[tokio::test]
async fn hover_and_definition_round_trip() {
    let stub = stub_path();
    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "test-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(5),
    });

    // Initialize.
    let init = timeout(Duration::from_secs(3), client.initialize()).await;
    assert!(init.is_ok(), "initialize() TIMED OUT");
    init.unwrap().expect("initialize() returned Err");

    // Hover round-trip.
    let hover = timeout(
        Duration::from_secs(2),
        client.hover("file:///tmp/test.rs", Position { line: 0, character: 0 }),
    )
    .await;
    assert!(hover.is_ok(), "hover() TIMED OUT");
    let markup = hover.unwrap().expect("hover() returned None").unwrap();
    assert_eq!(markup.value, "STUB_HOVER", "hover value mismatch");

    // Definition round-trip.
    let def = timeout(
        Duration::from_secs(2),
        client.goto_definition("file:///tmp/test.rs", Position { line: 0, character: 0 }),
    )
    .await;
    assert!(def.is_ok(), "definition() TIMED OUT");
    let location = def.unwrap().expect("definition() returned None").unwrap();
    assert_eq!(location.uri, "file:///tmp/test.rs", "definition uri mismatch");
    assert_eq!(location.range.start.line, 10, "definition start line mismatch");
    assert_eq!(location.range.start.character, 5, "definition start char mismatch");

    // Shutdown round-trip.
    client.shutdown().await.expect("shutdown() failed");
}

/// A Python LSP stub that reads but never responds (hangs forever).
fn write_hung_stub(path: &std::path::Path) {
    let script = r#"
import sys

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
    return body.decode()

while True:
    msg = read_msg()
    if msg is None: break
    # Never respond — hang forever.
"#;
    std::fs::write(path, script).expect("write hung stub");
}

/// Path to the hung stub script.
fn hung_stub_path() -> Stub {
    stub_in_tempdir("hung_stub.py", write_hung_stub)
}

#[tokio::test]
async fn init_timeout_fires_on_hung_stub() {
    let stub = hung_stub_path();
    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "hung-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(2),
    });

    let result = timeout(Duration::from_secs(5), client.initialize()).await;
    assert!(result.is_ok(), "initialize() TIMED OUT");
    let init_result = result.unwrap();
    assert!(init_result.is_err(), "expected Err(InitTimeout)");
    let err = init_result.unwrap_err();
    // Assert it's an InitTimeout error.
    match err {
        luxe_lsp::client::LspError::InitTimeout { server, timeout } => {
            assert_eq!(server, "hung-stub");
            assert_eq!(timeout, Duration::from_secs(2));
        }
        other => panic!("expected InitTimeout, got: {:?}", other),
    }
}

/// A Python LSP stub that responds to `initialize`, waits for `initialized` notification,
/// then waits for `didOpen`, and sends `publishDiagnostics`.
pub(super) fn write_did_open_stub(path: &std::path::Path) {
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
    body = sys.stdin.buffer.read(n)
    return json.loads(body)

def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

# Respond to initialize.
msg = read_msg()
write_msg({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{}}})

# Wait for initialized notification (client sends this after initialize).
msg = read_msg()
if msg.get("method") == "initialized":
    # Now wait for didOpen.
    msg = read_msg()
    if msg.get("method") == "textDocument/didOpen":
        # Send publishDiagnostics notification (no id).
        write_msg({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///tmp/test.rs",
                "diagnostics": [
                    {
                        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                        "severity": 1,
                        "source": "test",
                        "message": "Test diagnostic"
                    }
                ]
            }
        })

# Exit (as a notification, no id).
write_msg({"jsonrpc":"2.0","method":"exit","params":{}})
"#;
    std::fs::write(path, script).expect("write did_open stub");
}

/// Path to the did_open stub script.
fn did_open_stub_path() -> Stub {
    stub_in_tempdir("did_open_stub.py", write_did_open_stub)
}

#[tokio::test]
async fn did_open_triggers_publish_diagnostics() {
    let stub = did_open_stub_path();
    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "did-open-stub".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(5),
    });

    // Initialize.
    let init = timeout(Duration::from_secs(3), client.initialize()).await;
    assert!(init.is_ok(), "initialize() TIMED OUT");
    init.unwrap().expect("initialize() returned Err");

    // Send didOpen for a file.
    client
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///tmp/test.rs",
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn main() {}"
                }
            }),
        )
        .await
        .expect("didOpen notification failed");

    // Wait for publishDiagnostics notification (may receive other notifications first).
    let mut found_publish_diagnostics = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(100), client.wait_for_notification()).await {
            Ok(Some(notification)) => {
                if let Some(method) = &notification.method {
                    if method == "textDocument/publishDiagnostics" {
                        found_publish_diagnostics = true;
                        // Assert the diagnostics are present.
                        let params_value = notification.params.expect("notification has params");
                        let params = params_value.as_object().expect("params not an object");
                        let diagnostics = params.get("diagnostics").expect("no diagnostics field");
                        let diag_array = diagnostics.as_array().expect("diagnostics not an array");
                        assert_eq!(diag_array.len(), 1, "expected 1 diagnostic");

                        let diag = &diag_array[0];
                        assert_eq!(
                            diag.get("message").and_then(|v| v.as_str()),
                            Some("Test diagnostic")
                        );
                        break;
                    }
                }
            }
            _ => continue,
        }
    }
    assert!(found_publish_diagnostics, "did not receive publishDiagnostics notification");
}

/// Gated rust-analyzer integration test. Exercise LspClient against a real server.
///
/// Run manually with: `cargo test -p luxe-lsp --test integration_client -- --ignored`
#[cfg(test)]
#[tokio::test]
#[ignore = "gated: requires rust-analyzer; run with --ignored"]
async fn rust_analyzer_initialize_gated() {
    use std::io::Write;

    // Create a minimal Cargo project in /tmp so rust-analyzer has something to work with.
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    std::fs::create_dir_all(tmp_dir.join("src")).expect("create src dir");
    {
        let mut cargo =
            std::fs::File::create(tmp_dir.join("Cargo.toml")).expect("create Cargo.toml");
        write!(cargo, "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
            .expect("write Cargo.toml");
    }
    {
        let mut main_rs =
            std::fs::File::create(tmp_dir.join("src/main.rs")).expect("create main.rs");
        writeln!(main_rs, "fn main() {{ println!(\"hello\"); }}").expect("write main.rs");
    }

    // Build LspClient pointing at rust-analyzer.
    let config = LspConfig {
        command: vec!["rust-analyzer".to_string()],
        server_id: "rust-analyzer".to_string(),
        root_path: tmp_dir.clone(),
        init_timeout: Duration::from_secs(10),
    };
    let mut client = LspClient::new(config);

    // Initialize — the minimum meaningful round-trip against a real server.
    let init = tokio::time::timeout(Duration::from_secs(15), client.initialize()).await;
    match init {
        Ok(Ok(_)) => {
            // Success: LspClient initialized against real rust-analyzer.
        }
        Ok(Err(e)) => panic!("initialize() returned Err: {:?}", e),
        Err(_) => panic!("initialize() TIMED OUT after 15s"),
    }

    // Shutdown cleanly.
    let shutdown = tokio::time::timeout(Duration::from_secs(5), client.shutdown()).await;
    match shutdown {
        Ok(Ok(_)) | Ok(Err(_)) => {
            // Either is fine; the important thing is we got through initialize.
        }
        Err(_) => {
            panic!("shutdown() TIMED OUT after 5s");
        }
    }
}
