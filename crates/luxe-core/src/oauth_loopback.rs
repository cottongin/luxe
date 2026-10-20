//! OAuth loopback — PKCE, `state`, and the localhost listener a browser hands a code back to.
//!
//! Shared by provider sign-ins and MCP servers, and kept to a raw `TcpListener` and a few lines of
//! HTTP, since core has no HTTP client. Whatever arrives is untrusted, because any page the browser
//! has open can make it request `localhost`: a connection that says nothing is dropped after a
//! short deadline instead of stalling the listener, a request for another path or carrying another
//! sign-in's `state` is answered and ignored instead of ending this one, and every page escapes
//! what it repeats.

use std::time::Duration;

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// How long one connection gets to send its request line before it is dropped.
const READ_DEADLINE: Duration = Duration::from_secs(2);

/// A PKCE verifier and challenge (S256).
#[derive(Debug, Clone)]
pub struct Pkce {
    /// Sent with the token exchange.
    pub verifier: String,
    /// `S256(verifier)`, sent with the authorization request.
    pub challenge: String,
}

/// A fresh PKCE pair.
pub fn pkce() -> Pkce {
    let verifier = random_urlsafe(32);
    let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

/// A random `state`, which ties a redirect to the sign-in that asked for it.
pub fn random_state() -> String {
    random_urlsafe(24)
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    // with no OS randomness a predictable verifier would be a hole, not an inconvenience
    getrandom::fill(&mut buffer).expect("OS randomness unavailable");
    base64_url(&buffer)
}

fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The listener a browser comes back to, and the redirect URI it answers.
///
/// Dropping it closes the sockets, which is how a cancelled sign-in stops listening.
pub struct LoopbackCallback {
    listeners: Vec<TcpListener>,
    redirect_uri: String,
    path: String,
}

impl LoopbackCallback {
    /// Listen for `redirect_uri`: `http` on `127.0.0.1`, `localhost` or `[::1]` with a port, `0`
    /// for any free one (the redirect URI then names the one chosen).
    ///
    /// A provider checks a registered redirect exactly, so it has to be the address actually
    /// listened on. `localhost` is listened for on both address families, since a browser may
    /// resolve it to either.
    pub async fn bind(redirect_uri: &str) -> Result<Self, LoopbackError> {
        let invalid = |reason: String| LoopbackError::Invalid(reason);
        let mut url = url::Url::parse(redirect_uri)
            .map_err(|e| invalid(format!("redirect_uri {redirect_uri:?} is not a URL: {e}")))?;
        let host = url.host_str().unwrap_or_default().to_string();
        if url.scheme() != "http" || !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
            return Err(invalid(format!(
                "redirect_uri {redirect_uri:?} must be http on 127.0.0.1, localhost or [::1] — a \
                 browser can only hand a code back to this machine there"
            )));
        }
        let port = url.port().ok_or_else(|| {
            invalid(format!(
                "redirect_uri {redirect_uri:?} needs an explicit port, the one it was registered with"
            ))
        })?;
        let hosts: &[&str] = match host.as_str() {
            "localhost" => &["127.0.0.1", "[::1]"],
            _ => &[host.as_str()],
        };
        let listeners = listen(hosts, port).await?;
        let path = if url.path().is_empty() { "/".to_string() } else { url.path().to_string() };
        // a configured redirect stays exactly as written, since a provider compares the string
        let redirect_uri = match port {
            0 => {
                let bound = listeners[0].local_addr().map_err(LoopbackError::Io)?.port();
                let _ = url.set_port(Some(bound));
                url.to_string()
            }
            _ => redirect_uri.to_string(),
        };
        Ok(Self { listeners, redirect_uri, path })
    }

    /// The redirect URI to hand the provider.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// The next request on the redirect path that answers this sign-in: it carries `code` or
    /// `error`, and `expected_state` when there is one. Anything else is answered and ignored.
    pub async fn wait(&self, expected_state: Option<&str>) -> std::io::Result<Arrival> {
        loop {
            let mut socket = accept_any(&self.listeners).await?;
            let Some(target) = read_request_target(&mut socket).await else { continue };
            let arrival = Arrival::new(socket, target);
            let (path, _) = arrival.target.split_once('?').unwrap_or((&arrival.target, ""));
            if path != self.path {
                arrival.answer(404, "Not the sign-in", "This address only takes a sign-in.").await;
                continue;
            }
            if expected_state.is_some_and(|expected| arrival.param("state") != Some(expected)) {
                let message = "This answer belongs to another sign-in, so it was ignored.";
                arrival.answer(400, "Not this sign-in", message).await;
                continue;
            }
            if arrival.param("code").is_none() && arrival.param("error").is_none() {
                let message = "This is not an authorization answer.";
                arrival.answer(400, "Not the sign-in", message).await;
                continue;
            }
            return Ok(arrival);
        }
    }
}

/// Why the listener could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum LoopbackError {
    /// The redirect URI is not one this machine can receive.
    #[error("{0}")]
    Invalid(String),
    /// Something else holds the address, on one family or both.
    #[error("{addr} is already in use")]
    InUse {
        /// The `host:port` that was taken
        addr: String,
    },
    /// Every bind failed for another reason; each is listed.
    #[error("could not listen for the redirect: {0}")]
    Unavailable(String),
    /// The socket could not report its own address.
    #[error("the redirect listener has no address: {0}")]
    Io(std::io::Error),
}

