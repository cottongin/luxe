//! Failures — a server that dies at startup, exits silently or dies mid-request.

use super::*;

/// A server that starts, complains on stderr, and stops.
///
/// The commonest real shape of a broken language server, and the one Luxe reported worst: a
/// version-manager shim that resolves on `PATH` and then refuses to run. `mise ERROR No version is
/// set for shim: pyright-langserver` and rustup's `error: infinite recursion detected` are both
/// this, and both were reported as `initialize timed out after 30s` — after waiting the full 30
/// seconds for a process that had been dead since millisecond ten.
#[tokio::test]
async fn a_server_that_dies_at_startup_says_so_at_once_and_in_its_own_words() {
    let mut client = LspClient::new(LspConfig {
        command: vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo 'No version is set for shim' >&2; exit 3".to_string(),
        ],
        server_id: "broken-shim".to_string(),
        // Long enough that waiting it out would be unmistakable in the elapsed time below.
        init_timeout: Duration::from_secs(30),
        root_path: PathBuf::from("/tmp"),
    });

    let started = std::time::Instant::now();
    let error = client.initialize().await.expect_err("a server that exits cannot initialize");
    let elapsed = started.elapsed();

    match error {
        luxe_lsp::client::LspError::ServerExited { server, status, stderr } => {
            assert_eq!(server, "broken-shim");
            assert!(status.contains('3'), "the exit status is part of the answer: {status}");
            assert!(
                stderr.contains("No version is set"),
                "the server's own complaint is the whole point: {stderr:?}"
            );
        }
        other => panic!("expected ServerExited, got: {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "a dead server is knowable immediately; this waited {elapsed:?} of a 30s timeout"
    );
}

/// A server with nothing to say still reports the exit rather than a timeout.
///
/// Both orderings, because they reach the answer down different paths and only one of them used to
/// be covered. A server that is already gone breaks the pipe, so the handshake *write* fails and
/// the exit has to be recognized after the fact; one that lingers a moment accepts the write and
/// then dies, so the exit wins the race outright. The first is the one that flaked: the exit status
/// only becomes collectible once this process is notified, which under load lands after the
/// `EPIPE`, and a client that checked once was told "still running" about a process that was
/// already gone — reporting a broken pipe instead of an exit.
#[tokio::test]
async fn a_silent_exit_is_still_an_exit_not_a_timeout() {
    for (label, script) in
        [("gone before the write", "exit 0"), ("gone after the write", "sleep 0.2; exit 3")]
    {
        let mut client = LspClient::new(LspConfig {
            command: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            server_id: "quiet".to_string(),
            init_timeout: Duration::from_secs(30),
            root_path: PathBuf::from("/tmp"),
        });
        let error = client.initialize().await.expect_err("an exited server cannot initialize");
        assert!(
            matches!(error, luxe_lsp::client::LspError::ServerExited { .. }),
            "{label}: got {error:?}"
        );
    }
}

/// A stub that answers `initialize` and then dies on the next request, mid-handshake-free session.
fn write_dying_stub(path: &std::path::Path) {
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

while True:
    msg = read_msg()
    if msg is None: break
    if msg.get("method") == "initialize":
        write_msg({"jsonrpc":"2.0","id":msg.get("id"),"result":{"capabilities":{}}})
    else:
        # Whatever it was, die without answering — a crash under a real request.
        sys.exit(9)
"#;
    std::fs::write(path, script).expect("write stub");
}

/// A request in flight when the server dies comes back as an error, rather than never coming back.
///
/// The reader is the only thing that can answer a request, so when it stops nothing is coming. A
/// waiter left in the pending map never resolves: a server that crashed mid-session would become a
/// turn that hangs, with no error and nothing in the log. The bound here is the assertion — a
/// client that forgets the waiter sits here until the test harness kills it.
#[tokio::test]
async fn a_request_in_flight_when_the_server_dies_returns_rather_than_hanging() {
    let scratch = tempfile::tempdir().expect("temp dir");
    let stub = scratch.path().join("stub_dying.py");
    write_dying_stub(&stub);

    let mut client = LspClient::new(LspConfig {
        command: vec!["python3".to_string(), stub.to_string_lossy().to_string()],
        server_id: "dying".to_string(),
        init_timeout: Duration::from_secs(10),
        root_path: PathBuf::from("/tmp"),
    });
    client.initialize().await.expect("the stub completes a handshake");

    let answered = timeout(
        Duration::from_secs(5),
        client.send_request("textDocument/hover", serde_json::json!({})),
    )
    .await;

    assert!(answered.is_ok(), "a dead server's request must resolve, not hang forever");
    assert!(answered.expect("resolved").is_err(), "and it must resolve as a failure");
}
