// These tests stand up a stub server as an executable shell script, so they are unix-only by
// construction — the thing under test is portable, the fixture is not. Gated at the file level rather
// than per function so a Windows `cargo test` compiles the crate instead of failing on the fixture.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use luxe_lsp::client::{LspClient, LspConfig};
use luxe_lsp::protocol::Position;
use tokio::time::timeout;

fn service_for_stub(stub: &std::path::Path) -> std::sync::Arc<luxe_lsp::WorkspaceLspClient> {
    let servers = std::collections::HashMap::from([(
        "rust".to_string(),
        luxe_lsp::registry::ServerEntryPatch {
            command: Some(luxe_core::paths::display(stub)),
            ..Default::default()
        },
    )]);
    std::sync::Arc::new(luxe_lsp::WorkspaceLspClient::with_servers(servers))
}

/// A minimal Python LSP stub that responds to `initialize` with a valid handshake.
fn write_stub(path: &std::path::Path) {
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

while True:
    msg = read_msg()
    if msg is None: break
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"capabilities":{}}})
    elif method == "textDocument/hover":
        write_msg({"jsonrpc":"2.0","id":mid,"result":{"contents":{"kind":"markdown","value":"STUB_HOVER"}}})
    elif method == "textDocument/definition":
        write_msg({"jsonrpc":"2.0","id":mid,"result":[{"uri":"file:///tmp/test.rs","range":{"start":{"line":10,"character":5},"end":{"line":10,"character":10}}}]})
    elif method == "shutdown":
        write_msg({"jsonrpc":"2.0","id":mid,"result":None})
    elif method == "exit":
        break
"#;
    std::fs::write(path, script).expect("write stub");
}

/// A stub script in a directory of its own, which goes when this does.
struct Stub {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for Stub {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

/// Writes a stub with `write` into a fresh directory.
fn stub_in_tempdir(name: &str, write: impl FnOnce(&std::path::Path)) -> Stub {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(name);
    write(&path);
    Stub { path, _dir: dir }
}

/// Path to the stub script, written to a temp dir so it doesn't pollute the source tree.
fn stub_path() -> Stub {
    stub_in_tempdir("stub_init.py", write_stub)
}

mod diagnostics;
mod failures;
mod lifecycle;
mod navigation;

use lifecycle::*;
