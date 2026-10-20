use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A client reaches the endpoint and both directions carry bytes; the listener names the client's
/// process, which here is this test's own.
#[tokio::test]
async fn a_client_reaches_the_endpoint_both_ways() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut listener = EndpointListener::bind(&dir.path().join("run")).expect("binds");
    let address = listener.address().to_string();
    let (client, joined) = tokio::join!(connect(&address), listener.accept());
    let (mut client, mut joined) = (client.expect("connects"), joined.expect("accepts"));

    client.write_all(b"ping").await.expect("writes");
    let mut heard = [0u8; 4];
    joined.stream.read_exact(&mut heard).await.expect("reads");
    joined.stream.write_all(b"pong").await.expect("answers");
    client.read_exact(&mut heard).await.expect("hears");
    assert_eq!(&heard, b"pong");
    assert_eq!(joined.peer, Some(std::process::id()));
}

/// The socket and its folder are this user's alone, and the socket goes with the listener.
#[tokio::test]
#[cfg(unix)]
async fn the_socket_is_private_and_goes_with_the_listener() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("a temp dir");
    let run = dir.path().join("run");
    let listener = EndpointListener::bind(&run).expect("binds");
    let socket = std::path::PathBuf::from(listener.address());
    let mode = |path: &std::path::Path| {
        std::fs::metadata(path).expect("exists").permissions().mode() & 0o777
    };
    assert_eq!(mode(&run), 0o700, "the folder");
    assert_eq!(mode(&socket), 0o600, "the socket");
    drop(listener);
    assert!(!socket.exists(), "a stopped luxe leaves no socket behind");
    assert!(connect(&socket.to_string_lossy()).await.is_err());
}

/// A published record is listed until it is dropped, and a stray file in the folder is not one.
#[test]
fn a_record_is_listed_while_its_luxe_runs() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let session = LiveSession {
        pid: 4242,
        session_id: "session-a".into(),
        cwd: "/work/widget".into(),
        address: "/run/user/1000/luxe/4242.sock".into(),
        luxe_version: "0.1.0".into(),
    };
    let publication = session.publish(dir.path()).expect("publishes");
    std::fs::write(dir.path().join("notes.txt"), "not a record").expect("writes");
    std::fs::write(dir.path().join("7.json"), "{ half").expect("writes");
    assert_eq!(live_sessions(dir.path()), vec![session.clone()]);

    let moved = LiveSession { session_id: "session-b".into(), ..session };
    publication.update(&moved).expect("updates");
    assert_eq!(live_sessions(dir.path()), vec![moved]);
    drop(publication);
    assert_eq!(live_sessions(dir.path()), Vec::new());
}

/// The record's shape is what other programs read, so it is pinned, not only round-tripped.
#[test]
fn the_record_shape_is_pinned() {
    let literal = r#"{"pid":4242,"session_id":"session-a","cwd":"/work/widget","address":"/run/user/1000/luxe/4242.sock","luxe_version":"0.1.0"}"#;
    let read: LiveSession = serde_json::from_str(literal).expect("reads");
    assert_eq!(serde_json::to_string(&read).expect("writes"), literal);
}
