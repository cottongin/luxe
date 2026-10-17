//! Navigation — every action of the navigation tool against a stub.

use super::*;

/// Test LspNavigationTool with all four navigation actions against a Python stub.
#[tokio::test]
async fn lsp_navigation_tool_all_actions() {
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

    // Write a Python stub that responds to all navigation methods.
    let stub = tmp_dir.join("navigation-stub");
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
        write_msg({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":True}})
    elif method == "textDocument/didOpen":
        pass  # No response needed for notifications
    elif method == "textDocument/definition":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"uri":"file:///tmp/test.rs","range":{"start":{"line":10,"character":5},"end":{"line":10,"character":10}}}]})
    elif method == "textDocument/references":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"uri":"file:///tmp/test.rs","range":{"start":{"line":5,"character":0},"end":{"line":5,"character":5}}}]})
    elif method == "textDocument/hover":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"contents":{"kind":"markdown","value":"STUB_HOVER_INFO"}}})
    elif method == "textDocument/documentSymbol":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"name":"main","kind":12,"location":{"uri":"file:///tmp/test.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":20}}}}]})
    elif method == "textDocument/typeDefinition":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"targetUri":"file:///tmp/types.rs","targetSelectionRange":{"start":{"line":2,"character":4},"end":{"line":2,"character":8}}}]})
    elif method == "workspace/symbol":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"name":"main","kind":12,"location":{"uri":"file:///tmp/test.rs","range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}}}}]})
    elif method == "textDocument/signatureHelp":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"signatures":[{"label":"fn main()"}],"activeSignature":0}})
    elif method == "textDocument/prepareCallHierarchy":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"name":"main","kind":12,"uri":"file:///tmp/test.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":20}},"selectionRange":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}}}]})
    elif method == "callHierarchy/incomingCalls":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"from":{"name":"caller","kind":12,"uri":"file:///tmp/other.rs","selectionRange":{"start":{"line":7,"character":3},"end":{"line":7,"character":9}}},"fromRanges":[]}]})
    elif method == "textDocument/codeAction":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"title":"Fix the thing","kind":"quickfix"}]})
    elif method == "textDocument/rename":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"changes":{"file:///tmp/test.rs":[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"newText":"renamed"}]}}})
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

    // Test go_to_definition.
    use luxe_core::Tool;
    use luxe_lsp::LspNavigationTool;
    let tool = LspNavigationTool::new(service_for_stub(&stub));
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust",
        "action": "go_to_definition",
        "line": 1,
        "character": 1
    });
    let result = tool.run(args, &cx).await;
    assert!(!result.is_error, "go_to_definition returned error: {:?}", result.content);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("Definition:"), "expected definition output, got: {}", text);
    } else {
        panic!("expected Text block");
    }

    // Symbol-name resolution: no line/character, resolve the position from `symbol`.
    let by_name = tool
        .run(
            serde_json::json!({ "file": "src/main.rs", "action": "go_to_definition", "symbol": "main" }),
            &cx,
        )
        .await;
    assert!(!by_name.is_error, "symbol-name definition errored: {:?}", by_name.content);
    assert!(matches!(&by_name.content[0], ContentBlock::Text(t) if t.contains("Definition:")));

    // Test the new navigation actions against the stub.
    let cases = [
        (
            serde_json::json!({ "file": "src/main.rs", "action": "type_definition", "line": 1 }),
            "types.rs",
        ),
        (
            serde_json::json!({ "file": "src/main.rs", "action": "workspace_symbol", "query": "main" }),
            "main",
        ),
        (
            serde_json::json!({ "file": "src/main.rs", "action": "signature_help", "line": 1 }),
            "fn main()",
        ),
        (
            serde_json::json!({ "file": "src/main.rs", "action": "call_hierarchy", "line": 1 }),
            "caller",
        ),
        (
            serde_json::json!({ "file": "src/main.rs", "action": "code_action", "line": 1 }),
            "Fix the thing",
        ),
        (
            serde_json::json!({ "file": "src/main.rs", "action": "rename", "line": 1, "newName": "renamed" }),
            "Rename preview",
        ),
    ];
    for (args, expected) in cases {
        let action = args["action"].as_str().unwrap().to_string();
        let result = tool.run(args, &cx).await;
        assert!(!result.is_error, "{action} errored: {:?}", result.content);
        assert!(
            matches!(&result.content[0], ContentBlock::Text(t) if t.contains(expected)),
            "{action}: expected {expected:?}, got {:?}",
            result.content
        );
    }

    // Test find_references.
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust",
        "action": "find_references",
        "line": 1,
        "character": 1
    });
    let result = tool.run(args, &cx).await;
    assert!(!result.is_error, "find_references returned error: {:?}", result.content);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("1 reference"), "expected references output, got: {}", text);
    } else {
        panic!("expected Text block");
    }

    // Test hover.
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust",
        "action": "hover",
        "line": 1,
        "character": 1
    });
    let result = tool.run(args, &cx).await;
    assert!(!result.is_error, "hover returned error: {:?}", result.content);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("STUB_HOVER_INFO"), "expected hover output, got: {}", text);
    } else {
        panic!("expected Text block");
    }

    // Test document_symbols.
    let args = serde_json::json!({
        "file": "src/main.rs",
        "language": "rust",
        "action": "document_symbols"
    });
    let result = tool.run(args, &cx).await;
    assert!(!result.is_error, "document_symbols returned error: {:?}", result.content);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(
            text.contains("main") && text.contains("1 symbol"),
            "expected symbols output, got: {}",
            text
        );
    } else {
        panic!("expected Text block");
    }
}
