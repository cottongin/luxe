//! Test server — the one canned HTTP/1.1 server the adapters' tests talk to.
//!
//! A mock that parses requests its own way is a place a test can pass for the wrong reason, so
//! the request is read once, verbatim, by [`read_http_request`], and every test sees the same
//! thing: the request line, the headers and the whole `Content-Length` body as text.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;

/// Reads one whole HTTP request (headers and `Content-Length` body) off `sock`, verbatim.
pub(crate) fn read_http_request(sock: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = sock.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(head_end) = text.find("\r\n\r\n") {
            let content_len = text
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + content_len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// The body of a request [`read_http_request`] returned.
pub(crate) fn request_body(raw: &str) -> &str {
    raw.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// One canned HTTP reply.
///
/// `Connection: close` keeps each request on its own socket, so a mock serving several replies
/// isn't left waiting on `accept` while the client reuses the first.
pub(crate) fn http_reply(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A server on a loopback port, answering each reply it was given once, in order.
pub(crate) struct MockServer {
    port: u16,
    handle: JoinHandle<Vec<String>>,
}

impl MockServer {
    /// The port it listens on
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Waits for the last reply to go out and returns every request it saw, verbatim.
    pub(crate) fn requests(self) -> Vec<String> {
        self.handle.join().expect("the mock server thread")
    }
}

/// Starts a server that answers `replies` in order, one per connection.
pub(crate) fn serve_replies(replies: Vec<String>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept() else { break };
            seen.push(read_http_request(&mut sock));
            let _ = sock.write_all(reply.as_bytes());
            let _ = sock.flush();
        }
        seen
    });
    MockServer { port, handle }
}

/// Starts a server that answers one request with `status`, `content_type` and `body`.
pub(crate) fn serve_once(status: &str, content_type: &str, body: &str) -> MockServer {
    serve_replies(vec![http_reply(status, content_type, body)])
}

/// Starts a server that writes `reply` as is — no length, no close — and keeps the socket open
/// until the returned sender fires, the way a proxy that holds a connection after the last event
/// does.
pub(crate) fn serve_held_open(reply: &str) -> (MockServer, std::sync::mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    let (release, held) = std::sync::mpsc::channel::<()>();
    let reply = reply.to_owned();
    let handle = std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return Vec::new() };
        let seen = read_http_request(&mut sock);
        let _ = sock.write_all(reply.as_bytes());
        let _ = sock.flush();
        let _ = held.recv_timeout(std::time::Duration::from_secs(30));
        vec![seen]
    });
    (MockServer { port, handle }, release)
}
