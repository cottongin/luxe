//! A local server standing in for LM Studio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A local server standing in for LM Studio: answers every `/models` request with whatever it
/// currently serves, until it is stopped.
///
/// The reply is keyed on what the server is *holding*, never on how many connections have
/// arrived. That distinction is load-bearing. A probe races `LOCAL_PROBE_TIMEOUT`, so a client
/// that gives up still leaves an accepted connection behind; a queue popped per accept
/// desynchronizes the moment that happens, and every later assertion reads the answer meant
/// for the one before it. That is a real failure on a loaded runner and invisible everywhere
/// else, which is the worst combination a test can have.
pub(super) struct LocalServer {
    pub(super) endpoint: String,
    body: Arc<Mutex<String>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LocalServer {
    /// Bind a port and start answering with `body`.
    pub(super) fn start(body: &str) -> Self {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        listener.set_nonblocking(true).expect("a non-blocking listener");
        let endpoint = format!("http://{}/v1", listener.local_addr().expect("an address"));
        let body = Arc::new(Mutex::new(body.to_string()));
        let running = Arc::new(AtomicBool::new(true));

        let (served, alive) = (Arc::clone(&body), Arc::clone(&running));
        let thread = std::thread::spawn(move || {
            while alive.load(Ordering::Relaxed) {
                let Ok((mut sock, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                sock.set_nonblocking(false).expect("a blocking connection");
                // Read the request before answering: closing a socket with unread bytes on it
                // sends a reset, and the reply goes with it.
                let mut seen = Vec::new();
                let mut chunk = [0u8; 1024];
                while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => seen.extend_from_slice(&chunk[..read]),
                    }
                }
                let body = served.lock().expect("the body").clone();
                // `Connection: close` keeps each request on its own socket. This loop answers
                // one request per `accept` and goes back to accepting, so a client entitled to
                // reuse the connection puts its next request into a socket nobody will read —
                // and the test that changes what the server serves then never sees the change.
                // Dropping the socket closes it, but the client only learns that on its own
                // schedule; on Windows it learned late enough to fail in CI while passing
                // everywhere else. Every other stub in the workspace says this out loud.
                let reply = crate::test_server::http_reply("200 OK", "application/json", &body);
                let _ = sock.write_all(reply.as_bytes());
                let _ = sock.flush();
            }
        });

        Self { endpoint, body, running, thread: Some(thread) }
    }

    /// What every request from here on answers with.
    pub(super) fn serves(&self, body: &str) {
        *self.body.lock().expect("the body") = body.to_string();
    }

    /// Take the server away, and wait until the port is really gone.
    pub(super) fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}