impl From<LoopbackError> for crate::auth::AuthError {
    fn from(error: LoopbackError) -> Self {
        Self::Network(error.to_string())
    }
}

/// Bind every host at `port`. A port in use is fatal on either family, since a browser trying that
/// family would hand the code to whoever holds it; only an IPv6 loopback this machine lacks is
/// skipped. Port `0` retries with fresh ports when the second family's side is taken.
async fn listen(hosts: &[&str], port: u16) -> Result<Vec<TcpListener>, LoopbackError> {
    let mut attempts = 0;
    loop {
        match listen_once(hosts, port).await {
            Err(LoopbackError::InUse { .. }) if port == 0 && attempts < 5 => attempts += 1,
            outcome => return outcome,
        }
    }
}

async fn listen_once(hosts: &[&str], port: u16) -> Result<Vec<TcpListener>, LoopbackError> {
    let mut listeners: Vec<TcpListener> = Vec::new();
    let mut failures = Vec::new();
    for host in hosts {
        // the second family takes the port the first one was given
        let port = match listeners.first() {
            Some(first) => first.local_addr().map_err(LoopbackError::Io)?.port(),
            None => port,
        };
        let addr = format!("{host}:{port}");
        match TcpListener::bind(&addr).await {
            Ok(listener) => listeners.push(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(LoopbackError::InUse { addr });
            }
            // a machine with no IPv6 loopback still has the IPv4 one
            Err(error)
                if *host == "[::1]"
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
                    ) => {}
            Err(error) => failures.push(format!("{addr}: {error}")),
        }
    }
    if listeners.is_empty() || !failures.is_empty() {
        let failures =
            if failures.is_empty() { "no address".to_string() } else { failures.join("; ") };
        return Err(LoopbackError::Unavailable(failures));
    }
    Ok(listeners)
}

/// The next connection on any of `listeners`.
async fn accept_any(listeners: &[TcpListener]) -> std::io::Result<TcpStream> {
    match listeners {
        [one] => one.accept().await.map(|(socket, _)| socket),
        [first, second, ..] => tokio::select! {
            accepted = first.accept() => accepted.map(|(socket, _)| socket),
            accepted = second.accept() => accepted.map(|(socket, _)| socket),
        },
        [] => Err(std::io::Error::other("nothing to listen on")),
    }
}

/// The request target of the first line (`/callback?code=…`), if one arrives within the deadline.
async fn read_request_target(socket: &mut TcpStream) -> Option<String> {
    let mut buffer = [0u8; 8192];
    let read = tokio::time::timeout(READ_DEADLINE, socket.read(&mut buffer)).await.ok()?.ok()?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    request.lines().next()?.split_whitespace().nth(1).map(str::to_owned)
}

/// A request that answers the sign-in, still open so the page can say how it went.
pub struct Arrival {
    socket: TcpStream,
    target: String,
    params: Vec<(String, String)>,
}

impl Arrival {
    fn new(socket: TcpStream, target: String) -> Self {
        let query = target.split_once('?').map(|(_, query)| query).unwrap_or_default();
        let params = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        Self { socket, target, params }
    }

    /// A query parameter, decoded.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// The provider's own account of a refusal: `error_description`, else `error`.
    pub fn error(&self) -> Option<&str> {
        self.param("error_description").or_else(|| self.param("error"))
    }

    /// The whole redirect as a URL, for a caller that parses it itself.
    pub fn redirect_url(&self) -> String {
        format!("http://127.0.0.1{}", self.target)
    }

    /// Answer with [`page`] and close.
    pub async fn answer(mut self, status: u16, title: &str, message: &str) {
        let accent = if status == 200 { "#5f8f76" } else { "#b45309" };
        let body = page(title, message, accent);
        let reason = match status {
            200 => "OK",
            404 => "Not Found",
            _ => "Bad Request",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = self.socket.write_all(response.as_bytes()).await;
        let _ = self.socket.flush().await;
    }
}

/// The page the browser lands on: a quiet card that says what happened and sends the reader back
/// to the terminal, where the result is. `title` and `message` are escaped, since either may repeat
/// what the request carried. It follows the reader's light or dark preference; a sign-in redirect is
/// no place to flash white.
pub fn page(title: &str, message: &str, accent: &str) -> String {
    let (title, message) = (escape(title), escape(message));
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>luxe</title><style>\
         :root{{color-scheme:light dark;--bg:#fbfbfc;--card:#fff;--line:#e4e4e7;--text:#18181b;\
         --muted:#6b7280;--accent:{accent}}}\
         @media(prefers-color-scheme:dark){{:root{{--bg:#101114;--card:#17181c;--line:#2a2c33;\
         --text:#e8e8ea;--muted:#9096a2}}}}\
         body{{background:var(--bg);color:var(--text);margin:0;min-height:100vh;display:flex;\
         align-items:center;justify-content:center;\
         font:15px/1.6 ui-sans-serif,system-ui,-apple-system,Segoe UI,sans-serif}}\
         .card{{max-width:30rem;padding:1.75rem 2rem;background:var(--card);\
         border:1px solid var(--line);border-top:2px solid var(--accent);border-radius:10px}}\
         h1{{margin:0 0 .4rem;font-size:1.0625rem;font-weight:600;letter-spacing:-.01em}}\
         p{{margin:0;color:var(--muted)}}\
         </style></head><body><div class=\"card\"><h1>{title}</h1><p>{message}</p></div></body></html>"
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests;
