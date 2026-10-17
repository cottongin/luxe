//! End-to-end test for the discovery funnel (`symbol_search`/`module_report`/`read_symbol`)
//! against a stub `rust-analyzer` that answers `documentSymbol` + `workspace/symbol`.
// These tests stand up a stub server as an executable shell script, so they are unix-only by
// construction — the thing under test is portable, the fixture is not. Gated at the file level rather
// than per function so a Windows `cargo test` compiles the crate instead of failing on the fixture.
#![cfg(unix)]

use luxe_core::{ContentBlock, Tool, ToolContext};
use luxe_lsp::{ModuleReportTool, ReadSymbolTool, SymbolSearchTool, WorkspaceLspClient};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

static PATH_SERIAL: Mutex<()> = Mutex::const_new(());

const LIB_RS: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n\nimpl Point {\n    pub fn origin() -> Self {\n        Point { x: 0, y: 0 }\n    }\n}\n";

// documentSymbol returns hierarchical DocumentSymbols; workspace/symbol returns SymbolInformation.
const STUB: &str = r#"#!/usr/bin/env python3
import json, sys

def read():
    header = b""
    while b"\r\n\r\n" not in header:
        ch = sys.stdin.buffer.read(1)
        if not ch: return None
        header += ch
    n = 0
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    return json.loads(sys.stdin.buffer.read(n))

def send(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

DOC_SYMBOLS = [
    {"name": "add", "kind": 12,
     "range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 1}},
     "selectionRange": {"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 10}}},
    {"name": "Point", "kind": 23,
     "range": {"start": {"line": 4, "character": 0}, "end": {"line": 7, "character": 1}},
     "selectionRange": {"start": {"line": 4, "character": 11}, "end": {"line": 4, "character": 16}},
     "children": [
        {"name": "x", "kind": 8,
         "range": {"start": {"line": 5, "character": 4}, "end": {"line": 5, "character": 14}},
         "selectionRange": {"start": {"line": 5, "character": 8}, "end": {"line": 5, "character": 9}}},
        {"name": "y", "kind": 8,
         "range": {"start": {"line": 6, "character": 4}, "end": {"line": 6, "character": 14}},
         "selectionRange": {"start": {"line": 6, "character": 8}, "end": {"line": 6, "character": 9}}}
     ]}
]

while True:
    msg = read()
    if msg is None: break
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
    elif method == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": mid, "result": DOC_SYMBOLS})
    elif method == "workspace/symbol":
        send({"jsonrpc": "2.0", "id": mid, "result": [
            {"name": "add", "kind": 12, "location": {"uri": "file:///tmp/lib.rs",
             "range": {"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 10}}}}
        ]})
    elif method == "textDocument/references":
        send({"jsonrpc": "2.0", "id": mid, "result": [
            {"uri": "file:///tmp/lib.rs", "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}}},
            {"uri": "file:///tmp/other.rs", "range": {"start": {"line": 9, "character": 2}, "end": {"line": 9, "character": 5}}}
        ]})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": mid, "result": None})
    elif method == "exit":
        break
"#;

fn ctx(cwd: &std::path::Path) -> ToolContext {
    let (event_tx, _rx) = mpsc::channel(8);
    let (_c, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "test".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn text(result: &luxe_core::ToolResultData) -> String {
    match &result.content[0] {
        ContentBlock::Text(t) => t.clone(),
        _ => panic!("expected text"),
    }
}

#[tokio::test]
async fn discovery_funnel_search_report_and_read() {
    let _serial = PATH_SERIAL.lock().await;
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"t\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();

    let stub = dir.join("rust-analyzer");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let old_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.display(), old_path));

    let service = Arc::new(WorkspaceLspClient::new());
    let cx = ctx(&dir);

    // The funnel reads that return file content share the mutation-guard read tracker so a
    // subsequent `edit` isn't falsely blocked. Before any read the file is unseen.
    let tracker = Arc::new(luxe_core::read_tracker::ReadTracker::new());
    let lib = dir.join("src/lib.rs");
    assert_eq!(tracker.status("test", &lib), luxe_core::read_tracker::ReadStatus::Unseen);

    // module_report: tree-sitter outline (offline) with names, kinds, ranges, and nested methods.
    let report = ModuleReportTool::new(service.clone()).with_tracker(tracker.clone());
    let out = text(&report.run(serde_json::json!({ "file": "src/lib.rs" }), &cx).await);
    assert!(out.contains("[tree-sitter]"), "outline should come from tree-sitter: {out}");
    assert!(out.contains("[function] add  L1-3"), "{out}");
    assert!(out.contains("[struct] Point  L5-8"), "{out}");
    assert!(out.contains("  [method] origin  L11-13"), "methods should be nested: {out}");

    // module_report with who-uses-this: top-level symbols get reference counts (via the stub
    // server); nested methods are not queried.
    let with_usages =
        text(&report.run(serde_json::json!({ "file": "src/lib.rs", "usages": true }), &cx).await);
    assert!(with_usages.contains("add  L1-3  · used by 2"), "{with_usages}");
    assert!(with_usages.contains("Point  L5-8  · used by 2"), "{with_usages}");
    assert!(
        with_usages.contains("  [method] origin  L11-13")
            && !with_usages.contains("origin  L11-13  · used"),
        "nested methods should not be queried for usages: {with_usages}"
    );

    // module_report reading the file counts as having seen it for the edit/write guards.
    assert_eq!(
        tracker.status("test", &lib),
        luxe_core::read_tracker::ReadStatus::Current,
        "module_report should record the file for the mutation guards"
    );

    // read_symbol: verbatim body of `add`.
    let read = ReadSymbolTool::new(service.clone()).with_tracker(tracker.clone());
    let out =
        text(&read.run(serde_json::json!({ "file": "src/lib.rs", "symbol": "add" }), &cx).await);
    assert!(out.contains("add (function) — src/lib.rs:1-3"), "{out}");
    assert_eq!(
        tracker.status("test", &lib),
        luxe_core::read_tracker::ReadStatus::Current,
        "read_symbol should record the file for the mutation guards"
    );
    assert!(out.contains("pub fn add(a: i32, b: i32) -> i32 {"), "{out}");
    assert!(out.contains("a + b"), "{out}");
    assert!(!out.contains("pub struct Point"), "read_symbol should not leak other symbols: {out}");

    // symbol_search: ranked workspace match.
    let search = SymbolSearchTool::new(service.clone());
    let out =
        text(&search.run(serde_json::json!({ "query": "add", "file": "src/lib.rs" }), &cx).await);
    assert!(out.contains("add (function)"), "{out}");

    std::env::set_var("PATH", old_path);
}
