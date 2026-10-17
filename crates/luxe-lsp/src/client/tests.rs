use super::diagnostics::hash_content;
use super::lifecycle::pull_provider;
use super::*;
use crate::protocol::Range;

fn config() -> LspConfig {
    LspConfig {
        command: vec!["rust-analyzer".to_string()],
        server_id: "rust-analyzer".to_string(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(30),
    }
}

#[tokio::test]
async fn test_shutdown_when_not_initialized() {
    let mut client = LspClient::new(config());
    assert_eq!(client.state, ClientState::Uninitialized);
    client.shutdown().await.unwrap();
    assert_eq!(client.state, ClientState::Shutdown);
}

/// The spec enumerates these identifiers; a wrong one asserts something false about the file.
#[test]
fn language_id_is_inferred_from_the_uri_extension() {
    let client = |language: &str| {
        LspClient::new(LspConfig {
            command: vec!["true".to_string()],
            server_id: language.to_string(),
            root_path: PathBuf::from("/tmp"),
            init_timeout: Duration::from_secs(1),
        })
    };
    let rust = client("rust");
    assert_eq!(rust.language_id_for("file:///a/b.rs"), "rust");
    assert_eq!(rust.language_id_for("file:///a/b.ts"), "typescript");
    assert_eq!(rust.language_id_for("file:///a/b.py"), "python");
    assert_eq!(rust.language_id_for("file:///a/b.go"), "go");
    // React variants are their own languages, not the plain ones.
    assert_eq!(rust.language_id_for("file:///a/b.tsx"), "typescriptreact");
    assert_eq!(rust.language_id_for("file:///a/b.jsx"), "javascriptreact");
    // C and C++ are separate, and neither is Rust.
    assert_eq!(rust.language_id_for("file:///a/b.c"), "c");
    assert_eq!(rust.language_id_for("file:///a/b.cpp"), "cpp");

    // The fallback is *this server's* language. Everything unrecognized used to claim `rust`,
    // so a `.vue` file opened against a Vue server announced itself as Rust.
    assert_eq!(client("vue").language_id_for("file:///a/b.vue"), "vue");
    assert_eq!(client("vue").language_id_for("file:///a/noextension"), "vue");
}

#[test]
fn unchanged_content_hashes_match() {
    assert_eq!(hash_content("fn main() {}"), hash_content("fn main() {}"));
    assert_ne!(hash_content("fn main() {}"), hash_content("fn main() { }"));
}

/// The capability object decides whether a read asks or waits, so a shape that is not an
/// object must not be read as one: `false` and a missing key both mean push-only, and a
/// truthiness check on the JSON would have turned the first into a pull server.
#[test]
fn a_pull_provider_is_read_from_the_capability_object_only() {
    let with = serde_json::json!({
        "capabilities": { "diagnosticProvider": {
            "identifier": "typescript", "interFileDependencies": true
        }}
    });
    assert_eq!(pull_provider(&with).unwrap().identifier.as_deref(), Some("typescript"));

    // An options object without an identifier is still a pull server; the request omits it.
    let bare = serde_json::json!({ "capabilities": { "diagnosticProvider": {} } });
    assert_eq!(pull_provider(&bare).unwrap().identifier, None);

    for push_only in [
        serde_json::json!({ "capabilities": {} }),
        serde_json::json!({ "capabilities": { "diagnosticProvider": false } }),
        serde_json::json!({ "capabilities": { "diagnosticProvider": true } }),
        serde_json::json!({ "capabilities": { "diagnosticProvider": null } }),
        serde_json::json!({}),
    ] {
        assert!(pull_provider(&push_only).is_none(), "not a pull provider: {push_only}");
    }
}

/// Push wins where it is offered, and silence is not an answer.
#[test]
fn only_a_document_the_server_has_said_nothing_about_is_asked_about() {
    let mut client = LspClient::new(config());
    let uri = "file:///tmp/a.ts";

    // No advertised provider: there is nothing to ask, whatever the cache holds.
    assert!(!client.pulls_diagnostics_for(uri));

    client.pull_diagnostics = Some(PullProvider { identifier: None });
    assert!(client.pulls_diagnostics_for(uri));

    // An empty push is the server saying "clean" — an answer, so it is not asked again.
    {
        let mut inner = client.shared.inner.lock().expect("state");
        inner.diagnostics.insert(uri.to_string(), Vec::new());
    }
    assert!(!client.pulls_diagnostics_for(uri));
    assert!(client.pulls_diagnostics_for("file:///tmp/b.ts"));
}

/// A pulled answer is only read where the server pushed nothing — it never shadows a push.
#[test]
fn a_pushed_answer_outranks_a_pulled_one() {
    let mut client = LspClient::new(config());
    let uri = "file:///tmp/a.ts";
    let pulled = Diagnostic {
        range: Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 0, character: 1 },
        },
        severity: None,
        source: None,
        code: None,
        message: "pulled".to_string(),
    };
    client.pulled.insert(uri.to_string(), PulledReport { result_id: None, items: vec![pulled] });
    assert_eq!(client.cached_diagnostics(uri)[0].message, "pulled");

    {
        let mut inner = client.shared.inner.lock().expect("state");
        inner.diagnostics.insert(uri.to_string(), Vec::new());
    }
    assert!(client.cached_diagnostics(uri).is_empty(), "the push is the answer");
}

