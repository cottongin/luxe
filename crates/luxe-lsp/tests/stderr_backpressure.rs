//! A server that logs more than a pipe buffer must stay answerable.
//!
//! An unread pipe fills at 64 KB and the next write blocks forever. clangd narrates every request
//! to stderr by default and passes that mark while indexing the C standard library, so a client
//! that stops draining does not merely lose the log — it wedges the server, and every later
//! request comes back `IO error: Broken pipe`. The stub here is the same shape, deterministically.

use luxe_lsp::protocol::Position;
use luxe_lsp::{LspClient, LspConfig};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::timeout;

/// Minimal LSP server that narrates far past a pipe buffer, then keeps answering.
const CHATTY_SERVER: &str = r#"
import sys, json

def read():
    header = b""
    while b"\r\n\r\n" not in header:
        byte = sys.stdin.buffer.read(1)
        if not byte:
            return None
        header += byte
    length = int([h for h in header.decode().split("\r\n")
                  if h.lower().startswith("content-length")][0].split(":")[1])
    return json.loads(sys.stdin.buffer.read(length))

def write(payload):
    body = json.dumps(payload).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write({"jsonrpc": "2.0", "id": message["id"],
               "result": {"capabilities": {"hoverProvider": True}}})
        # Four pipe buffers' worth. A client that stopped reading blocks us here, forever.
        for i in range(4000):
            sys.stderr.write("I[%05d] narrating every request, as clangd does by default\n" % i)
        sys.stderr.flush()
    elif method == "textDocument/hover":
        write({"jsonrpc": "2.0", "id": message["id"],
               "result": {"contents": {"kind": "plaintext", "value": "still alive"}}})
    elif method == "shutdown":
        write({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "exit":
        break
"#;

#[tokio::test]
async fn a_server_that_logs_past_the_pipe_buffer_stays_answerable() {
    if which("python3").is_none() {
        eprintln!("skipping: needs python3");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    let stub = dir.join("chatty_server.py");
    std::fs::write(&stub, CHATTY_SERVER).expect("write stub");

    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().into_owned()],
        server_id: "chatty".to_string(),
        root_path: dir.clone(),
        init_timeout: Duration::from_secs(10),
    });
    timeout(Duration::from_secs(10), client.initialize())
        .await
        .expect("initialize must not hang")
        .expect("initialize must succeed");

    // The server is mid-narration. Whether it can answer at all is the whole question.
    let file = dir.join("anything.txt");
    std::fs::write(&file, "x\n").expect("write");
    let answer = timeout(
        Duration::from_secs(10),
        client.hover(&format!("file://{}", file.display()), Position { line: 0, character: 0 }),
    )
    .await
    .expect("a server whose stderr is drained answers; one that is blocked on it does not");
    assert!(
        answer.is_ok(),
        "the server should still be reachable after logging past the buffer: {answer:?}"
    );

    let _ = client.shutdown().await;
}

fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}