/// A server that answers `initialize`, then reads on and answers nothing, `exit` included.
#[cfg(unix)]
fn deaf_server(dir: &std::path::Path) -> LspConfig {
    let script = dir.join("deaf.py");
    std::fs::write(
        &script,
        r#"
import json, sys
def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    n = int([l for l in header.split(b"\r\n") if l.lower().startswith(b"content-length:")][0].split(b":")[1])
    return json.loads(sys.stdin.buffer.read(n))
while True:
    msg = read_msg()
    if msg is None: break
    if msg.get("method") == "initialize":
        data = json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
        sys.stdout.buffer.flush()
"#,
    )
    .unwrap();
    LspConfig {
        command: vec!["python3".into(), script.display().to_string()],
        server_id: "deaf".into(),
        root_path: dir.to_path_buf(),
        init_timeout: Duration::from_secs(10),
    }
}

/// Shutting down a server that ignores `exit` ends, by killing it, instead of waiting forever.
#[cfg(unix)]
#[tokio::test]
async fn a_server_that_ignores_exit_is_killed_at_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = LspClient::new(deaf_server(dir.path()));
    client.initialize().await.expect("handshake");
    let pid = client.child_id().expect("running");

    let started = Instant::now();
    timeout(Duration::from_secs(10), client.shutdown()).await.expect("bounded").unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(!luxe_core::program::process_is_alive(pid), "the server was reaped");
}

/// A request given up on leaves nothing behind in the pending map.
///
/// The break this catches: the entry was removed only after an answer, so a dropped request
/// future, as a tool timeout drops it, left it there until the server exited.
#[cfg(unix)]
#[tokio::test]
async fn an_abandoned_request_leaves_no_pending_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = LspClient::new(deaf_server(dir.path()));
    client.initialize().await.expect("handshake");

    let asked = timeout(
        Duration::from_millis(200),
        client.send_request("textDocument/hover", serde_json::json!({})),
    )
    .await;
    assert!(asked.is_err(), "the deaf server never answers");
    assert!(client.pending_requests.lock().await.is_empty(), "the entry went with the request");
    client.shutdown().await.unwrap();
}

/// A client whose handshake failed does not report that it can be asked.
///
/// The break this catches: the state became `Initialized` right after the spawn, so a failed
/// handshake left a client that answered as ready and failed each request on a dead pipe instead.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_handshake_leaves_the_client_uninitialized() {
    let mut client = LspClient::new(LspConfig {
        command: vec!["sh".into(), "-c".into(), "exit 3".into()],
        server_id: "gone".into(),
        root_path: PathBuf::from("/tmp"),
        init_timeout: Duration::from_secs(10),
    });
    assert!(client.initialize().await.is_err());
    assert_eq!(client.state, ClientState::Uninitialized);
    let asked = client.send_request("textDocument/hover", serde_json::json!({})).await;
    assert!(matches!(asked, Err(LspError::NotInitialized)), "{asked:?}");
}
